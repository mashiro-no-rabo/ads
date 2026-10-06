# ads (agent-dev-stack): process supervisor CLI

## Goal

`ads` is one Rust binary that reads `ads.toml`, assigns ports, starts the configured
processes, and guarantees every descendant dies with the daemon. It targets macOS first;
only POSIX APIs are used so Linux should mostly work.

Agents are the main users, so the stack's state (ports, pids, logs) lives in plain files
under `.ads/` that an agent can `cat` without talking to the daemon.

## Constraints

- No async. One thread per blocking concern (signal wait, child wait, log pump).
- Dependencies are limited to what std can't do:

  | crate | why | config |
  |---|---|---|
  | `libc` 0.2.x (1.0 is still alpha) | `killpg`, `kill`, `pthread_sigmask`, `sigwait`, `setsid`, port probe without `SO_REUSEADDR` | default |
  | `toml` 1.x | config parsing | `default-features = false, features = ["std", "parse", "serde"]`: walk `toml::Table` by hand, no derive |
  | `pico-args` 0.5 | flags | zero deps; can be replaced with a hand-rolled `std::env::args` parser if we want 2 deps |

  `toml::Table` and `Value` only exist behind the `serde` feature. That pulls in
  `serde_core` alone, without `serde_derive`. Ask before pulling in anything else.
  Run `cargo outdated`, and keep the build at zero warnings.
- Edition 2024. Add `.gitignore` (`/target`, `.ads/`) and a `justfile` (`build`, `test`, `lint`).
- Out of scope for v1:
  - readiness checks
  - dependency ordering
  - restart policies
  - configurable stop signals or timeouts

## Config: `ads.toml`

```toml
[env]                                # added to every service's environment
RUST_LOG = "info"

[services.db]
cmd = ["postgres", "-D", ".ads/pg", "-p", "{{ports.db}}"]   # array → execvp

[services.api]
cmd = "cargo run -- --port {{ports.api}}"                   # string → `sh -c`
cwd = "backend"
env = { DATABASE_URL = "postgres://127.0.0.1:{{ports.db}}/app" }

[services.web]
cmd = "pnpm dev --port {{ports.web}}"
cwd = "frontend"
env = { API_URL = "http://127.0.0.1:{{ports.api}}" }
```

There's no ports section. A port exists because some template references it: every
distinct `{{ports.<name>}}` anywhere in the config gets one free port, and all references to
the same name resolve to the same port.

### Templating

This is a hand-rolled `{{ … }}` substitution, not a template engine. Whitespace inside the
braces is allowed. Supported references:

- `{{ports.<name>}}`: an auto-assigned port, starting from 8000.
- `{{env.<NAME>}}`: the daemon's environment. Unset is an error, and `{{env.X ? default}}`
  supplies a default.
- `{{root}}`: absolute directory of `ads.toml`.
- `{{state}}`: absolute `.ads/` directory.
- `{{service}}`: current service name.

Substitution applies to `cmd`, `cwd`, and `env` values. Unknown references fail config load
with the key path, so they never reach runtime. Every service also gets
`ADS_PORT_<NAME>=<port>` for all ports, plus `ADS_SERVICE`, `ADS_ROOT` and `ADS_STATE`.

Rendering has two passes:
1. Collect the port names from every template.
2. Allocate the ports, then substitute.

The config is found by walking up from cwd for `ads.toml`, or set with `-c <path>`.

### Port allocation (`src/ports.rs`)

All ports are on `127.0.0.1` only. Services should bind `127.0.0.1` explicitly, because
`localhost` may resolve to `::1`.

ads only probes for free ports. It doesn't hold them: it assumes the gap between the probe
and the service binding is short enough.

- Port names are sorted alphabetically. That gives every name the same port across runs
  whenever those ports are free, which helps agents and bookmarks.
- For each name, scan upward from `8000` and pick the first port that:
  - isn't already assigned to another name in this run, and
  - passes the probe.
