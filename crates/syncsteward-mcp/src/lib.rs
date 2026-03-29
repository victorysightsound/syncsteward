use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use std::path::PathBuf;
use syncsteward_core::{PreflightReport, StatusReport, preflight, status};

type McpResult<T> = Result<Json<T>, String>;

#[derive(Debug, Clone)]
pub struct SyncStewardMcpServer {
    config_path: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
}

impl SyncStewardMcpServer {
    pub fn new(config_path: Option<PathBuf>) -> Self {
        Self {
            config_path,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SyncStewardMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Use SyncSteward tools to inspect sync health and guarded preflight state before re-enabling automation.",
        )
    }
}

#[tool_router(router = tool_router)]
impl SyncStewardMcpServer {
    #[tool(
        description = "Read the current sync health snapshot, including local and remote writer state, drift artifacts, and the latest rclone log summary."
    )]
    async fn status(&self) -> McpResult<StatusReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || status(config_path.as_deref()))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Run guarded preflight checks before re-enabling or manually executing sync."
    )]
    async fn preflight(&self) -> McpResult<PreflightReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || preflight(config_path.as_deref()))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }
}

pub fn serve_stdio_blocking(config_path: Option<PathBuf>) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;

    runtime.block_on(async move {
        let service = SyncStewardMcpServer::new(config_path);
        let transport = rmcp::transport::stdio();
        service
            .serve(transport)
            .await
            .map_err(|error| error.to_string())?
            .waiting()
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    })
}
