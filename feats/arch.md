# Client/Server Architecture (arch)

Unix socket IPC for controlling a running `ads` instance.

## Implementation

- **Socket path** (`src/ipc.rs`): deterministic path derived from config file's canonical path hash, stored in temp dir (`/tmp/ads-{hash}.sock`)
- **Server**: `UnixListener` runs alongside process manager, handles `STATUS` and `STOP` text commands over the socket
- **Client**: `ads stop` / `ads status` connect to the socket, send a command, read the response
- **Command dispatch**: `tokio::select!` loop in main handles both Ctrl-C and IPC commands, `mpsc` channel + `oneshot` reply channels for status queries
