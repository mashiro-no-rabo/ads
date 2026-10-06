use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File},
    io::{self, IsTerminal, Write},
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    GRACE, Res,
    config::{self, Cmd, Service},
    logs, ports,
    state::{self, State},
    sys,
    watchdog::Lifeline,
};

enum Event {
    Signal,
    Exited {
        idx: usize,
        status: io::Result<ExitStatus>,
    },
    GroupGone(i32),
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Running,
    Stopping,
    Exited,
    Failed,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Running => "running",
            Phase::Stopping => "stopping",
            Phase::Exited => "exited",
            Phase::Failed => "failed",
        }
    }
}

struct Proc {
    svc: Service,
    pid: Option<i32>,
    phase: Phase,
    started: Option<u64>,
    exit: Option<String>,
}

struct Daemon {
    log: File,
    prefix: String,
}

impl Daemon {
    fn say(&self, msg: &str) {
        let line = format!("{msg}\n");
        let _ = (&self.log).write_all(line.as_bytes());
        let _ = io::stdout()
            .lock()
            .write_all(format!("{}{line}", self.prefix).as_bytes());
    }
}

pub fn up(config_path: &Path, only: &[String]) -> Res<()> {
    let cfg = config::load(config_path)?;
    for name in only {
        if !cfg.services.iter().any(|s| &s.name == name) {
            return Err(format!("unknown service `{name}`"));
        }
    }
    let state = State::new(&cfg.root);
    state.init()?;
    let _lock = state.lock()?;
    let ports = ports::allocate(&cfg.port_names())?;
    let services = cfg.render(&ports, &state.dir, only)?;
    write_ports(&state, &ports).map_err(|e| format!("writing ports: {e}"))?;

    let color = io::stdout().is_terminal();
    let width = services
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0)
        .max(3);
    let daemon = Daemon {
        log: File::create(state.file("daemon.log")).map_err(|e| format!("daemon.log: {e}"))?,
        prefix: match color {
            true => format!("\x1b[1m{:<width$}\x1b[0m | ", "ads"),
            false => format!("{:<width$} | ", "ads"),
        },
    };

    let sigset = sys::block_shutdown_signals().map_err(|e| format!("blocking signals: {e}"))?;
    let (tx, rx) = mpsc::channel();
    {
        let tx = tx.clone();
        thread::spawn(move || {
            while sys::wait_signal(&sigset).is_ok() {
                if tx.send(Event::Signal).is_err() {
                    break;
                }
            }
        });
    }
    let mut lifeline = Lifeline::spawn().map_err(|e| format!("starting watchdog: {e}"))?;

    for (name, port) in &ports {
        daemon.say(&format!("port {name}={port}"));
    }

    let mut groups = BTreeSet::new();
    let mut procs: Vec<Proc> = Vec::with_capacity(services.len());
    for (idx, svc) in services.into_iter().enumerate() {
        let prefix = logs::prefix(
            &svc.name,
            width,
            color.then_some(logs::COLORS[idx % logs::COLORS.len()]),
        );
        let mut p = Proc {
            svc,
            pid: None,
            phase: Phase::Failed,
            started: None,
            exit: None,
        };
        match spawn(&state, &p.svc, prefix, idx, &tx) {
            Ok(pid) => {
                lifeline.add(pid);
                groups.insert(pid);
                p.pid = Some(pid);
                p.phase = Phase::Running;
                p.started = Some(now());
                daemon.say(&format!(
                    "started {} (pid {pid}): {}",
                    p.svc.name,
                    p.svc.cmd.display()
                ));
            }
            Err(e) => daemon.say(&format!("failed to start {}: {e}", p.svc.name)),
        }
        procs.push(p);
    }
    write_status(&state, &procs);

    let mut shutting = false;
    let mut forced = false;
    loop {
        let busy = procs
            .iter()
            .any(|p| matches!(p.phase, Phase::Running | Phase::Stopping));
        if shutting && !busy && groups.is_empty() {
            break;
        }
        let ev = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(ev) => ev,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if lifeline.check() {
                    daemon.say("watchdog died, restarted");
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match ev {
            Event::Signal if !shutting => {
                shutting = true;
                daemon.say("stopping (Ctrl-C again to kill)");
                for p in procs.iter_mut().filter(|p| p.phase == Phase::Running) {
                    p.phase = Phase::Stopping;
                    stop_group(p.pid.unwrap(), &tx);
                }
            }
            Event::Signal if !forced => {
                forced = true;
                daemon.say("killing");
                for &g in &groups {
                    let _ = sys::killpg(g, sys::SIGKILL);
                }
            }
            Event::Signal => {}
            Event::Exited { idx, status } => {
                let p = &mut procs[idx];
                let was_stopping = p.phase == Phase::Stopping;
                let exit = match status {
                    Ok(s) => describe(s),
                    Err(e) => format!("wait failed: {e}"),
                };
                daemon.say(&format!("{} exited ({exit})", p.svc.name));
                p.phase = Phase::Exited;
                p.exit = Some(exit);
                // The leader is gone but the group may still have members (backgrounded children).
                if !was_stopping {
                    stop_group(p.pid.unwrap(), &tx);
                }
            }
            Event::GroupGone(g) => {
                groups.remove(&g);
                lifeline.remove(g);
            }
        }
        write_status(&state, &procs);
    }
    lifeline.close();
    daemon.say("stopped");
    Ok(())
}

