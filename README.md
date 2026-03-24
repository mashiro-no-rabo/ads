# agent-dev-stack

Manages a local dev stack for Claude Code sessions: spawns processes, captures logs, collects OpenTelemetry traces, and exposes tools via MCP.

## Quick Start

1. Create an `ads.toml` config in your project root (see [Process Management](feats/procs.md))
2. Run `ads start` to launch your dev stack
3. Use `ads status`, `ads logs`, `ads traces` to inspect

## CLI

```
ads [OPTIONS] [COMMAND]
```

### Global Options

| Option | Description |
|---|---|
| `-c, --config <PATH>` | Config file path (default: `ads.toml`) |
| `-h, --help` | Print help |
| `-V, --version` | Print version |

### Commands

#### `ads start`

Start all processes defined in the config file. This is the default when no command is given.

Allocates ephemeral ports for `{{ port.x }}` templates, starts an OTLP receiver, spawns all processes, and runs an IPC server for other commands to connect to.

#### `ads stop`

Stop a running ads instance via IPC.

#### `ads status`

Show process states (running/exited) via IPC.

#### `ads logs [NAME] [-s PATTERN]`

Show log paths or search logs.

| Argument/Option | Description |
|---|---|
| `<NAME>` | Show log file path for a specific process |
| `-s, --search <PATTERN>` | Search across all log files for a pattern |

Without arguments, prints the log directory path.

#### `ads traces`

List recent OpenTelemetry traces collected from managed processes.

#### `ads trace <TRACE_ID>`

Show detailed span information for a specific trace.

#### `ads channel`

Start an MCP server on stdio for Claude Code integration. Connects to a running `ads` instance via IPC and exposes tools: `ads_status`, `ads_logs`, `ads_search_logs`, `ads_traces`, `ads_trace`.

## Config Format

File: `ads.toml` — each section defines a process:

```toml
[api]
cmd = ["cargo", "run", "--", "--port", "{{ port.api }}"]
cwd = "services/api"
env = { RUST_LOG = "debug" }

[worker]
shell = "node worker.js --port {{ port.worker }}"
```

- `cmd` (array) or `shell` (string) — exactly one required
- `cwd` — optional working directory (relative to config file)
- `env` — optional environment variables
- `{{ port.x }}` — automatically allocated ephemeral ports

## Claude Code Integration

Add ads as a channel in your project's `.claude/settings.json`:

```json
{
  "channels": [
    {
      "command": ["ads", "channel"],
      "name": "ads"
    }
  ]
}
```

This gives Claude Code access to these MCP tools:

- **ads_status** — check which processes are running
- **ads_logs** — get the log directory path
- **ads_search_logs(pattern)** — search across all process logs
- **ads_traces** — list recent OpenTelemetry traces
- **ads_trace(trace_id)** — inspect a specific trace's spans and attributes

## Features

Detailed design docs for each feature:

- [Process Management](feats/procs.md) — config parsing, port allocation, process spawning and shutdown
- [Log Capture](feats/logs.md) — per-process log files with timestamps, search
- [OpenTelemetry Traces](feats/otel.md) — OTLP receiver, trace storage, auto-injection of env vars
- [MCP Channel](feats/channel.md) — stdio MCP server exposing ads tools to Claude Code
- [Client/Server Architecture](feats/arch.md) — Unix socket IPC design
