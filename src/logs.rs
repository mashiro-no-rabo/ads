use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, IsTerminal, Read, Seek, SeekFrom, Write},
    sync::Arc,
    thread,
    time::Duration,
};

use crate::{Res, state::State};

pub const COLORS: [u8; 6] = [36, 33, 35, 32, 34, 31];

pub fn prefix(name: &str, width: usize, color: Option<u8>) -> String {
    match color {
        Some(c) => format!("\x1b[{c}m{name:<width$}\x1b[0m | "),
        None => format!("{name:<width$} | "),
    }
}

/// Each line goes out in a single `write_all` so concurrent pumps never interleave mid-line.
pub fn pump(src: impl Read, log: Arc<File>, echo: Option<String>) {
    let mut r = BufReader::new(src);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !buf.ends_with(b"\n") {
            buf.push(b'\n');
        }
        let _ = (&*log).write_all(&buf);
        if let Some(prefix) = &echo {
            let mut line = Vec::with_capacity(prefix.len() + buf.len());
            line.extend_from_slice(prefix.as_bytes());
            line.extend_from_slice(&buf);
            let _ = io::stdout().lock().write_all(&line);
        }
    }
}

struct Tail {
    path: std::path::PathBuf,
    prefix: String,
    offset: u64,
    pending: Vec<u8>,
}

pub fn cmd(state: &State, services: &[String], lines: usize, follow: bool) -> Res<()> {
    let names: Vec<String> = if services.is_empty() {
        let dir = state.logs_dir();
        let mut names: Vec<String> = fs::read_dir(&dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| {
                let name = e.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".log").map(str::to_string)
            })
            .collect();
        names.sort();
        names
    } else {
        services.to_vec()
    };
    let color = io::stdout().is_terminal();
    let width = names.iter().map(String::len).max().unwrap_or(0);
    let mut tails = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let path = state.log_path(name);
        if !path.is_file() {
            return Err(format!("no log for `{name}` at {}", path.display()));
        }
        tails.push(Tail {
            path,
            prefix: match names.len() {
                1 => String::new(),
                _ => prefix(name, width, color.then_some(COLORS[i % COLORS.len()])),
            },
            offset: 0,
            pending: Vec::new(),
        });
    }

    let mut out = io::stdout().lock();
    for t in &mut tails {
        let (bytes, end) =
            tail(&t.path, lines).map_err(|e| format!("{}: {e}", t.path.display()))?;
        t.offset = end;
        emit(&mut out, &t.prefix, &bytes, &mut t.pending);
    }
    let _ = out.flush();
    drop(out);

    if !follow {
        return Ok(());
    }
    loop {
        thread::sleep(Duration::from_millis(200));
        let mut out = io::stdout().lock();
        for t in &mut tails {
            let Ok(len) = fs::metadata(&t.path).map(|m| m.len()) else {
                continue;
            };
            if len < t.offset {
                t.offset = 0;
                t.pending.clear();
            }
            if len == t.offset {
                continue;
            }
            let Ok(bytes) = read_range(&t.path, t.offset, len) else {
                continue;
            };
            t.offset = len;
            emit(&mut out, &t.prefix, &bytes, &mut t.pending);
        }
        if out.flush().is_err() {
            break;
        }
    }
    Ok(())
}

fn emit(out: &mut impl Write, prefix: &str, bytes: &[u8], pending: &mut Vec<u8>) {
    pending.extend_from_slice(bytes);
    let Some(last) = pending.iter().rposition(|&b| b == b'\n') else {
        return;
    };
    let complete: Vec<u8> = pending.drain(..=last).collect();
    for line in complete.split_inclusive(|&b| b == b'\n') {
        let _ = out.write_all(prefix.as_bytes());
        let _ = out.write_all(line);
    }
}

fn read_range(path: &std::path::Path, from: u64, to: u64) -> io::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::new();
    f.take(to - from).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Reads backwards in chunks so large logs aren't loaded whole.
fn tail(path: &std::path::Path, n: usize) -> io::Result<(Vec<u8>, u64)> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut pos = len;
    let mut buf = Vec::new();
    while pos > 0 && buf.iter().filter(|&&b| b == b'\n').count() <= n {
        let step = pos.min(8192);
        pos -= step;
        f.seek(SeekFrom::Start(pos))?;
        let mut chunk = vec![0; step as usize];
        f.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&buf);
        buf = chunk;
    }
    let lines: Vec<&[u8]> = buf.split_inclusive(|&b| b == b'\n').collect();
    let start = lines.len().saturating_sub(n);
    Ok((lines[start..].concat(), len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_last_lines() {
        let dir = std::env::temp_dir().join(format!("ads-tail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.log");
        let body: String = (0..5000).map(|i| format!("line {i}\n")).collect();
        fs::write(&p, &body).unwrap();
        let (bytes, end) = tail(&p, 3).unwrap();
        assert_eq!(bytes, b"line 4997\nline 4998\nline 4999\n");
        assert_eq!(end, body.len() as u64);
        let (bytes, _) = tail(&p, 0).unwrap();
        assert!(bytes.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn emit_holds_partial_lines() {
        let mut out = Vec::new();
        let mut pending = Vec::new();
        emit(&mut out, "p | ", b"a\nb", &mut pending);
        assert_eq!(out, b"p | a\n");
        emit(&mut out, "p | ", b"c\n", &mut pending);
        assert_eq!(out, b"p | a\np | bc\n");
    }
}
