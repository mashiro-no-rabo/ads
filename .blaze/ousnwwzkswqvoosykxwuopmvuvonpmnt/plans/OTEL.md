# ads otel: minimal OpenTelemetry sink and query

## Goal

Services started by `ads` send traces, logs, and metrics to a local receiver. An agent can
then dump the received data or filter it from the CLI. Dashboards, aggregation, and long
retention are out of scope.

## Dependencies

The official OTLP data model comes from the OpenTelemetry Rust project. No OTLP structures
are hand-written.

| crate | why | config |
|---|---|---|
| `opentelemetry-proto` 0.33 | official generated OTLP v1 types (`ExportTraceServiceRequest`, `ResourceSpans`, `Span`, `LogRecord`, `Metric`, `AnyValue`, …) and the serde impls for OTLP/JSON | `default-features = false, features = ["gen-tonic-messages", "trace", "logs", "metrics", "with-serde"]` |
| `prost` 0.14 | `Message::decode` for protobuf bodies (the version must match what `opentelemetry-proto` uses) | default |
| `serde_json` 1.x | OTLP/JSON decode and encode through the `with-serde` impls | default |

- No `tonic`, `gen-tonic`, or `full`: gRPC stays out, so there's no HTTP/2 and no async
  runtime.
- The `trace`/`logs`/`metrics` features enable `opentelemetry` and `opentelemetry_sdk` as
  dependencies.
- Before committing to this set, run `cargo tree -e normal` and confirm none of them pulls in
  `tokio` or another async runtime. If one does, ask before continuing. The fallback is to
  vendor `opentelemetry-proto`'s generated `.rs` files, which is still official code.
- Check the JSON encoding against the OTLP/JSON spec (hex trace and span IDs, lowerCamelCase
  field names, 64-bit ints as strings). Use the spec's example payloads in
  `opentelemetry-proto` (`examples/*.json`) as test fixtures.

## Transport

- **OTLP/HTTP** only: `POST /v1/traces`, `/v1/logs`, `/v1/metrics`.
- **Both bodies supported**:
  - `application/x-protobuf` → `prost::Message::decode`
  - `application/json` → `serde_json::from_slice`
- Not supported:
  - gRPC.
  - `Content-Encoding: gzip`, rejected with 415 to avoid adding `flate2`.

ads injects these into every service's env (a service's own `env` can override them):

```
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:{{ports.otel}}
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
OTEL_EXPORTER_OTLP_COMPRESSION=none
OTEL_SERVICE_NAME=<service>
```

Config:

```toml
[otel]
enabled = true          # allocates port "otel" on 127.0.0.1, also referenceable as {{ports.otel}}
max_requests = 10000    # in-memory ring per signal, counted in export requests
```

## HTTP server (`src/otel/http.rs`)

Plain request/response handling stays hand-rolled because it involves no OTLP structures.
Pulling `hyper` would bring async with it.

- A `TcpListener` on `127.0.0.1:{{ports.otel}}` runs in an accept thread, with one thread per
  connection and keep-alive.
- Request parsing: the request line, headers up to 64 KiB, and the body (`Content-Length` or
  chunked) capped at 16 MiB. Anything else gets 400 or 413.
- Success response: an empty `Export*ServiceResponse` (`::default()`), encoded with
  `prost::Message::encode_to_vec` or `serde_json` to match the request's content type.
- Malformed body: 400 with the decode error as text. Nothing is stored.
- The same server answers queries at `GET /q/{traces,logs,metrics}?…`, so the CLI client is a
  plain `TcpStream`.

## Storage (`src/otel/store.rs`)

- **In memory**, per signal, a `Mutex<VecDeque<(seq, recv_time, ExportXServiceRequest)>>`
  bounded by `max_requests`, with the oldest entries dropped first. The decoded official
  request type is stored as-is, with no flattening and no model of our own.
- **On disk**, every request is appended as one OTLP/JSON line to
  `.ads/otel/{traces,logs,metrics}.jsonl`.
  - This matches the line format of the Collector's `file` exporter and `otlpjsonfile`
    receiver, so the dump can be replayed into a real Collector or read with `jq`.
  - Files are truncated on `ads up`, and there's no rotation.
- **Restart**: the ring can be rebuilt by reading the JSONL back with `serde_json`, which is
  essentially free since the format is identical. Do it on `ads up` when
  `--keep-otel` is passed.

## Query

`ads otel <traces|logs|metrics> [filters]` calls `GET /q/<signal>?…` on the running daemon.

**Output** is OTLP/JSON as well: one line per matching request, pruned. Each request is
cloned, and `Vec::retain` drops the non-matching spans, log records, and metric data points,
then removes any `ScopeX`/`ResourceX` left empty. The result is still a valid
`ExportXServiceRequest`, so agents see exactly the official shape. `--flat` prints one
compact text line per span, log, or point for humans.

| filter | traces | logs | metrics | matches on (official fields) |
|---|---|---|---|---|
| `--service S` | ✓ | ✓ | ✓ | `resource.attributes["service.name"]` |
| `--trace ID` | ✓ | ✓ | | `trace_id` (hex prefix) |
| `--name STR` | ✓ | | ✓ | `Span.name` / `Metric.name` (substring) |
| `--grep STR` | ✓ | ✓ | | substring over the `serde_json` rendering of the span or log record |
| `--since 5m` | ✓ | ✓ | ✓ | `start_time_unix_nano` / `time_unix_nano` (with `observed_time_unix_nano` as fallback) |
| `--min-duration 100ms` | ✓ | | | `end − start` |
| `--errors` | ✓ | ✓ | | `status.code == Error` / `severity_number >= Error` |
| `--severity warn` | | ✓ | | `severity_number >=` the named `SeverityNumber` |
| `--attr k=v` | ✓ | ✓ | ✓ | `attributes`, comparing the `AnyValue` rendered as a string. Repeatable |
| `--limit N` | ✓ | ✓ | ✓ | newest N requests after filtering, default 50 |
| `--after SEQ` | ✓ | ✓ | ✓ | for polling. The response header `X-Ads-Last-Seq` carries the latest seq |

Extra commands:
- `ads otel trace <id>` collects every span of one trace across requests, sorted by
  `start_time_unix_nano` and indented by `parent_span_id`. This is the only text view built
  on top.
- `ads otel tail <signal> [filters]` polls with `--after` every 500ms.

The only code we write is a small `AnyValue → String` helper for attribute matching. Query
strings are parsed with a hand-rolled `%XX` decoder.

## Module layout

```
src/otel/mod.rs     service wiring and env injection
src/otel/http.rs    tiny HTTP/1.1 server and client
src/otel/store.rs   rings and jsonl append/reload
src/otel/query.rs   filter parsing, prune via retain, AnyValue helpers
```

## Phases

1. Add the dependencies and audit them with `cargo tree`. Round-trip tests run the
   `opentelemetry-proto` example JSON through serde, then prost encode/decode.
2. HTTP server, store, and JSONL dump, running as threads inside the daemon. The listener
   binds before any service spawns, so no ordering logic is needed.
3. Query endpoint, `ads otel` CLI, filters, and pruning.
4. `trace` tree view, `tail`, and `--keep-otel` reload.
5. End-to-end check: one service in a test config uses an official SDK exporter (for
   example a tiny `uv run` Python script with `opentelemetry-exporter-otlp-proto-http`),
   then the test queries its span back.

## Open questions

- Should metrics be included at all in v1? Agents mostly need traces and logs. Plan to keep
  the feature, since it costs only the `metrics` cargo feature and a filter column.
