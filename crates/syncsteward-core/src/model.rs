use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatusReport {
    pub config_source: String,
    pub launch_agent: LaunchAgentStatus,
    pub remote: RemoteStatus,
    pub artifacts: ArtifactReport,
    pub latest_log: Option<LogSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LaunchAgentStatus {
    pub label: String,
    pub loaded: bool,
    pub running: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemoteStatus {
    pub selected_host: Option<String>,
    pub reachable: bool,
    pub service_state: ServiceState,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Active,
    Inactive,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactReport {
    pub roots_scanned: Vec<PathBuf>,
    pub conflict_count: usize,
    pub conflict_examples: Vec<PathBuf>,
    pub safe_backup_count: usize,
    pub safe_backup_examples: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogSummary {
    pub path: PathBuf,
    pub warning_count: usize,
    pub error_count: usize,
    pub out_of_sync_count: usize,
    pub last_started_line: Option<String>,
    pub last_completed_line: Option<String>,
    pub issue_examples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PreflightReport {
    pub ready: bool,
    pub checks: Vec<PreflightCheck>,
    pub status: StatusReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PreflightCheck {
    pub id: String,
    pub status: CheckStatus,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}
