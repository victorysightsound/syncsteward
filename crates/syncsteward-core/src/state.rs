use crate::model::{AcknowledgedLogSummary, LogSummary};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub acknowledged_log: Option<AcknowledgedLogSummary>,
}

pub fn load_state(path: &Path) -> Result<AppState> {
    if !path.exists() {
        return Ok(AppState::default());
    }

    let raw = fs::read_to_string(path)?;
    let state = serde_json::from_str(&raw)?;
    Ok(state)
}

pub fn save_acknowledged_log(path: &Path, log: &LogSummary) -> Result<AcknowledgedLogSummary> {
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

    let state = AppState {
        acknowledged_log: Some(acknowledged.clone()),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_string_pretty(&state)?)?;

    Ok(acknowledged)
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
