use std::{
    collections::BTreeSet,
    env,
    io::{self, BufRead, Write},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
};

use crate::{GRACE, sys};

/// Runs as `ads __watchdog`. The daemon holds the write end of stdin; EOF without the clean
/// marker means the daemon died without stopping its groups (e.g. SIGKILL), and macOS has no
/// parent-death signal to do it for us.
pub fn run() {
    let mut groups = BTreeSet::new();
    let mut clean = false;
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match line.split_at_checked(1) {
            Some(("+", g)) => _ = g.parse().map(|g: i32| groups.insert(g)),
            Some(("-", g)) => _ = g.parse().map(|g: i32| groups.remove(&g)),
            Some((".", _)) => clean = true,
            _ => {}
        }
    }
    if !clean && !groups.is_empty() {
        let groups: Vec<i32> = groups.into_iter().collect();
        sys::terminate_groups(&groups, GRACE);
    }
}

pub struct Lifeline {
    child: Child,
    groups: BTreeSet<i32>,
}

impl Lifeline {
    pub fn spawn() -> io::Result<Self> {
        Ok(Self {
            child: start()?,
            groups: BTreeSet::new(),
        })
    }

    pub fn add(&mut self, pgid: i32) {
        self.groups.insert(pgid);
        self.send(&format!("+{pgid}\n"));
    }

    pub fn remove(&mut self, pgid: i32) {
        self.groups.remove(&pgid);
        self.send(&format!("-{pgid}\n"));
    }

    /// Returns true if the watchdog had died and was restarted.
    pub fn check(&mut self) -> bool {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            self.respawn();
            return true;
        }
        false
    }

    pub fn close(mut self) {
        let _ = self.write(".\n");
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }

    fn send(&mut self, msg: &str) {
        if self.write(msg).is_err() {
            self.respawn();
        }
    }

    fn write(&mut self, msg: &str) -> io::Result<()> {
        match self.child.stdin.as_mut() {
            Some(w) => w.write_all(msg.as_bytes()),
            None => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    fn respawn(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        match start() {
            Ok(c) => {
                self.child = c;
                let all: String = self.groups.iter().map(|g| format!("+{g}\n")).collect();
                if let Err(e) = self.write(&all) {
                    eprintln!("ads: watchdog: {e}");
                }
            }
            Err(e) => eprintln!("ads: failed to restart watchdog: {e}"),
        }
    }
}

fn start() -> io::Result<Child> {
    let mut cmd = Command::new(env::current_exe()?);
    sys::unblock_signals_on_exec(&mut cmd);
    cmd.arg("__watchdog")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
}
