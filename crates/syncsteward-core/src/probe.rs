use crate::config::{AppConfig, load_config};
use crate::model::{
    ArtifactReport, CheckStatus, LaunchAgentStatus, LogSummary, PreflightCheck, PreflightReport,
    RemoteStatus, ServiceState, StatusReport,
};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::{DirEntry, WalkDir};

pub fn status(config_path: Option<&Path>) -> Result<StatusReport> {
    let loaded = load_config(config_path)?;
    Ok(collect_status(&loaded.config, loaded.source.description()))
}

pub fn preflight(config_path: Option<&Path>) -> Result<PreflightReport> {
    let loaded = load_config(config_path)?;
    let status = collect_status(&loaded.config, loaded.source.description());
    Ok(evaluate_preflight(status))
}

fn collect_status(config: &AppConfig, config_source: String) -> StatusReport {
    let launch_agent = probe_launch_agent(&config.launch_agent_label);
    let remote = probe_remote_service(config);
    let artifacts = scan_artifacts(config);
    let latest_log = summarize_latest_log(&config.rclone_log_dir, config.scan.max_examples);

    StatusReport {
        config_source,
        launch_agent,
        remote,
        artifacts,
        latest_log,
    }
}

fn evaluate_preflight(status: StatusReport) -> PreflightReport {
    let mut checks = Vec::new();

    checks.push(if status.launch_agent.loaded {
        fail_check(
            "local_launch_agent_paused",
            format!("{} is still loaded", status.launch_agent.label),
            status.launch_agent.detail.clone(),
        )
    } else {
        pass_check(
            "local_launch_agent_paused",
            format!("{} is not loaded", status.launch_agent.label),
            status.launch_agent.detail.clone(),
        )
    });

    checks.push(match status.remote.service_state {
        ServiceState::Active => fail_check(
            "remote_onedrive_paused",
            "remote OneDrive service is still active".to_string(),
            status.remote.detail.clone(),
        ),
        ServiceState::Inactive => pass_check(
            "remote_onedrive_paused",
            "remote OneDrive service is inactive".to_string(),
            status.remote.detail.clone(),
        ),
        ServiceState::Failed => fail_check(
            "remote_onedrive_paused",
            "remote OneDrive service is in a failed state".to_string(),
            status.remote.detail.clone(),
        ),
        ServiceState::Unknown => warn_check(
            "remote_onedrive_paused",
            "remote OneDrive service could not be verified".to_string(),
            status.remote.detail.clone(),
        ),
    });

    checks.push(if status.artifacts.conflict_count == 0 {
        pass_check(
            "no_conflict_artifacts",
            "no .conflict artifacts detected".to_string(),
            "scan roots are clear".to_string(),
        )
    } else {
        fail_check(
            "no_conflict_artifacts",
            format!(
                "{} conflict artifacts still need review",
                status.artifacts.conflict_count
            ),
            format_examples(&status.artifacts.conflict_examples),
        )
    });

    checks.push(if status.artifacts.safe_backup_count == 0 {
        pass_check(
            "no_safe_backup_artifacts",
            "no victorystore safeBackup artifacts detected".to_string(),
            "scan roots are clear".to_string(),
        )
    } else {
        fail_check(
            "no_safe_backup_artifacts",
            format!(
                "{} safeBackup artifacts still need review",
                status.artifacts.safe_backup_count
            ),
            format_examples(&status.artifacts.safe_backup_examples),
        )
    });

    checks.push(match &status.latest_log {
        Some(log) if log.out_of_sync_count > 0 || log.error_count > 0 => fail_check(
            "latest_log_clean",
            "latest rclone log still reports out-of-sync or error conditions".to_string(),
            format!(
                "{} out_of_sync, {} errors, {} warnings",
                log.out_of_sync_count, log.error_count, log.warning_count
            ),
        ),
        Some(log) if log.warning_count > 0 => warn_check(
            "latest_log_clean",
            "latest rclone log still reports warnings".to_string(),
            format!(
                "{} warnings in {}",
                log.warning_count,
                log.path.display()
            ),
        ),
        Some(log) => pass_check(
            "latest_log_clean",
            "latest rclone log is clean".to_string(),
            format!("checked {}", log.path.display()),
        ),
        None => warn_check(
            "latest_log_clean",
            "no rclone log was found to verify".to_string(),
            "cannot confirm prior sync state".to_string(),
        ),
    });

    let ready = checks.iter().all(|check| check.status != CheckStatus::Fail);

    PreflightReport {
        ready,
        checks,
        status,
    }
}

