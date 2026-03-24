# Log Capture (logs)

Captures managed process stdout and stderr to log files with timestamps, and provides commands to locate and search them.

## Log Storage

- **One log file per process** — `{name}.log` in a run-specific directory
- **Log directory** — `$TMPDIR/ads-{config_hash}/logs/` (same hash as socket path)
- **Format** — `{RFC3339_timestamp} {OUT|ERR} {line}` — interleaves stdout and stderr with timestamps and stream tags
- **Lifecycle** — fresh directory per run, no rotation needed

## Implementation

- **Log writing** (`src/proc.rs`): stdout/stderr piped and forwarded to both terminal (with `[name]` prefix) and log file (with timestamp + stream tag)
- **Search** (`src/main.rs`): `search_logs()` scans all `.log` files in log dir for substring matches, returns `{proc}:{line_number}: {content}` format
- **IPC** (`src/ipc.rs`): `LOGS` command returns log directory path; `SEARCH <pattern>` command searches log files and returns matches

## CLI

- `ads logs` — print log directory path
- `ads logs <name>` — print log file path for a specific process
- `ads logs -s <pattern>` / `--search <pattern>` — search across all log files

## IPC Commands

- `LOGS` — returns the log directory path
- `SEARCH <pattern>` — searches all log files and returns matching lines
