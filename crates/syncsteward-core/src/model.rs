use crate::config::{FileClassPolicy, FolderPolicy};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatusReport {
    pub config_source: String,
    pub policy: PolicySummary,
    pub launch_agent: LaunchAgentStatus,
    pub remote: RemoteStatus,
    pub artifacts: ArtifactReport,
    pub acknowledged_log: Option<AcknowledgedLogSummary>,
    pub latest_log: Option<LogSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncTargetInventoryReport {
    pub config_source: String,
    pub script_path: PathBuf,
    pub targets: Vec<SyncTargetRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetCheckSetReport {
    pub config_source: String,
    pub preflight_ready: bool,
    pub evaluations: Vec<TargetEvaluation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetCheckReport {
    pub config_source: String,
    pub selector: String,
    pub preflight_ready: bool,
    pub evaluation: TargetEvaluation,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetRunReport {
    pub config_source: String,
    pub selector: String,
    pub dry_run: bool,
    pub outcome: ActionOutcome,
    pub summary: String,
    pub preflight_ready: bool,
    pub evaluation: TargetEvaluation,
    pub steps: Vec<ActionStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AlertReport {
    pub config_source: String,
    pub generated_at_unix_ms: u128,
    pub preflight_ready: bool,
    pub stale_success_after_hours: u64,
    pub alerts: Vec<AlertRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NotifyAlertsReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub dry_run: bool,
    pub alerts: Vec<AlertRecord>,
    pub steps: Vec<ActionStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AlertRecord {
    pub id: String,
    pub severity: AlertSeverity,
    pub summary: String,
    pub detail: String,
    pub target_name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyncTargetRecord {
    pub name: String,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub legacy_mode: LegacySyncMode,
    pub recommended_mode: crate::config::PolicyMode,
    pub configured_mode: Option<crate::config::PolicyMode>,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetEvaluation {
    pub target: SyncTargetRecord,
    pub effective_mode: crate::config::PolicyMode,
    pub ready: bool,
    pub blockers: Vec<TargetBlocker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetBlocker {
    pub id: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LegacySyncMode {
    Bisync,
    BackupOneWay,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicySummary {
    pub folder_policies: Vec<FolderPolicy>,
    pub file_class_policies: Vec<FileClassPolicy>,
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
pub struct AcknowledgedLogSummary {
    pub path: PathBuf,
    pub warning_count: usize,
    pub error_count: usize,
    pub out_of_sync_count: usize,
    pub last_started_line: Option<String>,
    pub last_completed_line: Option<String>,
    pub acknowledged_at_unix_ms: u128,
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Pause,
    Resume,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionTarget {
    Local,
    Remote,
    All,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Success,
    NoOp,
    Blocked,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ControlReport {
    pub action: ControlAction,
    pub target: ActionTarget,
    pub outcome: ActionOutcome,
    pub summary: String,
    pub steps: Vec<ActionStep>,
    pub preflight: Option<PreflightReport>,
    pub status: StatusReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActionStep {
    pub id: String,
    pub status: ActionStepStatus,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogAcknowledgeReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub state_path: PathBuf,
    pub acknowledged_log: Option<AcknowledgedLogSummary>,
    pub latest_log: Option<LogSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConfigScaffoldReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub path: PathBuf,
    pub overwritten: bool,
    pub folder_policy_count: usize,
    pub file_class_policy_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionStepStatus {
    Applied,
    Skipped,
    Blocked,
    Failed,
}