fn probe_launch_agent(label: &str) -> LaunchAgentStatus {
    let output = run_command("launchctl", ["list"]);
    if !output.success {
        return LaunchAgentStatus {
            label: label.to_string(),
            loaded: false,
            running: false,
            detail: format!(
                "launchctl list failed: {}",
                output.trim_or(&output.stdout)
            ),
        };
    }

    let matching_line = output
        .stdout
        .lines()
        .find(|line| line.split_whitespace().last() == Some(label));

    match matching_line {
        Some(line) => {
            let pid_field = line.split_whitespace().next().unwrap_or("-");
            let running = pid_field.parse::<i32>().ok().is_some_and(|pid| pid > 0);
            LaunchAgentStatus {
                label: label.to_string(),
                loaded: true,
                running,
                detail: line.trim().to_string(),
            }
        }
        None => LaunchAgentStatus {
            label: label.to_string(),
            loaded: false,
            running: false,
            detail: "launchctl list does not contain the label".to_string(),
        },
    }
}

fn probe_remote_service(config: &AppConfig) -> RemoteStatus {
    for host in &config.remote.preferred_hosts {
        if !ssh_reachable(config, host) {
            continue;
        }

        let remote = format!("{}@{}", config.remote.ssh_user, host);
        let command = format!("systemctl is-active {}", config.remote.onedrive_service);
        let output = run_command(
            "ssh",
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=3",
                "-i",
                config.ssh_key_path.to_string_lossy().as_ref(),
                remote.as_str(),
                command.as_str(),
            ],
        );

        let raw = output.stdout.trim();
        let service_state = match raw {
            "active" => ServiceState::Active,
            "inactive" => ServiceState::Inactive,
            "failed" => ServiceState::Failed,
            _ => ServiceState::Unknown,
        };

        let detail = if !raw.is_empty() {
            format!("{} returned {}", config.remote.onedrive_service, raw)
        } else if output.success {
            format!("{} returned empty output", config.remote.onedrive_service)
        } else {
            format!("ssh command failed: {}", output.trim_or(&output.stdout))
        };

        return RemoteStatus {
            selected_host: Some(host.clone()),
            reachable: true,
            service_state,
            detail,
        };
    }

    RemoteStatus {
        selected_host: None,
        reachable: false,
        service_state: ServiceState::Unknown,
        detail: "no configured remote host responded over SSH".to_string(),
    }
}

fn ssh_reachable(config: &AppConfig, host: &str) -> bool {
    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let output = run_command(
        "ssh",
        [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=3",
            "-i",
            config.ssh_key_path.to_string_lossy().as_ref(),
            remote.as_str(),
            "true",
        ],
    );
    output.success
}

fn scan_artifacts(config: &AppConfig) -> ArtifactReport {
    let mut roots_scanned = Vec::new();
    let mut conflict_examples = Vec::new();
    let mut safe_backup_examples = Vec::new();
    let mut conflict_count = 0usize;
    let mut safe_backup_count = 0usize;

    for root in &config.scan.roots {
        if !root.exists() {
            continue;
        }
        roots_scanned.push(root.clone());

        let iterator = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(skip_git)
            .filter_map(Result::ok);

        for entry in iterator {
            if !entry.file_type().is_file() {
                continue;
            }

            let name = entry.file_name().to_string_lossy();
            if name.contains(".conflict") {
                conflict_count += 1;
                if conflict_examples.len() < config.scan.max_examples {
                    conflict_examples.push(entry.path().to_path_buf());
                }
            }
            if name.contains("victorystore-safeBackup") {
                safe_backup_count += 1;
                if safe_backup_examples.len() < config.scan.max_examples {
                    safe_backup_examples.push(entry.path().to_path_buf());
                }
            }
        }
    }

    ArtifactReport {
        roots_scanned,
        conflict_count,
        conflict_examples,
        safe_backup_count,
        safe_backup_examples,
    }
}

