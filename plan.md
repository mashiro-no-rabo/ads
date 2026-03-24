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
- [ ] Spawn each process using `command-group` (process group)
- [ ] Capture stdout/stderr via piped handles (store for future log streaming)
- [ ] Track process state: Starting, Running(pid), Exited(code), Failed(error)
- [ ] Graceful shutdown: SIGTERM to process group, wait, then SIGKILL after timeout

### 3. CLI interface
- [ ] `ads start` — read config, allocate ports, spawn all processes, print port assignments
- [ ] `ads stop` — connect to running instance, send shutdown signal
- [ ] `ads status` — show process states
- [ ] Use `pico-args` for CLI parsing (per project rules)

## Current Step

**Step 1 complete.** Next: Step 2 — Process spawning and lifecycle.
