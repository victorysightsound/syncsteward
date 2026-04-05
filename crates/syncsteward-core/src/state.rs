use crate::config::{PolicyMode, VerificationMode, expand_path};
use crate::model::{
    AcknowledgedLogSummary, ActionOutcome, FailureClass, LogSummary, TargetOperationKind,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub acknowledged_log: Option<AcknowledgedLogSummary>,
    #[serde(default)]
    pub runner: RunnerState,
    #[serde(default)]
    pub alert_notifications: AlertNotificationState,
    #[serde(default)]
    pub active_target_operation: Option<ActiveTargetOperationState>,
    #[serde(default)]
    pub target_runs: BTreeMap<String, TargetRunState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertNotificationState {
    #[serde(default)]
    pub active_signature: Option<String>,
    #[serde(default)]
    pub active_since_unix_ms: Option<u128>,
    #[serde(default)]
    pub last_notified_signature: Option<String>,
    #[serde(default)]
    pub last_notified_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub repeat_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunnerState {
    #[serde(default)]
    pub last_live_cycle_finished_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub active_cycle: Option<RunnerActiveCycleState>,
    #[serde(default)]
    pub last_cycle: Option<RunnerCycleState>,
    #[serde(default)]
    pub last_tick: Option<RunnerTickState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerActiveCycleState {
    pub dry_run: bool,
    pub started_at_unix_ms: u128,
    #[serde(default)]
    pub process_id: Option<u32>,
    #[serde(default)]
    pub current_target_selector: Option<String>,
    #[serde(default)]
    pub current_target_name: Option<String>,
    #[serde(default)]
    pub current_target_started_at_unix_ms: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveTargetOperationState {
    pub kind: TargetOperationKind,
    pub dry_run: bool,
    pub started_at_unix_ms: u128,
    #[serde(default)]
    pub process_id: Option<u32>,
    pub selector: String,
    pub target_name: String,
    #[serde(default)]
    pub target_id: Option<String>,
    pub local_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRunState {
    pub target_name: String,
    #[serde(default)]
    pub target_id: Option<String>,
    pub local_path: PathBuf,
    pub effective_mode: PolicyMode,
    pub outcome: ActionOutcome,
    pub dry_run: bool,
    pub finished_at_unix_ms: u128,
    pub last_success_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub last_verified_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub last_full_verified_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub last_verification_mode: Option<VerificationMode>,
    #[serde(default)]
    pub last_failure_class: Option<FailureClass>,
    #[serde(default)]
    pub last_repair_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub last_rebaseline_at_unix_ms: Option<u128>,
    #[serde(default)]
    pub consecutive_failure_count: u32,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerCycleState {
    pub dry_run: bool,
    pub started_at_unix_ms: u128,
    pub finished_at_unix_ms: u128,
    pub outcome: ActionOutcome,
    pub approved_target_count: usize,
    pub active_alert_count: usize,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerTickState {
    pub dry_run: bool,
    pub finished_at_unix_ms: u128,
    pub due: bool,
    pub outcome: ActionOutcome,
    pub next_due_at_unix_ms: Option<u128>,
    pub summary: String,
}

pub fn load_state(path: &Path) -> Result<AppState> {
    let path = resolved_path(path);
    if !path.exists() {
        return Ok(AppState::default());
    }

    let raw = fs::read_to_string(&path)?;
    let state = serde_json::from_str(&raw)?;
    Ok(state)
}

pub fn save_acknowledged_log(path: &Path, log: &LogSummary) -> Result<AcknowledgedLogSummary> {
    let path = resolved_path(path);
    let acknowledged = AcknowledgedLogSummary {
        path: log.path.clone(),
        warning_count: log.warning_count,
        error_count: log.error_count,
        out_of_sync_count: log.out_of_sync_count,
        last_started_line: log.last_started_line.clone(),
        last_completed_line: log.last_completed_line.clone(),
        acknowledged_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };

    let mut state = load_state(&path)?;
    state.acknowledged_log = Some(acknowledged.clone());
    write_state(&path, &state)?;

    Ok(acknowledged)
}

pub fn save_target_run(path: &Path, state_key: &str, run: TargetRunState) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    let mut run = run;
    if let Some(existing) = state.target_runs.get(state_key) {
        if run.last_success_at_unix_ms.is_none() {
            run.last_success_at_unix_ms = existing.last_success_at_unix_ms;
        }
        if run.last_verified_at_unix_ms.is_none() {
            run.last_verified_at_unix_ms = existing.last_verified_at_unix_ms;
        }
        if run.last_full_verified_at_unix_ms.is_none() {
            run.last_full_verified_at_unix_ms = existing.last_full_verified_at_unix_ms;
        }
        if run.last_verification_mode.is_none() {
            run.last_verification_mode = existing.last_verification_mode;
        }
        if run.last_repair_at_unix_ms.is_none() {
            run.last_repair_at_unix_ms = existing.last_repair_at_unix_ms;
        }
        if run.last_rebaseline_at_unix_ms.is_none() {
            run.last_rebaseline_at_unix_ms = existing.last_rebaseline_at_unix_ms;
        }
    } else if let Some(existing) = state.target_runs.get(&run.target_name) {
        if run.last_success_at_unix_ms.is_none() {
            run.last_success_at_unix_ms = existing.last_success_at_unix_ms;
        }
        if run.last_verified_at_unix_ms.is_none() {
            run.last_verified_at_unix_ms = existing.last_verified_at_unix_ms;
        }
        if run.last_full_verified_at_unix_ms.is_none() {
            run.last_full_verified_at_unix_ms = existing.last_full_verified_at_unix_ms;
        }
        if run.last_verification_mode.is_none() {
            run.last_verification_mode = existing.last_verification_mode;
        }
        if run.last_repair_at_unix_ms.is_none() {
            run.last_repair_at_unix_ms = existing.last_repair_at_unix_ms;
        }
        if run.last_rebaseline_at_unix_ms.is_none() {
            run.last_rebaseline_at_unix_ms = existing.last_rebaseline_at_unix_ms;
        }
    }
    state.target_runs.insert(state_key.to_string(), run);
    write_state(&path, &state)?;
    Ok(())
}

pub fn prune_target_runs(
    path: &Path,
    keep_keys: &std::collections::BTreeSet<String>,
) -> Result<Vec<String>> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    let removed_keys = state
        .target_runs
        .keys()
        .filter(|key| !keep_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();

    if removed_keys.is_empty() {
        return Ok(removed_keys);
    }

    for key in &removed_keys {
        state.target_runs.remove(key);
    }

    write_state(&path, &state)?;
    Ok(removed_keys)
}

pub fn save_runner_cycle(
    path: &Path,
    cycle: RunnerCycleState,
    live_finished_at_unix_ms: Option<u128>,
) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    state.runner.last_cycle = Some(cycle);
    if live_finished_at_unix_ms.is_some() {
        state.runner.last_live_cycle_finished_at_unix_ms = live_finished_at_unix_ms;
    }
    write_state(&path, &state)?;
    Ok(())
}

pub fn save_runner_active_cycle(path: &Path, cycle: Option<RunnerActiveCycleState>) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    state.runner.active_cycle = cycle;
    write_state(&path, &state)?;
    Ok(())
}

pub fn save_active_target_operation(
    path: &Path,
    operation: Option<ActiveTargetOperationState>,
) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    state.active_target_operation = operation;
    write_state(&path, &state)?;
    Ok(())
}

pub fn save_runner_tick(path: &Path, tick: RunnerTickState) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    state.runner.last_tick = Some(tick);
    write_state(&path, &state)?;
    Ok(())
}

pub fn save_alert_notification_state(
    path: &Path,
    alert_state: AlertNotificationState,
) -> Result<()> {
    let path = resolved_path(path);
    let mut state = load_state(&path)?;
    state.alert_notifications = alert_state;
    write_state(&path, &state)?;
    Ok(())
}

pub fn matches_acknowledged_log(
    acknowledged: Option<&AcknowledgedLogSummary>,
    latest: &LogSummary,
) -> bool {
    let Some(acknowledged) = acknowledged else {
        return false;
    };

    acknowledged.path == latest.path
        && acknowledged.warning_count == latest.warning_count
        && acknowledged.error_count == latest.error_count
        && acknowledged.out_of_sync_count == latest.out_of_sync_count
        && acknowledged.last_started_line == latest.last_started_line
        && acknowledged.last_completed_line == latest.last_completed_line
}

fn resolved_path(path: &Path) -> PathBuf {
    expand_path(path)
}

fn write_state(path: &Path, state: &AppState) -> Result<()> {
    let encoded = serde_json::to_string_pretty(state)?;
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "state path must have a parent directory: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow::anyhow!("state path must have a file name"))?;
    let temp_path = parent.join(format!(".{file_name}.tmp-{}", Uuid::now_v7()));

    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}
