use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn setup(name: &str, toml: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("ads.toml"), toml).unwrap();
    dir
}

fn ads(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ads"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

/// Stops the daemon if a test panics, so it can't outlive the run.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(self.0.id() as i32, libc::SIGTERM) };
            let _ = self.0.wait();
        }
    }
}

impl std::ops::Deref for Daemon {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Daemon {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

fn up(dir: &Path) -> Daemon {
    let out = || fs::File::create(dir.join("up.out")).unwrap();
    Daemon(
        Command::new(env!("CARGO_BIN_EXE_ads"))
            .arg("up")
            .current_dir(dir)
            .stdout(out())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    cond()
}

/// (name, pid, state, exit)
fn status(dir: &Path) -> Vec<(String, i32, String, String)> {
    let Ok(text) = fs::read_to_string(dir.join(".ads/status")) else {
        return vec![];
    };
    text.lines()
        .skip(1)
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (
                c[0].into(),
                c[1].parse().unwrap_or(0),
                c[2].into(),
                c[4].into(),
            )
        })
        .collect()
}

fn running_groups(dir: &Path, n: usize) -> Vec<i32> {
    let mut pids = vec![];
    assert!(
        wait_for(Duration::from_secs(5), || {
            let s = status(dir);
            pids = s.iter().filter(|r| r.2 == "running").map(|r| r.1).collect();
            pids.len() == n
        }),
        "services did not start: {:?}",
        status(dir)
    );
    pids
}

fn group_alive(pgid: i32) -> bool {
    unsafe { libc::killpg(pgid, 0) == 0 }
}

fn wait_exit(child: &mut Child, timeout: Duration) -> bool {
    wait_for(timeout, || child.try_wait().unwrap().is_some())
}

const TWO_SLEEPERS: &str = r#"
[services.shell]
cmd = "sleep 1000 & sleep 1000"

[services.argv]
cmd = ["sleep", "1000"]
"#;

#[test]
fn sigterm_stops_all_groups() {
    let dir = setup("sigterm", TWO_SLEEPERS);
    let mut d = up(&dir);
    let groups = running_groups(&dir, 2);
    unsafe { libc::kill(d.id() as i32, libc::SIGTERM) };
    assert!(wait_exit(&mut d, Duration::from_secs(5)));
    assert!(d.wait().unwrap().success());
    for g in groups {
        assert!(!group_alive(g), "group {g} survived");
    }
    assert!(!dir.join(".ads/daemon.pid").exists());
    assert!(
        status(&dir)
            .iter()
            .all(|r| r.2 == "exited" && r.3 == "signal 15")
    );
}

#[test]
fn sigkill_daemon_watchdog_cleans_up() {
    let dir = setup("sigkill", TWO_SLEEPERS);
    let mut d = up(&dir);
    let groups = running_groups(&dir, 2);
    d.kill().unwrap();
    d.wait().unwrap();
    assert!(
        wait_for(Duration::from_secs(10), || groups
            .iter()
            .all(|&g| !group_alive(g))),
        "groups survived daemon SIGKILL"
    );
}

#[test]
fn sigterm_ignored_falls_back_to_sigkill() {
    let dir = setup(
        "stubborn",
        "[services.stubborn]\ncmd = \"trap '' TERM; sleep 1000\"\n",
    );
    let mut d = up(&dir);
    let groups = running_groups(&dir, 1);
    let t = Instant::now();
    let out = ads(&dir, &["down"]);
    assert!(out.status.success(), "{out:?}");
    assert!(t.elapsed() >= Duration::from_secs(4), "{:?}", t.elapsed());
    assert!(wait_exit(&mut d, Duration::from_secs(2)));
    assert!(!group_alive(groups[0]));
    assert_eq!(status(&dir)[0].3, "signal 9");
}

#[test]
fn leftover_children_killed_when_leader_exits() {
    let dir = setup(
        "leftover",
        "[services.bg]\ncmd = \"sleep 1000 & echo started\"\n\n[services.keep]\ncmd = [\"sleep\", \"1000\"]\n",
    );
    let mut d = up(&dir);
    let mut bg = 0;
    assert!(wait_for(Duration::from_secs(5), || {
        status(&dir)
            .iter()
            .find(|r| r.0 == "bg" && r.2 == "exited")
            .map(|r| bg = r.1)
            .is_some()
    }));
    assert!(
        wait_for(Duration::from_secs(5), || !group_alive(bg)),
        "backgrounded child survived"
    );
    assert!(ads(&dir, &["down"]).status.success());
    assert!(wait_exit(&mut d, Duration::from_secs(2)));
}

#[test]
fn signal_mask_not_inherited() {
    let dir = setup(
        "sigmask",
        "[services.selfterm]\ncmd = \"kill -TERM $$; sleep 5\"\n",
    );
    let mut d = up(&dir);
    assert!(
        wait_for(Duration::from_secs(3), || status(&dir)
            .first()
            .is_some_and(|r| r.3 == "signal 15")),
        "{:?}",
        status(&dir)
    );
    assert!(ads(&dir, &["down"]).status.success());
    assert!(wait_exit(&mut d, Duration::from_secs(2)));
}

#[test]
fn ports_env_and_templates() {
    let dir = setup(
        "ports",
        r#"
[services.web]
cmd = "echo \"$ADS_PORT_WEB {{ports.web}} $FOO\" > {{state}}/out; sleep 1000"
env = { FOO = "http://127.0.0.1:{{ports.web}}" }
"#,
    );
    let mut d = up(&dir);
    running_groups(&dir, 1);
    let out = dir.join(".ads/out");
    assert!(wait_for(Duration::from_secs(3), || fs::read_to_string(
        &out
    )
    .is_ok_and(|s| s.ends_with('\n'))));
    let ports = String::from_utf8(ads(&dir, &["ports"]).stdout).unwrap();
    let port: u16 = ports
        .trim()
        .strip_prefix("ADS_PORT_WEB=")
        .unwrap()
        .parse()
        .unwrap();
    assert!((8000..=8999).contains(&port));
    assert_eq!(
        fs::read_to_string(&out).unwrap(),
        format!("{port} {port} http://127.0.0.1:{port}\n")
    );
    let json = String::from_utf8(ads(&dir, &["ports", "--json"]).stdout).unwrap();
    assert_eq!(json, format!("{{\"web\":{port}}}\n"));

    let second = ads(&dir, &["up"]);
    assert!(!second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));

    assert!(ads(&dir, &["down"]).status.success());
    assert!(wait_exit(&mut d, Duration::from_secs(2)));
    assert!(!ads(&dir, &["ports"]).status.success());
}

#[test]
fn logs_written() {
    let dir = setup(
        "logs",
        "[services.a]\ncmd = \"echo out; echo err >&2; sleep 1000\"\n",
    );
    let mut d = up(&dir);
    running_groups(&dir, 1);
    assert!(wait_for(Duration::from_secs(3), || {
        let o = ads(&dir, &["logs", "a"]);
        let s = String::from_utf8_lossy(&o.stdout);
        s.contains("out\n") && s.contains("err\n")
    }));
    assert!(ads(&dir, &["down"]).status.success());
    assert!(wait_exit(&mut d, Duration::from_secs(2)));
}