- Running out at `8999` is an error that names the range.
- **Probe**: `socket` + `bind(127.0.0.1:P)` + `close` through `libc` in `sys.rs`, **without**
  `SO_REUSEADDR`. std's `TcpListener::bind` sets `SO_REUSEADDR`, and on macOS that lets a
  `127.0.0.1:P` bind succeed while another process listens on `0.0.0.0:P`, which would be a
  false "free". Without the flag, the bind fails on any existing listener for `P` (wildcard
  or `127.0.0.1`) and also on ports still in `TIME_WAIT`. That's conservative, and fine.
- The probe socket never listens, so it's gone immediately after `close`.
- Ports are allocated once per `ads up`. Restarting a service through the control socket
  keeps its port.

If something takes the port between probe and spawn, the service fails with `EADDRINUSE` in
its own log. ads doesn't detect or retry this.

## Process lifetime (core requirement)

There are three layers. Each one covers a failure the previous layer doesn't.

The only signal ads supports is `SIGTERM`, plus Ctrl-C (`SIGINT`) on the daemon itself.
`SIGKILL` exists only as a fixed internal fallback (see below) and isn't configurable.

### 1. Process group per service

Spawn with `std::os::unix::process::CommandExt::process_group(0)`, so each service leads its
own pgid. That pgid includes whatever it forks (`pnpm` → `node` → `esbuild`).

Stopping a service:
1. `killpg(pgid, SIGTERM)`.
2. Wait 5s (a fixed timeout).
3. `killpg(pgid, SIGKILL)`, so a service that ignores SIGTERM can't break the "everything
   dies" guarantee.

Separate groups (rather than one shared group) also mean the terminal's Ctrl-C reaches only
the daemon, which then stops the services itself.

Known escape: a descendant that calls `setsid`/`setpgid` itself leaves the group. That's
rare for dev tools. Document it rather than solving it.

### 2. Graceful daemon shutdown (SIGTERM / Ctrl-C)

- At the top of `main`, before any thread starts, block `SIGTERM` and `SIGINT` with
  `pthread_sigmask`. A dedicated thread then loops on `sigwait` and sends `Event::Shutdown`
  over an `mpsc` channel. This avoids async-signal-safety issues and needs no `signal-hook`
  crate.
- std does **not** reset the child's signal mask: the `signal_mask_not_inherited` test proved
  that children inherited the block and ignored SIGTERM. So every spawn (services and the
  watchdog) resets the mask with `pthread_sigmask(SIG_SETMASK, ∅)` in `pre_exec`. That forces
  the fork/exec path instead of `posix_spawn`, which is fine at this scale.
- Shutdown sends `SIGTERM` to all groups at once and waits for them in parallel.
- A second Ctrl-C or `SIGTERM` during shutdown skips the wait and goes straight to the
  `SIGKILL` fallback.

### 3. Daemon hard death (SIGKILL, panic=abort, crash): watchdog with a lifeline pipe

macOS has no `PR_SET_PDEATHSIG`, so this layer needs its own mechanism:

- At startup the daemon creates a pipe and spawns `ads __watchdog` (hidden subcommand) with
  the pipe's read end as stdin. The watchdog gets its own process group, so Ctrl-C and a
  group kill of a service can't reach it.
- The daemon keeps the write end. Std sets `CLOEXEC`, so services never inherit it.
- After each spawn the daemon writes `+<pgid>\n`. When a service exits it writes `-<pgid>\n`.
- The watchdog blocks on reads. On EOF, which happens however the daemon died, it sends
  `killpg(SIGTERM)` to every live pgid, waits 5s, sends `SIGKILL` to the remaining ones, and
  exits.
- On graceful shutdown the daemon writes `.\n` (clean) before closing, and the watchdog exits
  without signalling anything.
- If the watchdog itself is killed, nothing is lost while the daemon is alive: the daemon
  sees `EPIPE` on its next write and respawns it with the current pgid set.

Rejected options:
- kqueue `EVFILT_PROC NOTE_EXIT`: macOS-only, and it would need per-service shims.
- One shared group for everything: `kill -9 <daemon>` would still leave it alive.
- launchd: too heavy.

## CLI surface

```
ads up [-c ads.toml] [-d] [svc...]   start the daemon (foreground; -d detaches)
ads down                              SIGTERM the daemon from .ads/daemon.pid and wait for it to exit
ads ps                                services, pid, pgid, state, exit code
ads ports [--json]                    resolved ports, `ports.env` format by default (agents use this one)
ads logs [svc] [-f] [-n 100]          read .ads/logs/<svc>.log, -f follows
ads restart <svc>                     restart one service (needs the control socket, phase 4)
ads check                             validate the config and render templates without starting anything
```

