use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, ServiceExt, tool};
use std::path::PathBuf;

use crate::ipc;

#[derive(Clone)]
pub struct AdsChannel {
    socket_path: PathBuf,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl AdsChannel {
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// The pattern to search for in log files
    pub pattern: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct TraceParams {
    /// The trace ID to look up
    pub trace_id: String,
}

#[rmcp::tool_router]
impl AdsChannel {
    #[tool(description = "Get the status of all managed processes")]
    async fn ads_status(&self) -> String {
        match ipc::send_command(&self.socket_path, "STATUS").await {
            Ok(r) => r,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get the log directory path")]
    async fn ads_logs(&self) -> String {
        match ipc::send_command(&self.socket_path, "LOGS").await {
            Ok(r) => r,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Search across all log files for a pattern")]
    async fn ads_search_logs(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> String {
        let cmd = format!("SEARCH {}", params.pattern);
        match ipc::send_command(&self.socket_path, &cmd).await {
            Ok(r) if r.is_empty() => "No matches found".to_string(),
            Ok(r) => r,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "List recent traces collected from managed processes")]
    async fn ads_traces(&self) -> String {
        match ipc::send_command(&self.socket_path, "TRACES").await {
            Ok(r) if r.is_empty() => "No traces collected".to_string(),
            Ok(r) => r,
            Err(e) => format!("Error: {e}"),
        }
    }

    #[tool(description = "Get full span details for a specific trace by ID")]
    async fn ads_trace(
        &self,
        Parameters(params): Parameters<TraceParams>,
    ) -> String {
        let cmd = format!("TRACE {}", params.trace_id);
        match ipc::send_command(&self.socket_path, &cmd).await {
            Ok(r) if r.is_empty() => "Trace not found".to_string(),
            Ok(r) => r,
            Err(e) => format!("Error: {e}"),
        }
    }
}

#[rmcp::tool_handler]
impl ServerHandler for AdsChannel {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

pub async fn run(socket_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let server = AdsChannel::new(socket_path);
    let transport = rmcp::transport::stdio();
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
