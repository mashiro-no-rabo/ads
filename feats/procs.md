# Process Management (procs)

Core feature — parse a TOML config file, spawn and manage child processes with automatic port allocation.

## Config Format

File: `ads.toml` in project root. Each proc has:
- `cmd` (array) OR `shell` (string) — exactly one required
- `cwd` — optional working directory (relative to config file)
- `env` — optional extra environment variables
- `{{ port.x }}` — minijinja templates for automatic port allocation

## Implementation

- **Config parsing** (`src/config.rs`): serde deserialization, path resolution, port variable collection via `minijinja::undeclared_variables(true)`, ephemeral port allocation (bind :0), template rendering
- **Process spawning** (`src/proc.rs`): `command-group` for process groups with `kill_on_drop(true)`, piped stdout/stderr forwarded with `[name]` prefix, status tracking via `try_wait()`
- **Graceful shutdown**: SIGTERM to process groups, 5s timeout, SIGKILL stragglers

## CLI

- `ads start [--config <path>]` — read config, allocate ports, spawn processes
- `ads stop` — send shutdown via IPC
- `ads status` — show process states via IPC
- `ads --help` — usage info
