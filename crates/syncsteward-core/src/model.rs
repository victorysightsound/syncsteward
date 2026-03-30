use crate::config::{FileClassPolicy, FolderPolicy, TargetExclusion, TargetSnapshot};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StatusReport {
    pub config_source: String,
    pub policy: PolicySummary,
    pub launch_agent: LaunchAgentStatus,
    pub runner_agent: LaunchAgentStatus,
    pub remote: RemoteStatus,
    pub artifacts: ArtifactReport,
    pub acknowledged_log: Option<AcknowledgedLogSummary>,
    pub latest_log: Option<LogSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OverviewReport {
    pub config_source: String,
    pub generated_at_unix_ms: u128,
    pub preflight_ready: bool,
    pub failing_check_count: usize,
    pub warning_check_count: usize,
    pub active_alert_count: usize,
    pub status: StatusReport,
    pub preflight_checks: Vec<PreflightCheck>,
    pub runner: RunnerOverview,
    pub targets: TargetHealthOverview,
    pub approved_targets: Vec<ApprovedTargetOverview>,
    pub recent_target_runs: Vec<RecentTargetRunSummary>,
    pub alerts: Vec<AlertRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerOverview {
    pub agent: LaunchAgentStatus,
    pub cycle_interval_minutes: u64,
    pub tick_interval_minutes: u64,
    pub due: bool,
    pub last_live_cycle_finished_at_unix_ms: Option<u128>,
    pub next_due_at_unix_ms: Option<u128>,
    pub last_cycle: Option<RunnerCycleSummary>,
    pub last_tick: Option<RunnerTickSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerCycleSummary {
    pub dry_run: bool,
    pub started_at_unix_ms: u128,
    pub finished_at_unix_ms: u128,
    pub outcome: ActionOutcome,
    pub approved_target_count: usize,
    pub active_alert_count: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerTickSummary {
    pub dry_run: bool,
    pub finished_at_unix_ms: u128,
    pub due: bool,
    pub outcome: ActionOutcome,
    pub next_due_at_unix_ms: Option<u128>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetHealthOverview {
    pub total_target_count: usize,
    pub managed_target_count: usize,
    pub approved_target_count: usize,
    pub resolved_approved_target_count: usize,
    pub ready_target_count: usize,
    pub blocked_target_count: usize,
    pub ready_approved_target_count: usize,
    pub live_success_target_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApprovedTargetOverview {
    pub selector: String,
    pub resolved: bool,
    pub detail: String,
    pub evaluation: Option<TargetEvaluation>,
    pub last_run: Option<RecentTargetRunSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecentTargetRunSummary {
    pub target_name: String,
    pub target_id: Option<String>,
    pub local_path: PathBuf,
    pub effective_mode: crate::config::PolicyMode,
    pub outcome: ActionOutcome,
    pub finished_at_unix_ms: u128,
    pub last_success_at_unix_ms: Option<u128>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerAgentStatusReport {
    pub config_source: String,
    pub status: LaunchAgentStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerAgentControlReport {
    pub config_source: String,
    pub action: RunnerAgentAction,
    pub outcome: ActionOutcome,
    pub summary: String,
    pub status: LaunchAgentStatus,
    pub steps: Vec<ActionStep>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RunnerAgentAction {
    Install,
    Uninstall,
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
pub struct AddManagedTargetReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub path: PathBuf,
    pub target: SyncTargetRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RelocateManagedTargetReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub path: PathBuf,
    pub selector: String,
    pub previous_local_path: PathBuf,
    pub previous_remote_path: String,
    pub target: SyncTargetRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunCycleReport {
    pub config_source: String,
    pub dry_run: bool,
    pub outcome: ActionOutcome,
    pub summary: String,
    pub preflight_ready: bool,
    pub approved_target_count: usize,
    pub target_runs: Vec<TargetRunReport>,
    pub skipped_targets: Vec<CycleSkippedTarget>,
    pub alerts: Vec<AlertRecord>,
    pub notification: Option<NotifyAlertsReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CycleSkippedTarget {
    pub selector: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerTickReport {
    pub config_source: String,
    pub dry_run: bool,
    pub outcome: ActionOutcome,
    pub summary: String,
    pub due: bool,
    pub cycle_interval_minutes: u64,
    pub last_live_cycle_finished_at_unix_ms: Option<u128>,
    pub next_due_at_unix_ms: Option<u128>,
    pub preflight_ready: bool,
    pub cycle: Option<RunCycleReport>,
    pub alerts: Vec<AlertRecord>,
    pub notification: Option<NotifyAlertsReport>,
    pub steps: Vec<ActionStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AlertReport {
    pub config_source: String,
    pub generated_at_unix_ms: u128,
    pub preflight_ready: bool,
    pub stale_success_after_hours: u64,
    pub repeat_notification_after_minutes: u64,
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
    pub target_id: Option<String>,
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
    Managed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicySummary {
    pub folder_policies: Vec<FolderPolicy>,
    pub file_class_policies: Vec<FileClassPolicy>,
    pub target_exclusions: Vec<TargetExclusion>,
    pub target_snapshots: Vec<TargetSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LaunchAgentStatus {
    pub label: String,
    pub plist_path: Option<PathBuf>,
    pub installed: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnsureTargetIdsReport {
    pub outcome: ActionOutcome,
    pub summary: String,
    pub path: PathBuf,
    pub assigned_count: usize,
    pub preserved_count: usize,
    pub assignments: Vec<ManagedTargetIdAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManagedTargetIdAssignment {
    pub target_name: String,
    pub target_id: String,
    pub reason: ManagedTargetIdAssignmentReason,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ManagedTargetIdAssignmentReason {
    Missing,
    Duplicate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActionStepStatus {
    Applied,
    Skipped,
    Blocked,
    Failed,
}
