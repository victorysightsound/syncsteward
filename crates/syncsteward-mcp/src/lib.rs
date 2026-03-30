use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use syncsteward_core::{
    ActionTarget, AddManagedTargetReport, AlertReport, ConfigScaffoldReport, ControlReport,
    EnsureTargetIdsReport, LogAcknowledgeReport, NotifyAlertsReport, PolicyMode, PreflightReport,
    RelocateManagedTargetReport, StatusReport, SyncTargetInventoryReport, TargetCheckReport,
    TargetCheckSetReport, TargetRunReport, acknowledge_latest_log as core_acknowledge_latest_log,
    add_managed_target as core_add_managed_target, alerts as core_alerts,
    check_target as core_check_target, check_targets as core_check_targets,
    ensure_target_ids as core_ensure_target_ids, notify_alerts as core_notify_alerts, pause,
    preflight, relocate_managed_target as core_relocate_managed_target, resume,
    run_target as core_run_target, scaffold_config as core_scaffold_config, status, targets,
};

type McpResult<T> = Result<Json<T>, String>;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct TargetSelectorRequest {
    target: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RunTargetRequest {
    target: String,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct AddManagedTargetRequest {
    name: String,
    local_path: PathBuf,
    remote_path: String,
    #[serde(default = "default_policy_mode_backup_only")]
    mode: PolicyMode,
    rationale: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct RelocateManagedTargetRequest {
    target: String,
    local_path: PathBuf,
    remote_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct DryRunRequest {
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct SyncStewardMcpServer {
    config_path: Option<PathBuf>,
    tool_router: ToolRouter<Self>,
}

fn default_policy_mode_backup_only() -> PolicyMode {
    PolicyMode::BackupOnly
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

    #[tool(
        description = "Explain readiness and blockers for every configured sync target before any selective re-enablement."
    )]
    async fn check_targets(&self) -> McpResult<TargetCheckSetReport> {
        let config_path = self.config_path.clone();
        let report =
            tokio::task::spawn_blocking(move || core_check_targets(config_path.as_deref()))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Explain readiness and blockers for one configured sync target by name or local path."
    )]
    async fn check_target(
        &self,
        Parameters(request): Parameters<TargetSelectorRequest>,
    ) -> McpResult<TargetCheckReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_check_target(config_path.as_deref(), &request.target)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Evaluate active alerts from current preflight state and per-target run history."
    )]
    async fn alerts(&self) -> McpResult<AlertReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || core_alerts(config_path.as_deref()))
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Send a local notification summarizing current SyncSteward alerts. Supports dry-run mode."
    )]
    async fn notify_alerts(
        &self,
        Parameters(request): Parameters<DryRunRequest>,
    ) -> McpResult<NotifyAlertsReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_notify_alerts(config_path.as_deref(), request.dry_run)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Run one approved backup-only target with full preflight and policy gating. Supports dry-run mode for safe validation."
    )]
    async fn run_target(
        &self,
        Parameters(request): Parameters<RunTargetRequest>,
    ) -> McpResult<TargetRunReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_run_target(config_path.as_deref(), &request.target, request.dry_run)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Acknowledge the current latest rclone log as historical baseline state after cleanup, so preflight can distinguish old known incidents from new failures."
    )]
    async fn acknowledge_latest_log(&self) -> McpResult<LogAcknowledgeReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_acknowledge_latest_log(config_path.as_deref())
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Write a SyncSteward config scaffold from the current target inventory and recommended per-folder policies."
    )]
    async fn scaffold_config(&self) -> McpResult<ConfigScaffoldReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_scaffold_config(config_path.as_deref(), false)
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Force-write a SyncSteward config scaffold from the current target inventory and recommended per-folder policies, replacing any existing config file."
    )]
    async fn scaffold_config_force(&self) -> McpResult<ConfigScaffoldReport> {
        let config_path = self.config_path.clone();
        let report =
            tokio::task::spawn_blocking(move || core_scaffold_config(config_path.as_deref(), true))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Assign stable IDs to managed targets in the SyncSteward config file so future relocate/adopt workflows can recognize the same target after root-path moves."
    )]
    async fn ensure_target_ids(&self) -> McpResult<EnsureTargetIdsReport> {
        let config_path = self.config_path.clone();
        let report =
            tokio::task::spawn_blocking(move || core_ensure_target_ids(config_path.as_deref()))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Add a new managed target to the SyncSteward config without hand-editing the file. This assigns a durable target ID immediately."
    )]
    async fn add_managed_target(
        &self,
        Parameters(request): Parameters<AddManagedTargetRequest>,
    ) -> McpResult<AddManagedTargetReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_add_managed_target(
                config_path.as_deref(),
                &request.name,
                &request.local_path,
                &request.remote_path,
                request.mode,
                request.rationale.as_deref(),
            )
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Relocate an existing managed target by ID, name, or local path while preserving its durable target ID and run history."
    )]
    async fn relocate_managed_target(
        &self,
        Parameters(request): Parameters<RelocateManagedTargetRequest>,
    ) -> McpResult<RelocateManagedTargetReport> {
        let config_path = self.config_path.clone();
        let report = tokio::task::spawn_blocking(move || {
            core_relocate_managed_target(
                config_path.as_deref(),
                &request.target,
                &request.local_path,
                request.remote_path.as_deref(),
            )
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
        Ok(Json(report))
    }

    #[tool(
        description = "Pause both the local launch agent and the remote OneDrive service. This is safe to run repeatedly."
    )]
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

    #[tool(
        description = "Resume both the local launch agent and the remote OneDrive service. This stays blocked until preflight succeeds."
    )]
    async fn resume_all(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::All, resume).await
    }

    #[tool(
        description = "Resume only the local launch agent. This stays blocked until preflight succeeds."
    )]
    async fn resume_local(&self) -> McpResult<ControlReport> {
        self.run_control(ActionTarget::Local, resume).await
    }

    #[tool(
        description = "Resume only the remote OneDrive service. This stays blocked until preflight succeeds."
    )]
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
        operation: fn(
            Option<&std::path::Path>,
            ActionTarget,
        ) -> Result<ControlReport, anyhow::Error>,
    ) -> McpResult<ControlReport> {
        let config_path = self.config_path.clone();
        let handle = tokio::task::spawn_blocking(move || operation(config_path.as_deref(), target));
        let report: Result<ControlReport, anyhow::Error> =
            handle.await.map_err(|error| error.to_string())?;
        let report = report.map_err(|error| error.to_string())?;
        Ok(Json(report))
    }
}
