# Plan: Process Management (procs)

The core of `ads` — parse a config file, spawn and manage child processes with automatic port allocation.

## Design Decisions

- **TOML config** (not YAML like mprocs) — idiomatic Rust, blessed.rs recommended, `toml` crate
- **minijinja templates** in config values — enables `{{ port.x }}` patterns for auto port allocation
- **tokio async runtime** — `tokio::process::Command` for spawning, `tokio::net` for future unix socket work
- **command-group** — spawn processes in process groups so we can cleanly kill entire trees
- **No PTY** — we only need stdout/stderr capture (for logs feature later), not interactive terminal emulation
- **No TUI** — this tool is designed for Claude Code (agent) use, not human interactive use

## Config Format

File: `ads.toml` in project root

```toml
[procs.web]
cmd = ["npm", "run", "dev", "--", "--port", "{{ port.web }}"]
cwd = "./frontend"

[procs.api]
cmd = ["cargo", "run", "--", "--port", "{{ port.api }}"]
env = { DATABASE_URL = "postgres://localhost/mydb" }

[procs.db]
shell = "postgres -D ./data -p {{ port.db }}"
```

Each proc has:
- `cmd` (array) OR `shell` (string) — exactly one required
- `cwd` — optional working directory (relative to config file)
- `env` — optional extra environment variables

## Steps

### 1. Project setup — dependencies and config parsing
- [x] Add dependencies to Cargo.toml: `tokio`, `toml`, `serde`, `minijinja`, `command-group`
- [x] Define config structs with serde derives
- [x] Parse `ads.toml`, resolve `cwd` relative to config file location
- [x] Collect `port.*` template variables from minijinja's `undeclared_variables(true)` (nested mode)
- [x] Allocate ports (bind to :0, get assigned port, close) and render templates

### 2. Process spawning and lifecycle
- [x] Spawn each process using `command-group` (process group) with `kill_on_drop(true)`
- [x] Capture stdout/stderr via piped handles, forward with `[name]` prefix
- [x] Track process state via `try_wait()`: `ProcStatus::Running(pid) | Exited(code) | Failed(err)`
- [x] Graceful shutdown: SIGTERM to process group, wait 5s, then SIGKILL stragglers

### 3. CLI interface
- [x] Use `pico-args` for CLI parsing (per project rules)
- [x] `ads start` — read config, allocate ports, spawn all processes, print port assignments
- [x] `ads start --config <path>` / `-c <path>` flag for custom config file
- [x] `ads --help` / `-h` for usage info
- [x] `ads stop` — connect to running instance, send shutdown signal via unix socket IPC
- [x] `ads status` — show process states via unix socket IPC

## Done

All steps complete. The IPC mechanism uses a unix socket (path derived from config file hash in temp dir). The `start` command runs an IPC server alongside the process manager, handling `STATUS` and `STOP` commands. Client commands connect, send a text command, and read the response.
