use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::router::tool::ToolRouter,
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use std::path::PathBuf;
use syncsteward_core::{
    ActionTarget, ControlReport, PreflightReport, StatusReport, SyncTargetInventoryReport, pause,
    preflight, resume, status, targets,
};

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

    #[tool(
        description = "Read the current legacy sync targets from cloud-sync.sh and show the safer recommended SyncSteward policy for each one."
    )]
    async fn targets(&self) -> McpResult<SyncTargetInventoryReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || targets(config_path.as_deref()))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(description = "Pause both the local launch agent and the remote OneDrive service. This is safe to run repeatedly.")]
    async fn pause_all(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::All, pause).await
    }

    #[tool(description = "Pause only the local launch agent.")]
    async fn pause_local(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::Local, pause).await
    }

    #[tool(description = "Pause only the remote OneDrive service.")]
    async fn pause_remote(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::Remote, pause).await
    }

    #[tool(description = "Resume both the local launch agent and the remote OneDrive service. This stays blocked until preflight succeeds.")]
    async fn resume_all(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::All, resume).await
    }

    #[tool(description = "Resume only the local launch agent. This stays blocked until preflight succeeds.")]
    async fn resume_local(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::Local, resume).await
    }

    #[tool(description = "Resume only the remote OneDrive service. This stays blocked until preflight succeeds.")]
    async fn resume_remote(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::Remote, resume).await
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

impl SyncStewardMcpServer {
    async fn run_control(
        &self,
        target: ActionTarget,
        operation: fn(Option<&std::path::Path>, ActionTarget) -> Result<ControlReport, anyhow::Error>,
    ) -> McpResult<ControlReport> {
        let config_path = self.config_path.clone();
        let handle = tokio::task::spawn_blocking(move || operation(config_path.as_deref(), target));
        let report: Result<ControlReport, anyhow::Error> = handle
            .await
            .map_err(|error| error.to_string())?
            ;
        let report = report.map_err(|error| error.to_string())?;
        Ok(Json(report))
    }
}
