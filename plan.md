# OpenTelemetry Trace Collection

Receive OTLP traces from managed processes, store in memory, expose via IPC and MCP.

## Design

**OTLP/HTTP receiver** on an auto-allocated ephemeral port. Accept `POST /v1/traces` with `application/x-protobuf` body. Inject `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:{port}` into all managed process environments automatically so they send traces without any config.

**In-memory trace store** keyed by trace ID, with span list per trace. Bounded by max trace count (LRU eviction). Store the decoded `ResourceSpans` proto types directly.

**IPC commands**: `TRACES` (list recent trace IDs with service/operation summary), `TRACE <id>` (get full span tree for a trace ID).

**MCP tools**: `ads_traces()`, `ads_trace(trace_id)` mirroring the IPC commands.

## Dependencies

- `opentelemetry-proto` — official OTLP protobuf types (with `gen-tonic-messages` + `trace` features, for the Rust structs only)
- `prost` — decode protobuf from HTTP body
- `axum` — HTTP server for OTLP endpoint (lightweight, same tokio runtime)

## Steps

- [x] 1. Add dependencies: `opentelemetry-proto`, `prost`, `axum` to Cargo.toml
- [x] 2. Create `src/otel.rs` — OTLP/HTTP receiver + in-memory trace store
  - TraceStore struct (Arc<Mutex<...>>) with LRU-bounded HashMap<TraceId, TraceData>
  - axum router: `POST /v1/traces` decodes `ExportTraceServiceRequest`, stores spans
  - `start_otel_server()` → binds ephemeral port, returns (port, join handle)
  - Query methods: `list_traces()`, `get_trace(id)`
- [x] 3. Wire into `cmd_start` in main.rs
  - Start OTel server before spawning processes
  - Inject `OTEL_EXPORTER_OTLP_ENDPOINT` + `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` into process env
  - Add `TRACES` and `TRACE <id>` IPC command variants
  - Print OTel endpoint on startup
- [x] 4. Add IPC commands in `src/ipc.rs`
  - `TRACES` → list recent traces (id, service, root span name, span count, duration)
  - `TRACE <id>` → full span details for a trace
- [x] 5. Add MCP tools in `src/channel.rs`
  - `ads_traces()` — list recent traces
  - `ads_trace(trace_id)` — get trace detail
- [x] 6. Add `ads traces` and `ads trace <id>` CLI subcommands
- [ ] 7. Update `feats/otel.md` feature doc, clean up `todos/otel.md`
