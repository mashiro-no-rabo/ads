# Plan: MCP Channel for Claude Code

Implements `todos/channel.md` — expose ads as a Claude Code MCP channel over stdio, giving Claude tools to query process status and search logs.

## Context

- ads already has IPC (unix socket) with `STATUS`, `STOP`, `LOGS`, `SEARCH <pattern>` commands
- The channel is a new `ads channel` subcommand that starts a stdio MCP server
- It connects to the running ads instance via IPC and proxies tool calls
- Uses the `rmcp` crate (official Rust MCP SDK)

## Steps

### 1. Add `rmcp` dependency
- Add `rmcp` with `server` and `transport-io` features to Cargo.toml
- Add `serde_json` for JSON handling
- Run `cargo outdated` to verify latest versions

### 2. Create `src/channel.rs` — MCP server implementation
- Define a tool handler struct with the socket path
- Implement MCP tools:
  - `ads_status` — returns process statuses (proxies IPC `STATUS`)
  - `ads_logs` — returns log directory path, optionally for a specific process (proxies IPC `LOGS`)
  - `ads_search_logs(pattern)` — searches log files (proxies IPC `SEARCH <pattern>`)
- Use `rmcp` server macros to define tool schemas
- Connect via `StdioServerTransport`

### 3. Add `channel` subcommand to CLI
- Add `channel` to the CLI usage text and command dispatch in `src/main.rs`
- `ads channel [--config <path>]` — starts the MCP server on stdio
- Resolves config path to derive the IPC socket path

### 4. Build and test
- `cargo build` to verify compilation
- Manually verify tool schemas render correctly

### 5. Update docs
- Update `feats/` with channel feature doc
- Clean up `todos/channel.md` if complete

## Progress

- [x] Step 1: Add dependencies
- [x] Step 2: Implement channel module
- [ ] Step 3: Wire up CLI
- [ ] Step 4: Build and test
- [ ] Step 5: Update docs
