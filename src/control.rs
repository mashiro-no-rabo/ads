use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::mpsc,
    thread,
    time::Duration,
};

use crate::Res;

/// One request line per connection, answered with one reply line.
pub fn serve(
    path: &Path,
    handle: impl Fn(String, mpsc::Sender<String>) + Clone + Send + 'static,
) -> io::Result<()> {
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handle = handle.clone();
            thread::spawn(move || {
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    return;
                }
                let (tx, rx) = mpsc::channel();
                handle(line.trim().to_string(), tx);
                if let Ok(reply) = rx.recv() {
                    let _ = (&stream).write_all(format!("{reply}\n").as_bytes());
                }
            });
        }
    });
    Ok(())
}

pub fn request(path: &Path, line: &str) -> Res<String> {
    let err = |e: io::Error| format!("{}: {e}", path.display());
    let stream = UnixStream::connect(path).map_err(err)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(err)?;
    (&stream)
        .write_all(format!("{line}\n").as_bytes())
        .map_err(err)?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).map_err(err)?;
    match reply.trim() {
        "" => Err("daemon closed the connection without replying".into()),
        r => Ok(r.to_string()),
    }
}
