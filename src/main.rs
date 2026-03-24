mod channel;
mod config;
mod ipc;
mod otel;
mod proc;

use clap::{Parser, Subcommand};
use config::Config;
use ipc::IpcCommand;
use proc::ProcManager;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

/// ads — agent-dev-stack
///
/// Manages a local dev stack for Claude Code sessions: spawns processes,
/// captures logs, collects OpenTelemetry traces, and exposes tools via MCP.
#[derive(Parser)]
#[command(name = "ads", version)]
struct Cli {
    /// Config file path
    #[arg(short, long, default_value = "ads.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start all processes (default when no command given)
    Start,
    /// Stop a running ads instance
    Stop,
    /// Show process states
    Status,
    /// Show log paths or search logs
    Logs {
        /// Show log file path for a specific process
        name: Option<String>,

        /// Search across all log files for a pattern
        #[arg(short, long)]
        search: Option<String>,
    },
    /// List recent OpenTelemetry traces
    Traces,
    /// Show trace details by ID
    Trace {
        /// The trace ID to look up
        trace_id: String,
    },
    /// Start MCP server on stdio for Claude Code integration
    Channel,
}

fn main() {
    let cli = Cli::parse();
    let config_path = cli.config;

    match cli.command {
        None | Some(Command::Start) => cmd_start(config_path),
        Some(Command::Stop) => cmd_stop(config_path),
        Some(Command::Status) => cmd_status(config_path),
        Some(Command::Traces) => cmd_traces(config_path),
        Some(Command::Trace { trace_id }) => cmd_trace(config_path, &trace_id),
        Some(Command::Logs { name, search }) => cmd_logs(config_path, name, search),
        Some(Command::Channel) => cmd_channel(config_path),
    }
}

fn cmd_start(config_path: PathBuf) {
    let config = match Config::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let sock_path = ipc::socket_path(&config_path);
    let log_dir = ipc::log_dir(&config_path);

    if !config.ports.is_empty() {
        println!("Allocated ports:");
        for (name, port) in &config.ports {
            println!("  {name}: {port}");
        }
    }

    println!("Starting {} proc(s)...", config.procs.len());
    println!("Logs: {}", log_dir.display());

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let (otel_port, trace_store, _otel_handle) = match otel::start_otel_server().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to start OTel server: {e}");
                std::process::exit(1);
            }
        };
        println!("OTel endpoint: http://127.0.0.1:{otel_port}");

        let mut manager = match ProcManager::spawn_all(&config.procs, &log_dir, Some(otel_port)) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };

        let (mut ipc_rx, ipc_server) = match ipc::start_server(&sock_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to start IPC server: {e}");
                std::process::exit(1);
            }
        };

        tokio::spawn(ipc_server.run());

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    println!("\nShutting down...");
                    break;
                }
                cmd = ipc_rx.recv() => {
                    match cmd {
                        Some(IpcCommand::Status { reply }) => {
                            let _ = reply.send(manager.status());
                        }
                        Some(IpcCommand::Stop) => {
                            println!("Received stop command");
                            break;
                        }
                        Some(IpcCommand::Logs { reply }) => {
                            let _ = reply.send(format!("{}\n", log_dir.display()));
                        }
                        Some(IpcCommand::Search { pattern, reply }) => {
                            let result = search_logs(&log_dir, &pattern);
                            let _ = reply.send(result);
                        }
                        Some(IpcCommand::Traces { reply }) => {
                            let store = trace_store.lock().unwrap();
                            let traces = store.list_traces();
                            let mut output = String::new();
                            for t in &traces {
                                output.push_str(&format!(
                                    "{} {} {} ({} spans, {:.1}ms)\n",
                                    t.trace_id, t.service_name, t.root_span_name,
                                    t.span_count, t.duration_ms,
                                ));
                            }
                            if output.is_empty() {
                                output.push_str("no traces collected\n");
                            }
                            let _ = reply.send(output);
                        }
                        Some(IpcCommand::Trace { trace_id, reply }) => {
                            let store = trace_store.lock().unwrap();
                            let output = match store.get_trace(&trace_id) {
                                Some(t) => {
                                    let mut out = format!(
                                        "trace: {}\nservice: {}\nroot: {}\nspans: {}\n\n",
                                        t.trace_id, t.service_name, t.root_span_name, t.spans.len(),
                                    );
                                    for s in &t.spans {
                                        let dur_ms = s.end_time_unix_nano
                                            .saturating_sub(s.start_time_unix_nano)
                                            as f64 / 1_000_000.0;
                                        out.push_str(&format!(
                                            "  {} {} {:.1}ms status={} parent={}\n",
                                            s.span_id, s.name, dur_ms, s.status_code,
                                            if s.parent_span_id.is_empty() { "-" } else { &s.parent_span_id },
                                        ));
                                        for (k, v) in &s.attributes {
                                            out.push_str(&format!("    {k}={v}\n"));
                                        }
                                    }
                                    out
                                }
                                None => format!("trace not found: {trace_id}\n"),
                            };
                            let _ = reply.send(output);
                        }
                        None => break,
                    }
                }
            }
        }

        manager.shutdown().await;
        let _ = std::fs::remove_file(&sock_path);
    });
}