- `up` checks `.ads/daemon.pid` first. If that pid is alive (`kill(pid, 0)`), it refuses to
  start. The pid file uses an exclusive `flock` (`File::try_lock`, stable in std), so a stale
  pid file can't wedge startup.
- `-d` re-execs `ads up` with `process_group(0)`, stdio redirected to `.ads/daemon.log`, and
  `setsid` in `pre_exec` so it outlives the terminal. Then it prints the ports and returns.
  The detached daemon keeps all three lifetime layers.

## State dir `.ads/`

```
.ads/daemon.pid       pid, held under flock
.ads/ports.env        ADS_PORT_API=8000\n… (same names services get; written before any service starts)
.ads/ports.json       {"api":8000,…} (written by hand, no serde_json)
.ads/status           TSV with a header: name pid state started exit (pid == pgid; rewritten on every change)
.ads/logs/<svc>.log   raw combined stdout+stderr, truncated on `up`
.ads/daemon.log       daemon events (spawned, exited, ports)
```

All writes go to a temp file and are renamed into place, so readers never see a partial file.

## Runtime architecture

```
main thread: event loop over mpsc::Receiver<Event>
  ├─ signal thread       sigwait(TERM, INT) → Event::Shutdown
  ├─ per service:
  │   ├─ waiter thread   child.wait() → Event::Exited{svc, status}
  │   ├─ stdout pump     BufRead lines → terminal (prefixed, colored) + log file
  │   └─ stderr pump     same
  └─ watchdog writer     the main thread writes +/- pgid directly
```

- `Event` enum: `Shutdown`, `Exited`, plus `Control(..)` in phase 4.
- All services spawn at startup with no ordering.
- When a service exits, its state becomes `exited(code)` and nothing else changes; the
  others keep running.
- Terminal output uses the `name | line` prefix with a stable ANSI color per service, and
  disables color when stdout isn't a TTY (`std::io::IsTerminal`).

## Module layout

```
src/main.rs        arg dispatch
src/config.rs      toml::Table → Config, validation
src/template.rs    {{ }} parse/collect/render
src/ports.rs       probe/allocate
src/supervisor.rs  event loop, spawn, stop
src/sys.rs         every libc call goes here (killpg, sigmask, sigwait, setsid, kill(pid,0), port probe); no unsafe elsewhere
src/watchdog.rs    __watchdog subcommand + lifeline client
src/state.rs       .ads/ files, atomic writes, pid lock
src/logs.rs        pumps, `ads logs` reader/follow
```

## Phases

Status as of 2026-10-06:
- Phases 1–3 are done and covered by `tests/lifecycle.rs`.
- Not done yet: `-d`, `ads restart`, the control socket (phase 4), and OTel (phase 5).

1. **Skeleton + config**: cargo init, `.gitignore`, `justfile`, config parse, template
   collect and render, port allocation, `ads check`, `ads ports`. Unit tests:
   - template errors
   - the same port name resolving to the same port
   - a port held by a test listener on `0.0.0.0` being skipped
   - two names never sharing a port
2. **Supervisor**: process groups, log pumps, signal thread, graceful shutdown, `ps`,
   `logs`, `down`.
3. **Watchdog**: lifeline pipe. Integration test: `up` a config whose service runs
   `sh -c 'sleep 1000 & sleep 1000'`, `kill -9` the daemon, then assert with
   `kill(pgid, 0) == ESRCH` that both sleeps are gone within 10s.
4. **Detach + control**: `up -d`, plus a Unix socket at `.ads/ctl.sock` with a line
   protocol (`restart api`, `stop api`, `start api`, `status`), which enables `ads restart`.
5. **OTel**: see `OTEL.md`.

## Integration test harness

`tests/` uses `std::process::Command` on `env!("CARGO_BIN_EXE_ads")` with temp-dir configs.
`sleep`, `sh`, and `nc -l` serve as the services. Assertions are only on `.ads/` files and
`kill(pid, 0)`, never on terminal output.
