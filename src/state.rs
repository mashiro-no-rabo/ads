use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use crate::Res;

pub struct State {
    pub dir: PathBuf,
}

impl State {
    pub fn new(root: &Path) -> Self {
        Self {
            dir: root.join(".ads"),
        }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }

    pub fn log_path(&self, service: &str) -> PathBuf {
        self.logs_dir().join(format!("{service}.log"))
    }

    pub fn init(&self) -> Res<()> {
        fs::create_dir_all(self.logs_dir())
            .map_err(|e| format!("{}: {e}", self.logs_dir().display()))
    }

    pub fn lock(&self) -> Res<PidLock> {
        let path = self.file("daemon.pid");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let mut s = String::new();
                let _ = file.read_to_string(&mut s);
                return Err(format!("already running (pid {})", s.trim()));
            }
            Err(TryLockError::Error(e)) => return Err(format!("{}: {e}", path.display())),
        }
        file.set_len(0)
            .and_then(|()| write!(file, "{}", std::process::id()))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(PidLock { _file: file, path })
    }

    /// The flock, not the pid, decides liveness, so a stale file or a reused pid never counts.
    pub fn daemon_pid(&self) -> Option<i32> {
        let mut file = File::open(self.file("daemon.pid")).ok()?;
        match file.try_lock() {
            Err(TryLockError::WouldBlock) => {
                let mut s = String::new();
                file.read_to_string(&mut s).ok()?;
                s.trim().parse().ok().filter(|&p| p > 0)
            }
            _ => None,
        }
    }
}

pub struct PidLock {
    _file: File,
    path: PathBuf,
}

impl Drop for PidLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}
