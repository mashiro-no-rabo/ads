# Plan: Log Capture (logs)

Capture managed process stdout and stderr to log files, and provide commands to locate and search them.

## Design Decisions

- **One log file per process** — `{name}.log` in a run-specific directory, interleaving stdout and stderr with timestamps
- **Log directory** — `$TMPDIR/ads-{config_hash}/logs/` (same hash as socket path, keeps everything grouped)
- **Format** — `{timestamp} {stream} {line}` where stream is `OUT` or `ERR`, timestamp is RFC 3339
- **Rotation** — not needed for MVP; logs are per-run (fresh directory each start)
- **Search** — simple substring/regex search across log files, reporting `{proc}:{line_number}: {content}`

## Steps

### 1. Log file setup and writing
- [x] Create log directory alongside socket path derivation (reuse config hash)
- [x] In `ProcManager::spawn_all`, open a log file per process
- [x] Modify stdout/stderr forwarding tasks to write each line to the log file with timestamp and stream tag
- [x] Continue forwarding to terminal as before (dual output)

### 2. CLI commands for log access
- [x] `ads logs` — print log directory path (so Claude/user can find logs)
- [x] `ads logs <name>` — print log file path for a specific process
- [x] `ads logs --search <pattern>` / `-s <pattern>` — search across all log files, print matching lines with process name and line number

### 3. IPC support for log queries
- [ ] Add `LOGS` IPC command — returns log directory path
- [ ] Add `SEARCH <pattern>` IPC command — searches log files and returns matches
- [ ] Wire up new IPC commands in main select loop
