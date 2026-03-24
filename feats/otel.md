# OpenTelemetry Trace Collection (otel)

Receives OTLP traces from managed processes, stores them in memory, and exposes them via IPC, MCP, and CLI.

## How It Works

An OTLP/HTTP receiver starts on an ephemeral port when `ads start` runs. `OTEL_EXPORTER_OTLP_ENDPOINT` and `OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf` are injected into all managed process environments automatically — no per-process config needed.

## Trace Store

- **In-memory**, keyed by trace ID with an LRU eviction policy (max 100 traces)
- Stores decoded span data: trace/span IDs, name, timing, status, service name, attributes
- Merges spans arriving in separate batches into existing traces

## Implementation

- **OTLP receiver** (`src/otel.rs`): axum `POST /v1/traces` endpoint, decodes `ExportTraceServiceRequest` protobuf, stores spans in `TraceStore`
- **TraceStore** (`src/otel.rs`): `Arc<Mutex<...>>` with `HashMap<TraceId, TraceData>` + `VecDeque` for insertion order
- **Env injection** (`src/main.rs`): `cmd_start` starts the OTel server, injects endpoint env vars into all spawned processes

## IPC Commands

- `TRACES` — list recent traces (ID, service, root span, span count, duration)
- `TRACE <id>` — full span details for a trace

## MCP Tools

- **ads_traces** — list recent traces
- **ads_trace(trace_id)** — get trace detail

## CLI

- `ads traces` — list recent traces via IPC
- `ads trace <id>` — show trace details via IPC

## Dependencies

- `opentelemetry-proto` — official OTLP protobuf types
- `prost` — protobuf decoding
- `axum` — HTTP server for OTLP endpoint
