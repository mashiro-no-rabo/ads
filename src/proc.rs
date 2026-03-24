use chrono::Utc;
use command_group::{AsyncCommandGroup, AsyncGroupChild, Signal, UnixChildExt};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::config::ProcConfig;

pub struct ProcManager {
    children: HashMap<String, AsyncGroupChild>,
}

impl ProcManager {
    pub fn spawn_all(
        procs: &HashMap<String, ProcConfig>,
        log_dir: &Path,
        otel_port: Option<u16>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(log_dir)?;
        let mut children = HashMap::new();

        for (name, proc_config) in procs {
            let mut cmd = if let Some(cmd_args) = &proc_config.cmd {
                let mut c = Command::new(&cmd_args[0]);
                if cmd_args.len() > 1 {
                    c.args(&cmd_args[1..]);
                }
                c
            } else if let Some(shell) = &proc_config.shell {
                let mut c = Command::new("sh");
                c.args(["-c", shell]);
                c
            } else {
                unreachable!("validated in config");
            };

            if let Some(cwd) = &proc_config.cwd {
                cmd.current_dir(cwd);
            }
            if let Some(env) = &proc_config.env {
                for (k, v) in env {
                    cmd.env(k, v);
                }
            }
            if let Some(port) = otel_port {
                cmd.env(
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    format!("http://127.0.0.1:{port}"),
                );
                cmd.env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf");
            }

            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let mut child = cmd
                .group()
                .kill_on_drop(true)
                .spawn()
                .map_err(|e| format!("failed to spawn '{name}': {e}"))?;

            // Open log file for this process (interleaves stdout and stderr)
            let log_path = log_dir.join(format!("{name}.log"));
            let log_file = std::fs::File::create(&log_path)
                .map_err(|e| format!("failed to create log file '{}': {e}", log_path.display()))?;
            let log_file = Arc::new(Mutex::new(File::from_std(log_file)));

            // Take stdout/stderr and spawn forwarding tasks (dual: terminal + log)
            if let Some(stdout) = child.inner().stdout.take() {
                let tag = name.clone();
                let log = Arc::clone(&log_file);
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        println!("[{tag}] {line}");
                        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                        let mut f = log.lock().await;
                        let _ = f.write_all(format!("{ts} OUT {line}\n").as_bytes()).await;
                        let _ = f.flush().await;
                    }
                });
            }
            if let Some(stderr) = child.inner().stderr.take() {
                let tag = name.clone();
                let log = Arc::clone(&log_file);
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        eprintln!("[{tag}] {line}");
                        let ts = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                        let mut f = log.lock().await;
                        let _ = f.write_all(format!("{ts} ERR {line}\n").as_bytes()).await;
                        let _ = f.flush().await;
                    }
                });
            }

            let pid = child.id().unwrap_or(0);
            println!("[{name}] started (pid: {pid})");
            children.insert(name.clone(), child);
        }

        Ok(Self { children })
    }

    /// Returns current status of each process: Running(pid) or Exited(code).
    pub fn status(&mut self) -> HashMap<String, ProcStatus> {
        let mut result = HashMap::new();
        for (name, child) in &mut self.children {
            let status = match child.try_wait() {
                Ok(Some(exit)) => ProcStatus::Exited(exit.code()),
                Ok(None) => ProcStatus::Running(child.id().unwrap_or(0)),
                Err(e) => ProcStatus::Failed(e.to_string()),
            };
            result.insert(name.clone(), status);
        }
        result
    }

    /// Graceful shutdown: SIGTERM all, wait up to timeout, then SIGKILL stragglers.
    pub async fn shutdown(&mut self) {
        // Send SIGTERM to all process groups
        for (name, child) in &self.children {
            if let Err(e) = child.signal(Signal::SIGTERM) {
                eprintln!("[{name}] SIGTERM failed: {e}");
            }
        }

        let timeout = tokio::time::Duration::from_secs(5);
        for (name, child) in &mut self.children {
            match tokio::time::timeout(timeout, child.wait()).await {
                Ok(Ok(status)) => {
                    let code = status.code().map_or("signal".into(), |c| c.to_string());
                    println!("[{name}] exited ({code})");
                }
                Ok(Err(e)) => {
                    eprintln!("[{name}] wait error: {e}");
                }
                Err(_) => {
                    eprintln!("[{name}] did not exit within 5s, sending SIGKILL");
                    if let Err(e) = child.kill().await {
                        eprintln!("[{name}] kill failed: {e}");
                    } else {
                        println!("[{name}] killed");
                    }
                }
            }
        }
    }
}

pub enum ProcStatus {
    Running(u32),
    Exited(Option<i32>),
    Failed(String),
}

impl std::fmt::Display for ProcStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcStatus::Running(pid) => write!(f, "running (pid: {pid})"),
            ProcStatus::Exited(Some(code)) => write!(f, "exited ({code})"),
            ProcStatus::Exited(None) => write!(f, "exited (signal)"),
            ProcStatus::Failed(err) => write!(f, "failed: {err}"),
        }
    }
}
