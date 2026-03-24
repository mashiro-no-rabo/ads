mod config;
mod ipc;
mod proc;

use config::Config;
use ipc::IpcCommand;
use proc::ProcManager;
use std::path::PathBuf;

const USAGE: &str = "\
ads — agent-dev-stack

USAGE:
    ads <COMMAND> [OPTIONS]

COMMANDS:
    start     Start all processes (default)
    stop      Stop running instance
    status    Show process states

OPTIONS:
    -c, --config <PATH>  Config file path (default: ads.toml)
    -h, --help           Show this help
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

fn cmd_start(config_path: PathBuf) {
    let config = match Config::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    let sock_path = ipc::socket_path(&config_path);

    if !config.ports.is_empty() {
        println!("Allocated ports:");
        for (name, port) in &config.ports {
            println!("  {name}: {port}");
        }
    }

    println!("Starting {} proc(s)...", config.procs.len());

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    rt.block_on(async {
        let mut manager = match ProcManager::spawn_all(&config.procs) {
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
