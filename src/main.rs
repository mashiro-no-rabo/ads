mod config;
mod ipc;
mod proc;

use config::Config;
use ipc::IpcCommand;
use proc::ProcManager;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

const USAGE: &str = "\
ads — agent-dev-stack

USAGE:
    ads <COMMAND> [OPTIONS]

COMMANDS:
    start     Start all processes (default)
    stop      Stop running instance
    status    Show process states
    logs      Show log paths or search logs

OPTIONS:
    -c, --config <PATH>  Config file path (default: ads.toml)
    -h, --help           Show this help
";

const LOGS_USAGE: &str = "\
ads logs — Show log paths or search logs

USAGE:
    ads logs [NAME] [OPTIONS]

ARGS:
    <NAME>               Show log file path for a specific process

OPTIONS:
    -s, --search <PATTERN>  Search across all log files
    -h, --help              Show this help
";

fn main() {
    let mut args = pico_args::Arguments::from_env();

    if args.contains(["-h", "--help"]) {
        print!("{USAGE}");
        return;
    }

    let subcommand = args.subcommand().unwrap_or_default();
    let config_path: PathBuf = args
        .opt_value_from_str(["-c", "--config"])
        .unwrap_or_default()
        .unwrap_or_else(|| PathBuf::from("ads.toml"));

    match subcommand.as_deref() {
        Some("logs") => cmd_logs(config_path, args),
        _ => {
            let remaining = args.finish();
            if !remaining.is_empty() {
                eprintln!("Unknown arguments: {remaining:?}");
                eprint!("{USAGE}");
                std::process::exit(1);
            }
            match subcommand.as_deref() {
                None | Some("start") => cmd_start(config_path),
                Some("stop") => cmd_stop(config_path),
                Some("status") => cmd_status(config_path),
                Some(other) => {
                    eprintln!("Unknown command: {other}");
                    eprint!("{USAGE}");
                    std::process::exit(1);
                }
            }
        }
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
        let mut manager = match ProcManager::spawn_all(&config.procs, &log_dir) {
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

fn cmd_logs(config_path: PathBuf, mut args: pico_args::Arguments) {
    if args.contains(["-h", "--help"]) {
        print!("{LOGS_USAGE}");
        return;
    }

    let search_pattern: Option<String> = args
        .opt_value_from_str(["-s", "--search"])
        .unwrap_or_default();

    let proc_name: Option<String> = args.opt_free_from_str().unwrap_or_default();

    let remaining = args.finish();
    if !remaining.is_empty() {
        eprintln!("Unknown arguments: {remaining:?}");
        eprint!("{LOGS_USAGE}");
        std::process::exit(1);
    }

    let log_dir = ipc::log_dir(&config_path);

    if let Some(pattern) = search_pattern {
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
    } else if let Some(name) = proc_name {
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
