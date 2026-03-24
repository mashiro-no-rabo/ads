use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

const MAX_TRACES: usize = 100;

#[derive(Debug, Clone)]
pub struct SpanData {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
    pub start_time_unix_nano: u64,
    pub end_time_unix_nano: u64,
    pub status_code: i32,
    pub service_name: String,
    pub attributes: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct TraceData {
    pub trace_id: String,
    pub spans: Vec<SpanData>,
    pub service_name: String,
    pub root_span_name: String,
}

pub struct TraceStore {
    traces: HashMap<String, TraceData>,
    order: VecDeque<String>,
    max_traces: usize,
}

impl TraceStore {
    pub fn new() -> Self {
        Self {
            traces: HashMap::new(),
            order: VecDeque::new(),
            max_traces: MAX_TRACES,
        }
    }

    pub fn insert_spans(&mut self, trace_id: &str, service_name: &str, new_spans: Vec<SpanData>) {
        if let Some(existing) = self.traces.get_mut(trace_id) {
            existing.spans.extend(new_spans);
            // Update root span info if found
            if let Some(root) = existing.spans.iter().find(|s| s.parent_span_id.is_empty()) {
                existing.root_span_name = root.name.clone();
                existing.service_name = root.service_name.clone();
            }
        } else {
            // Evict oldest if at capacity
            while self.order.len() >= self.max_traces {
                if let Some(old_id) = self.order.pop_front() {
                    self.traces.remove(&old_id);
                }
            }
            let root_span_name = new_spans
                .iter()
                .find(|s| s.parent_span_id.is_empty())
                .map(|s| s.name.clone())
                .unwrap_or_default();
            self.order.push_back(trace_id.to_string());
            self.traces.insert(
                trace_id.to_string(),
                TraceData {
                    trace_id: trace_id.to_string(),
                    spans: new_spans,
                    service_name: service_name.to_string(),
                    root_span_name,
                },
            );
        }
    }

    pub fn list_traces(&self) -> Vec<TraceSummary> {
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.traces.get(id))
            .map(|t| {
                let min_start = t
                    .spans
                    .iter()
                    .map(|s| s.start_time_unix_nano)
                    .min()
                    .unwrap_or(0);
                let max_end = t
                    .spans
                    .iter()
                    .map(|s| s.end_time_unix_nano)
                    .max()
                    .unwrap_or(0);
                TraceSummary {
                    trace_id: t.trace_id.clone(),
                    service_name: t.service_name.clone(),
                    root_span_name: t.root_span_name.clone(),
                    span_count: t.spans.len(),
                    duration_ms: max_end.saturating_sub(min_start) as f64 / 1_000_000.0,
                }
            })
            .collect()
    }

    pub fn get_trace(&self, trace_id: &str) -> Option<&TraceData> {
        self.traces.get(trace_id)
    }
}

pub struct TraceSummary {
    pub trace_id: String,
    pub service_name: String,
    pub root_span_name: String,
    pub span_count: usize,
    pub duration_ms: f64,
}

pub type SharedTraceStore = Arc<Mutex<TraceStore>>;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn extract_string_value(
    value: &opentelemetry_proto::tonic::common::v1::any_value::Value,
) -> String {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    match value {
        Value::StringValue(s) => s.clone(),
        Value::IntValue(i) => i.to_string(),
        Value::DoubleValue(d) => d.to_string(),
        Value::BoolValue(b) => b.to_string(),
        other => format!("{other:?}"),
    }
}

async fn handle_traces(State(store): State<SharedTraceStore>, body: Bytes) -> StatusCode {
    let request = match ExportTraceServiceRequest::decode(body) {
        Ok(r) => r,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    let mut store = store.lock().unwrap();

    for resource_spans in request.resource_spans {
        let service_name = resource_spans
            .resource
            .as_ref()
            .and_then(|r| r.attributes.iter().find(|a| a.key == "service.name"))
            .and_then(|a| a.value.as_ref())
            .and_then(|v| v.value.as_ref())
            .map(extract_string_value)
            .unwrap_or_default();

        for scope_spans in resource_spans.scope_spans {
            for span in scope_spans.spans {
                let trace_id = hex(&span.trace_id);
                let parent_span_id = if span.parent_span_id.is_empty() {
                    String::new()
                } else {
                    hex(&span.parent_span_id)
                };
                let span_data = SpanData {
                    trace_id: trace_id.clone(),
                    span_id: hex(&span.span_id),
                    parent_span_id,
                    name: span.name,
                    start_time_unix_nano: span.start_time_unix_nano,
                    end_time_unix_nano: span.end_time_unix_nano,
                    status_code: span.status.map_or(0, |s| s.code),
                    service_name: service_name.clone(),
                    attributes: span
                        .attributes
                        .iter()
                        .filter_map(|a| {
                            a.value.as_ref().and_then(|v| {
                                v.value
                                    .as_ref()
                                    .map(|val| (a.key.clone(), extract_string_value(val)))
                            })
                        })
                        .collect(),
                };
                store.insert_spans(&trace_id, &service_name, vec![span_data]);
            }
        }
    }

    StatusCode::OK
}

/// Start the OTLP/HTTP receiver on an ephemeral port.
/// Returns (port, trace_store, join_handle).
pub async fn start_otel_server()
-> Result<(u16, SharedTraceStore, tokio::task::JoinHandle<()>), Box<dyn std::error::Error + Send + Sync>>
{
    let store = Arc::new(Mutex::new(TraceStore::new()));

    let app = Router::new()
        .route("/v1/traces", post(handle_traces))
        .with_state(Arc::clone(&store));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    Ok((port, store, handle))
}