fn spawn(
    state: &State,
    svc: &Service,
    prefix: String,
    idx: usize,
    tx: &mpsc::Sender<Event>,
) -> io::Result<i32> {
    let mut cmd = match &svc.cmd {
        Cmd::Shell(s) => {
            let mut c = Command::new("/bin/sh");
            c.arg("-c").arg(s);
            c
        }
        Cmd::Argv(a) => {
            let mut c = Command::new(&a[0]);
            c.args(&a[1..]);
            c
        }
    };
    let log = Arc::new(File::create(state.log_path(&svc.name))?);
    sys::unblock_signals_on_exec(&mut cmd);
    let mut child = cmd
        .current_dir(&svc.cwd)
        .envs(svc.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let pid = child.id() as i32;
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    {
        let (log, prefix) = (log.clone(), prefix.clone());
        thread::spawn(move || logs::pump(out, log, prefix));
    }
    thread::spawn(move || logs::pump(err, log, prefix));
    let tx = tx.clone();
    thread::spawn(move || {
        let status = child.wait();
        let _ = tx.send(Event::Exited { idx, status });
    });
    Ok(pid)
}

fn stop_group(pgid: i32, tx: &mpsc::Sender<Event>) {
    let tx = tx.clone();
    thread::spawn(move || {
        sys::terminate_groups(&[pgid], GRACE);
        let _ = tx.send(Event::GroupGone(pgid));
    });
}

fn describe(s: ExitStatus) -> String {
    match (s.code(), s.signal()) {
        (Some(c), _) => format!("code {c}"),
        (None, Some(sig)) => format!("signal {sig}"),
        _ => "unknown".into(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_ports(state: &State, ports: &BTreeMap<String, u16>) -> io::Result<()> {
    let env: String = ports
        .iter()
        .map(|(n, p)| format!("{}={p}\n", ports::env_name(n)))
        .collect();
    let json = format!(
        "{{{}}}\n",
        ports
            .iter()
            .map(|(n, p)| format!("\"{n}\":{p}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    state::write_atomic(&state.file("ports.env"), env.as_bytes())?;
    state::write_atomic(&state.file("ports.json"), json.as_bytes())
}

const STATUS_HEADER: &str = "name\tpid\tstate\tstarted\texit";

fn write_status(state: &State, procs: &[Proc]) {
    let mut s = format!("{STATUS_HEADER}\n");
    for p in procs {
        let dash = || "-".to_string();
        let _ = writeln!(
            s,
            "{}\t{}\t{}\t{}\t{}",
            p.svc.name,
            p.pid.map_or_else(dash, |v| v.to_string()),
            p.phase.as_str(),
            p.started.map_or_else(dash, |v| v.to_string()),
            p.exit.clone().unwrap_or_else(dash),
        );
    }
    let _ = state::write_atomic(&state.file("status"), s.as_bytes());
}

pub fn down(state: &State) -> Res<()> {
    let pid = state.daemon_pid().ok_or("not running")?;
    sys::kill(pid, sys::SIGTERM).map_err(|e| format!("signalling pid {pid}: {e}"))?;
    let deadline = Instant::now() + GRACE + Duration::from_secs(10);
    while state.daemon_pid().is_some() {
        if Instant::now() > deadline {
            return Err(format!("daemon (pid {pid}) did not stop"));
        }
        thread::sleep(Duration::from_millis(50));
    }
    println!("stopped");
    Ok(())
}

pub fn ps(state: &State) -> Res<()> {
    match state.daemon_pid() {
        Some(pid) => println!("daemon running (pid {pid})"),
        None => println!("daemon not running"),
    }
    let Ok(text) = fs::read_to_string(state.file("status")) else {
        return Ok(());
    };
    println!();
    print!("{}", align(&text));
    Ok(())
}

pub fn ports_cmd(state: &State, format: &str) -> Res<()> {
    if state.daemon_pid().is_none() {
        return Err("not running".into());
    }
    let file = match format {
        "json" => "ports.json",
        _ => "ports.env",
    };
    let text = fs::read_to_string(state.file(file)).map_err(|e| format!("{file}: {e}"))?;
    print!("{text}");
    Ok(())
}

fn align(tsv: &str) -> String {
    let rows: Vec<Vec<&str>> = tsv.lines().map(|l| l.split('\t').collect()).collect();
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.len())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for r in &rows {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{s:<w$}", w = widths[i]))
            .collect();
        let _ = writeln!(out, "{}", line.join("  ").trim_end());
    }
    out
}

pub fn check(config_path: &Path) -> Res<()> {
    let cfg = config::load(config_path)?;
    let state = State::new(&cfg.root);
    let ports = ports::allocate(&cfg.port_names())?;
    let services = cfg.render(&ports, &state.dir, &[])?;
    println!("config: {}", config_path.display());
    if !ports.is_empty() {
        println!("ports:");
        for (n, p) in &ports {
            println!("  {n} = {p}");
        }
    }
    println!("services:");
    for s in &services {
        println!("  {}:", s.name);
        println!("    cmd: {}", s.cmd.display());
        println!("    cwd: {}", s.cwd.display());
        for (k, v) in s.env.iter().filter(|(k, _)| !k.starts_with("ADS_")) {
            println!("    env: {k}={v}");
        }
    }
    Ok(())
}