fn cmd_stop(config_path: PathBuf) {
    let sock_path = ipc::socket_path(&config_path);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        match ipc::send_command(&sock_path, "STOP").await {
            Ok(response) => print!("{response}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });
}

fn cmd_status(config_path: PathBuf) {
    let sock_path = ipc::socket_path(&config_path);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        match ipc::send_command(&sock_path, "STATUS").await {
            Ok(response) => print!("{response}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });
}

fn cmd_traces(config_path: PathBuf) {
    let sock_path = ipc::socket_path(&config_path);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        match ipc::send_command(&sock_path, "TRACES").await {
            Ok(response) => print!("{response}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });
}

fn cmd_trace(config_path: PathBuf, trace_id: &str) {
    let sock_path = ipc::socket_path(&config_path);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        match ipc::send_command(&sock_path, &format!("TRACE {trace_id}")).await {
            Ok(response) => print!("{response}"),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
    });
}

fn cmd_logs(config_path: PathBuf, name: Option<String>, search: Option<String>) {
    let log_dir = ipc::log_dir(&config_path);

    if let Some(pattern) = search {
        if !log_dir.is_dir() {
            eprintln!("No log directory found (is `ads start` running?)");
            std::process::exit(1);
        }
        let result = search_logs(&log_dir, &pattern);
        if result.is_empty() {
            eprintln!("No matches found for '{pattern}'");
            std::process::exit(1);
        }
        print!("{result}");
    } else if let Some(name) = name {
        // Print log file path for a specific process
        let log_path = log_dir.join(format!("{name}.log"));
        if log_path.is_file() {
            println!("{}", log_path.display());
        } else {
            eprintln!("No log file found for process '{name}'");
            std::process::exit(1);
        }
    } else {
        // Print log directory path
        if log_dir.is_dir() {
            println!("{}", log_dir.display());
        } else {
            eprintln!("No log directory found (is `ads start` running?)");
            std::process::exit(1);
        }
    }
}

fn cmd_channel(config_path: PathBuf) {
    let sock_path = ipc::socket_path(&config_path);
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        if let Err(e) = channel::run(sock_path).await {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    });
}

/// Search across all log files in a directory for lines containing `pattern`.
/// Returns matching lines formatted as `{proc}:{line_number}: {content}`.
fn search_logs(log_dir: &std::path::Path, pattern: &str) -> String {
    let mut entries: Vec<_> = match std::fs::read_dir(log_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
            .collect(),
        Err(_) => return String::new(),
    };
    entries.sort_by_key(|e| e.file_name());

    let mut output = String::new();
    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap_or_default().to_string_lossy();
        let file = match std::fs::File::open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        for (line_num, line) in BufReader::new(file).lines().enumerate() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.contains(pattern) {
                output.push_str(&format!("{name}:{}: {line}\n", line_num + 1));
            }
        }
    }
    output
}
