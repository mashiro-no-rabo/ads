# MCP Channel (channel)

Stdio MCP server that exposes ads tools to Claude Code via the [channels](https://code.claude.com/docs/en/channels-reference) protocol.

## Usage

```
ads channel [--config <path>]
```

Starts an MCP server on stdio. Connects to a running `ads` instance via IPC socket.

## Tools

- **ads_status** — returns process statuses
- **ads_logs** — returns log directory path
- **ads_search_logs(pattern)** — searches across all log files for a pattern

## Implementation

- **MCP server** (`src/channel.rs`): uses `rmcp` crate with `StdioServerTransport`
- **Tool routing**: `#[rmcp::tool_router]` and `#[rmcp::tool_handler]` macros define tool schemas
- **IPC proxy**: each tool call forwards to the running ads instance via the unix socket IPC
