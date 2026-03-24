use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::proc::ProcStatus;

pub enum IpcCommand {
    Status {
        reply: oneshot::Sender<HashMap<String, ProcStatus>>,
    },
    Stop,
    Logs {
        reply: oneshot::Sender<String>,
    },
    Search {
        pattern: String,
        reply: oneshot::Sender<String>,
    },
}

/// Hash the canonical config path for deterministic derived paths.
fn config_hash(config_path: &Path) -> u64 {
    let canonical = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());
    let mut hasher = std::hash::DefaultHasher::new();
    canonical.hash(&mut hasher);
    hasher.finish()
}

/// Derive a deterministic socket path from the config file's canonical path.
pub fn socket_path(config_path: &Path) -> PathBuf {
    let hash = config_hash(config_path);
    std::env::temp_dir().join(format!("ads-{hash:x}.sock"))
}

/// Derive the log directory from the config file's canonical path.
pub fn log_dir(config_path: &Path) -> PathBuf {
    let hash = config_hash(config_path);
    std::env::temp_dir()
        .join(format!("ads-{hash:x}"))
        .join("logs")
}

pub struct IpcServer {
    listener: UnixListener,
    cmd_tx: mpsc::Sender<IpcCommand>,
}

/// Start the IPC server, returning a receiver for commands and the server handle.
pub fn start_server(
    sock_path: &Path,
) -> Result<(mpsc::Receiver<IpcCommand>, IpcServer), Box<dyn std::error::Error>> {
    // Remove stale socket if it exists
    let _ = std::fs::remove_file(sock_path);
    let listener = UnixListener::bind(sock_path)?;
    let (cmd_tx, cmd_rx) = mpsc::channel(16);
    Ok((cmd_rx, IpcServer { listener, cmd_tx }))
}

impl IpcServer {
    pub async fn run(self) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    let tx = self.cmd_tx.clone();
                    tokio::spawn(handle_client(stream, tx));
                }
                Err(e) => {
                    eprintln!("IPC accept error: {e}");
                }
            }
        }
    }
}

async fn handle_client(stream: UnixStream, cmd_tx: mpsc::Sender<IpcCommand>) {
    if let Err(e) = handle_client_inner(stream, cmd_tx).await {
        eprintln!("IPC client error: {e}");
    }
}

async fn handle_client_inner(
    stream: UnixStream,
    cmd_tx: mpsc::Sender<IpcCommand>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let line = match lines.next_line().await? {
        Some(l) => l,
        None => return Ok(()),
    };

    match line.trim() {
        "STATUS" => {
            let (reply_tx, reply_rx) = oneshot::channel();
            cmd_tx
                .send(IpcCommand::Status { reply: reply_tx })
                .await?;
            match reply_rx.await {
                Ok(statuses) => {
                    let mut sorted: Vec<_> = statuses.into_iter().collect();
                    sorted.sort_by(|(a, _), (b, _)| a.cmp(b));
                    for (name, status) in sorted {
                        writer
                            .write_all(format!("{name}: {status}\n").as_bytes())
                            .await?;
                    }
                }
                Err(_) => {
                    writer
                        .write_all(b"error: server shutting down\n")
                        .await?;
                }
            }
        }
        "STOP" => {
            writer.write_all(b"stopping...\n").await?;
            let _ = cmd_tx.send(IpcCommand::Stop).await;
        }
        "LOGS" => {
            let (reply_tx, reply_rx) = oneshot::channel();
            cmd_tx.send(IpcCommand::Logs { reply: reply_tx }).await?;
            match reply_rx.await {
                Ok(response) => {
                    writer.write_all(response.as_bytes()).await?;
                }
                Err(_) => {
                    writer
                        .write_all(b"error: server shutting down\n")
                        .await?;
                }
            }
        }
        other if other.starts_with("SEARCH ") => {
            let pattern = other[7..].to_string();
            let (reply_tx, reply_rx) = oneshot::channel();
            cmd_tx
                .send(IpcCommand::Search {
                    pattern,
                    reply: reply_tx,
                })
                .await?;
            match reply_rx.await {
                Ok(response) => {
                    writer.write_all(response.as_bytes()).await?;
                }
                Err(_) => {
                    writer
                        .write_all(b"error: server shutting down\n")
                        .await?;
                }
            }
        }
        other => {
            writer
                .write_all(format!("unknown command: {other}\n").as_bytes())
                .await?;
        }
    }

    Ok(())
}

/// Send a command to a running ads instance and return the response.
pub async fn send_command(
    sock_path: &Path,
    command: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(sock_path)
        .await
        .map_err(|_| "no running ads instance found (is `ads start` running?)")?;
    let (reader, mut writer) = stream.into_split();
    writer
        .write_all(format!("{command}\n").as_bytes())
        .await?;
    drop(writer);

    let mut output = String::new();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}