fn skip_git(entry: &DirEntry) -> bool {
    entry.file_name() != ".git"
}

fn summarize_latest_log(log_dir: &Path, max_examples: usize) -> Option<LogSummary> {
    let path = latest_log_path(log_dir)?;
    let contents = fs::read_to_string(&path).ok()?;
    Some(analyze_log_contents(path, &contents, max_examples))
}

fn latest_log_path(log_dir: &Path) -> Option<PathBuf> {
    let mut paths = fs::read_dir(log_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("sync-") && name.ends_with(".log"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.pop()
}

fn analyze_log_contents(path: PathBuf, contents: &str, max_examples: usize) -> LogSummary {
    let warning_count = contents.matches("WARNING:").count();
    let error_count =
        contents.matches("ERROR:").count() + contents.matches("ERROR :").count() + contents.matches("Fatal error").count();
    let out_of_sync_count = contents.matches("out of sync").count();
    let last_started_line = contents
        .lines()
        .filter(|line| line.contains("Cloud Sync Started"))
        .next_back()
        .map(ToString::to_string);
    let last_completed_line = contents
        .lines()
        .filter(|line| line.contains("Cloud Sync Completed"))
        .next_back()
        .map(ToString::to_string);
    let issue_examples = contents
        .lines()
        .filter(|line| {
            line.contains("WARNING:")
                || line.contains("ERROR:")
                || line.contains("ERROR :")
                || line.contains("out of sync")
        })
        .take(max_examples)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    LogSummary {
        path,
        warning_count,
        error_count,
        out_of_sync_count,
        last_started_line,
        last_completed_line,
        issue_examples,
    }
}

fn pass_check(id: &str, summary: String, detail: String) -> PreflightCheck {
    PreflightCheck {
        id: id.to_string(),
        status: CheckStatus::Pass,
        summary,
        detail,
    }
}

fn warn_check(id: &str, summary: String, detail: String) -> PreflightCheck {
    PreflightCheck {
        id: id.to_string(),
        status: CheckStatus::Warn,
        summary,
        detail,
    }
}

fn fail_check(id: &str, summary: String, detail: String) -> PreflightCheck {
    PreflightCheck {
        id: id.to_string(),
        status: CheckStatus::Fail,
        summary,
        detail,
    }
}

fn format_examples(examples: &[PathBuf]) -> String {
    if examples.is_empty() {
        "no example paths recorded".to_string()
    } else {
        examples
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

impl CommandOutput {
    fn trim_or<'a>(&'a self, fallback: &'a str) -> &'a str {
        let stderr = self.stderr.trim();
        if stderr.is_empty() {
            fallback.trim()
        } else {
            stderr
        }
    }
}

fn run_command<I, S>(program: &str, args: I) -> CommandOutput
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    match Command::new(program).args(args).output() {
        Ok(output) => CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(error) => CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::analyze_log_contents;
    use std::path::PathBuf;

    #[test]
    fn log_analysis_counts_expected_markers() {
        let summary = analyze_log_contents(
            PathBuf::from("/tmp/sync-2026-03-29.log"),
            "\
[2026-03-29 10:00:00] ========== Cloud Sync Started ==========\n\
[2026-03-29 10:00:01] WARNING: Ministry had issues\n\
path1 and path2 are out of sync, run --resync to recover\n\
[2026-03-29 10:00:05] ERROR: neither remote is reachable\n\
[2026-03-29 10:05:00] ========== Cloud Sync Completed ==========\n",
            5,
        );

        assert_eq!(summary.warning_count, 1);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.out_of_sync_count, 1);
        assert!(summary.last_started_line.is_some());
        assert!(summary.last_completed_line.is_some());
        assert_eq!(summary.issue_examples.len(), 3);
    }
}
