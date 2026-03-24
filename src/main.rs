mod config;
mod proc;

use config::Config;
use proc::ProcManager;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    let config_path = PathBuf::from("ads.toml");
    let config = match Config::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    if !config.ports.is_empty() {
        println!("Allocated ports:");
        for (name, port) in &config.ports {
            println!("  {name}: {port}");
        }
    }

    println!("Starting {} proc(s)...", config.procs.len());

    let mut manager = match ProcManager::spawn_all(&config.procs) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Wait for ctrl-c
    if let Err(e) = tokio::signal::ctrl_c().await {
        eprintln!("Failed to listen for ctrl-c: {e}");
    }

    println!("\nShutting down...");
    manager.shutdown().await;
}
