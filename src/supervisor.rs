use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs::{self, File, OpenOptions},
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
    control, logs, ports,
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
    StepExited(io::Result<ExitStatus>),
    GroupGone(i32),
    Control {
        line: String,
        reply: mpsc::Sender<String>,
    },
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
    prefix: String,
    pid: Option<i32>,
    phase: Phase,
    started: Option<u64>,
    exit: Option<String>,
    /// Start again once the current instance's whole group is gone (restart / start while stopping).
    pending_start: bool,
    waiters: Vec<mpsc::Sender<String>>,
}

struct Daemon {
    log: File,
    prefix: String,
    echo: bool,
}

impl Daemon {
    fn say(&self, msg: &str) {
        let line = format!("{msg}\n");
        let _ = (&self.log).write_all(line.as_bytes());
        if self.echo {
            let _ = io::stdout()
                .lock()
                .write_all(format!("{}{line}", self.prefix).as_bytes());
        }
    }
}

struct Supervisor {
    state: State,
    daemon: Daemon,
    tx: mpsc::Sender<Event>,
    procs: Vec<Proc>,
    groups: BTreeSet<i32>,
    lifeline: Lifeline,
    shutting: bool,
    forced: bool,
}

pub fn up(config_path: &Path, only: &[String], detached: bool) -> Res<()> {
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
    let steps = cfg.render_run(&ports, &state.dir)?;
    write_ports(&state, &ports).map_err(|e| format!("writing ports: {e}"))?;
    let log_names = services
        .iter()
        .map(|s| s.name.as_str())
        .chain((!steps.is_empty()).then_some(config::RUN));
    for name in log_names {
        File::create(state.log_path(name)).map_err(|e| format!("{name} log: {e}"))?;
    }

    let echo = !detached;
    let color = echo && io::stdout().is_terminal();
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
        echo,
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

    let run_prefix = match echo {
        true => logs::prefix(config::RUN, width, color.then_some(90)),
        false => String::new(),
    };
    if let Err(e) = run_steps(
        &steps,
        &state,
        &daemon,
        &run_prefix,
        echo,
        &tx,
        &rx,
        &mut lifeline,
    ) {
        lifeline.close();
        daemon.say(&e);
        return Err(e);
    }

    let ctl = state.file("ctl.sock");
    {
        let tx = tx.clone();
        control::serve(&ctl, move |line, reply| {
            let _ = tx.send(Event::Control { line, reply });
        })
        .map_err(|e| format!("control socket {}: {e}", ctl.display()))?;
    }

    let procs = services
        .into_iter()
        .enumerate()
        .map(|(idx, svc)| Proc {
            prefix: match echo {
                true => logs::prefix(
                    &svc.name,
                    width,
                    color.then_some(logs::COLORS[idx % logs::COLORS.len()]),
                ),
                false => String::new(),
            },
            svc,
            pid: None,
            phase: Phase::Exited,
            started: None,
            exit: None,
            pending_start: false,
            waiters: Vec::new(),
        })
        .collect();
    let mut sup = Supervisor {
        state,
        daemon,
        tx,
        procs,
        groups: BTreeSet::new(),
        lifeline,
        shutting: false,
        forced: false,
    };
    for idx in 0..sup.procs.len() {
        sup.start(idx, echo);
    }
    sup.write_status();
    sup.run(rx, echo);
    let _ = fs::remove_file(&ctl);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_steps(
    steps: &[Service],
    state: &State,
    daemon: &Daemon,
    prefix: &str,
    echo: bool,
    tx: &mpsc::Sender<Event>,
    rx: &mpsc::Receiver<Event>,
    lifeline: &mut Lifeline,
) -> Res<()> {
    for (i, step) in steps.iter().enumerate() {
        let label = format!("run[{i}]");
        daemon.say(&format!("{label}: {}", step.cmd.display()));
        let pid = spawn(state, step, prefix, echo, tx, Event::StepExited)
            .map_err(|e| format!("{label} failed to start: {e}"))?;
        lifeline.add(pid);
        let mut interrupted = false;
        let status = loop {
            match rx.recv() {
                Ok(Event::StepExited(status)) => break status,
                Ok(Event::Signal) if !interrupted => {
                    interrupted = true;
                    daemon.say(&format!("stopping {label} (Ctrl-C again to kill)"));
                    stop_group(pid, tx);
                }
                Ok(Event::Signal) => {
                    let _ = sys::killpg(pid, sys::SIGKILL);
                }
                Ok(_) => {}
                Err(_) => return Err("event channel closed".into()),
            }
        };
        // Steps must finish; anything they left running in the background goes too.
        sys::terminate_groups(&[pid], GRACE);
        lifeline.remove(pid);
        match status {
            _ if interrupted => return Err(format!("{label} interrupted")),
            Ok(s) if s.success() => daemon.say(&format!("{label} done")),
            Ok(s) => return Err(format!("{label} failed ({})", describe(s))),
            Err(e) => return Err(format!("{label}: wait failed: {e}")),
        }
    }
    Ok(())
}

impl Supervisor {
    fn run(mut self, rx: mpsc::Receiver<Event>, echo: bool) {
        loop {
            let busy = self
                .procs
                .iter()
                .any(|p| matches!(p.phase, Phase::Running | Phase::Stopping));
            if self.shutting && !busy && self.groups.is_empty() {
                break;
            }
            let ev = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(ev) => ev,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if self.lifeline.check() {
                        self.daemon.say("watchdog died, restarted");
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match ev {
                Event::Signal if !self.shutting => {
                    self.shutting = true;
                    self.daemon.say("stopping (Ctrl-C again to kill)");
                    for idx in 0..self.procs.len() {
                        let p = &mut self.procs[idx];
                        p.pending_start = false;
                        for w in p.waiters.drain(..) {
                            let _ = w.send("err shutting down".into());
                        }
                        self.stop(idx);
                    }
                }
                Event::Signal if !self.forced => {
                    self.forced = true;
                    self.daemon.say("killing");
                    for &g in &self.groups {
                        let _ = sys::killpg(g, sys::SIGKILL);
                    }
                }
                Event::Signal => {}
                Event::Exited { idx, status } => {
                    let p = &mut self.procs[idx];
                    let was_stopping = p.phase == Phase::Stopping;
                    let exit = match status {
                        Ok(s) => describe(s),
                        Err(e) => format!("wait failed: {e}"),
                    };
                    self.daemon.say(&format!("{} exited ({exit})", p.svc.name));
                    p.phase = Phase::Exited;
                    p.exit = Some(exit);
                    // The leader is gone but the group may still have members (backgrounded children).
                    if !was_stopping {
                        stop_group(p.pid.unwrap(), &self.tx);
                    }
                    self.settle(idx, echo);
                }
                Event::StepExited(_) => {}
                Event::GroupGone(g) => {
                    self.groups.remove(&g);
                    self.lifeline.remove(g);
                    if let Some(idx) = self.procs.iter().position(|p| p.pid == Some(g)) {
                        self.settle(idx, echo);
                    }
                }
                Event::Control { line, reply } => self.control(&line, reply, echo),
            }
            self.write_status();
        }
        self.lifeline.close();
        self.daemon.say("stopped");
    }

    fn start(&mut self, idx: usize, echo: bool) {
        let p = &mut self.procs[idx];
        match spawn(
            &self.state,
            &p.svc,
            &p.prefix,
            echo,
            &self.tx,
            move |status| Event::Exited { idx, status },
        ) {
            Ok(pid) => {
                self.lifeline.add(pid);
                self.groups.insert(pid);
                p.pid = Some(pid);
                p.phase = Phase::Running;
                p.started = Some(now());
                p.exit = None;
                self.daemon.say(&format!(
                    "started {} (pid {pid}): {}",
                    p.svc.name,
                    p.svc.cmd.display()
                ));
            }
            Err(e) => {
                p.phase = Phase::Failed;
                p.exit = Some(format!("spawn: {e}"));
                self.daemon
                    .say(&format!("failed to start {}: {e}", p.svc.name));
            }
        }
    }

    fn stop(&mut self, idx: usize) {
        let p = &mut self.procs[idx];
        if p.phase == Phase::Running {
            p.phase = Phase::Stopping;
            stop_group(p.pid.unwrap(), &self.tx);
        }
    }

    /// Acts once the service and every member of its group are gone.
    fn settle(&mut self, idx: usize, echo: bool) {
        let p = &self.procs[idx];
        let gone = matches!(p.phase, Phase::Exited | Phase::Failed)
            && p.pid.is_none_or(|g| !self.groups.contains(&g));
        if !gone {
            return;
        }
        if p.pending_start && !self.shutting {
            self.procs[idx].pending_start = false;
            self.start(idx, echo);
        }
        let p = &mut self.procs[idx];
        let reply = match p.phase {
            Phase::Running => format!("ok {} running (pid {})", p.svc.name, p.pid.unwrap()),
            Phase::Failed => format!(
                "err {} failed to start: {}",
                p.svc.name,
                p.exit.as_deref().unwrap_or("")
            ),
            _ => format!("ok {} stopped", p.svc.name),
        };
        for w in p.waiters.drain(..) {
            let _ = w.send(reply.clone());
        }
    }

    fn control(&mut self, line: &str, reply: mpsc::Sender<String>, echo: bool) {
        let send = |r: String| {
            let _ = reply.send(r);
        };
        if self.shutting {
            return send("err shutting down".into());
        }
        let Some((cmd, name)) = line.split_once(' ') else {
            return send(format!("err invalid request `{line}`"));
        };
        let Some(idx) = self.procs.iter().position(|p| p.svc.name == name) else {
            return send(format!("err unknown service `{name}`"));
        };
        match cmd {
            "stop" => {
                self.procs[idx].pending_start = false;
                self.stop(idx);
            }
            "start" => {
                if self.procs[idx].phase == Phase::Running {
                    return send(format!(
                        "ok {name} already running (pid {})",
                        self.procs[idx].pid.unwrap()
                    ));
                }
                self.procs[idx].pending_start = true;
            }
            "restart" => {
                self.procs[idx].pending_start = true;
                self.stop(idx);
            }
            _ => return send(format!("err unknown command `{cmd}`")),
        }
        self.daemon.say(&format!("{cmd} {name} requested"));
        self.procs[idx].waiters.push(reply);
        self.settle(idx, echo);
    }

    fn write_status(&self) {
        let mut s = format!("{STATUS_HEADER}\n");
        for p in &self.procs {
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
        let _ = state::write_atomic(&self.state.file("status"), s.as_bytes());
    }
}

fn spawn(
    state: &State,
    svc: &Service,
    prefix: &str,
    echo: bool,
    tx: &mpsc::Sender<Event>,
    on_exit: impl FnOnce(io::Result<ExitStatus>) -> Event + Send + 'static,
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
    let log = Arc::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(state.log_path(&svc.name))?,
    );
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
    let echo_prefix = echo.then(|| prefix.to_string());
    {
        let (log, prefix) = (log.clone(), echo_prefix.clone());
        thread::spawn(move || logs::pump(out, log, prefix));
    }
    thread::spawn(move || logs::pump(err, log, echo_prefix));
    let tx = tx.clone();
    thread::spawn(move || {
        let _ = tx.send(on_exit(child.wait()));
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

pub fn detach(config_path: &Path, only: &[String]) -> Res<()> {
    let state = State::new(&config::root_of(config_path));
    state.init()?;
    if let Some(pid) = state.daemon_pid() {
        return Err(format!("already running (pid {pid})"));
    }
    let status = state.file("status");
    let _ = fs::remove_file(&status);
    let out_path = state.file("daemon.out");
    let out = File::create(&out_path).map_err(|e| format!("{}: {e}", out_path.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("-c")
        .arg(config_path)
        .arg("up")
        .arg("--detached-child")
        .args(only)
        .stdin(Stdio::null())
        .stdout(out.try_clone().map_err(|e| e.to_string())?)
        .stderr(out);
    sys::new_session_on_exec(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("spawning daemon: {e}"))?;
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            let out = fs::read_to_string(&out_path).unwrap_or_default();
            return Err(format!("daemon exited ({st})\n{}", out.trim_end()));
        }
        if status.exists() && state.daemon_pid().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(25));
    }
    print!(
        "{}",
        fs::read_to_string(state.file("ports.env")).unwrap_or_default()
    );
    println!("ads running (pid {})", child.id());
    Ok(())
}

pub fn ctl(state: &State, cmd: &str, services: &[String]) -> Res<()> {
    if services.is_empty() {
        return Err(format!("usage: ads {cmd} <svc>..."));
    }
    if state.daemon_pid().is_none() {
        return Err("not running".into());
    }
    let mut failed = false;
    for svc in services {
        let reply = control::request(&state.file("ctl.sock"), &format!("{cmd} {svc}"))?;
        match reply.strip_prefix("err ") {
            Some(e) => {
                failed = true;
                eprintln!("ads: {e}");
            }
            None => println!("{}", reply.strip_prefix("ok ").unwrap_or(&reply)),
        }
    }
    match failed {
        true => Err(format!("{cmd} failed")),
        false => Ok(()),
    }
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

pub fn port_cmd(state: &State, name: &str) -> Res<()> {
    if state.daemon_pid().is_none() {
        return Err("not running".into());
    }
    let text =
        fs::read_to_string(state.file("ports.env")).map_err(|e| format!("ports.env: {e}"))?;
    let key = ports::env_name(name);
    let port = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
        .ok_or_else(|| format!("unknown port name `{name}`"))?;
    println!("{port}");
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
    let steps = cfg.render_run(&ports, &state.dir)?;
    let urls = crate::open::render(
        &cfg,
        &state,
        &ports,
        cfg.open.iter().map(|(n, t)| (n.as_str(), t)).collect(),
    )?;
    println!("config: {}", config_path.display());
    if !ports.is_empty() {
        println!("ports:");
        for (n, p) in &ports {
            println!("  {n} = {p}");
        }
    }
    if !steps.is_empty() {
        println!("run:");
        for (i, s) in steps.iter().enumerate() {
            println!("  [{i}] {}", s.cmd.display());
            if s.cwd != cfg.root {
                println!("      cwd: {}", s.cwd.display());
            }
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
    if !urls.is_empty() {
        println!("open:");
        for ((name, _), url) in cfg.open.iter().zip(urls) {
            println!("  {name} = {url}");
        }
    }
    Ok(())
}
