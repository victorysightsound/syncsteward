use crate::config::{AppConfig, FolderPolicy, default_config_path, expand_path, load_config};
use crate::inventory::build_target_inventory;
use crate::model::{
    ActionOutcome, ActionStep, ActionStepStatus, ActionTarget, ArtifactReport, CheckStatus,
    ConfigScaffoldReport, ControlAction, ControlReport, LaunchAgentStatus, LogAcknowledgeReport,
    LogSummary, PolicySummary, PreflightCheck, PreflightReport, RemoteStatus, ServiceState,
    StatusReport,
};
use crate::state::{load_state, matches_acknowledged_log, save_acknowledged_log};
use anyhow::Result;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
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

pub fn pause(config_path: Option<&Path>, target: ActionTarget) -> Result<ControlReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let mut report = execute_pause(&loaded.config, &config_source, target);
    record_audit_event(&loaded.config, &mut report);
    Ok(report)
}

pub fn resume(config_path: Option<&Path>, target: ActionTarget) -> Result<ControlReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let mut report = execute_resume(&loaded.config, &config_source, target);
    record_audit_event(&loaded.config, &mut report);
    Ok(report)
}

pub fn acknowledge_latest_log(config_path: Option<&Path>) -> Result<LogAcknowledgeReport> {
    let loaded = load_config(config_path)?;
    let status = collect_status(&loaded.config, loaded.source.description());
    let latest_log = status.latest_log.clone();

    let Some(log) = latest_log.clone() else {
        return Ok(LogAcknowledgeReport {
            outcome: ActionOutcome::NoOp,
            summary: "no rclone log found to acknowledge".to_string(),
            state_path: loaded.config.state_path.clone(),
            acknowledged_log: status.acknowledged_log,
            latest_log: None,
        });
    };

    let acknowledged_log = save_acknowledged_log(&loaded.config.state_path, &log)?;
    Ok(LogAcknowledgeReport {
        outcome: ActionOutcome::Success,
        summary: format!(
            "acknowledged {} as the historical baseline log",
            log.path.display()
        ),
        state_path: loaded.config.state_path.clone(),
        acknowledged_log: Some(acknowledged_log),
        latest_log: Some(log),
    })
}

pub fn scaffold_config(config_path: Option<&Path>, force: bool) -> Result<ConfigScaffoldReport> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    let overwrite = output_path.exists();
    if overwrite && !force {
        anyhow::bail!(
            "config already exists at {} (use --force to overwrite)",
            output_path.display()
        );
    }

    let loaded = if overwrite {
        load_config(Some(output_path.as_path()))?
    } else {
        load_config(None)?
    };
    let inventory = build_target_inventory(&loaded.config, loaded.source.description())?;

    let mut config = loaded.config;
    config.policy.folders = inventory
        .targets
        .into_iter()
        .map(|target| FolderPolicy {
            path: target.local_path,
            mode: target.configured_mode.unwrap_or(target.recommended_mode),
            label: Some(target.name),
        })
        .collect();

    let encoded = toml::to_string_pretty(&config)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output_path, encoded)?;

    Ok(ConfigScaffoldReport {
        outcome: ActionOutcome::Success,
        summary: if overwrite {
            format!(
                "updated SyncSteward config scaffold at {}",
                output_path.display()
            )
        } else {
            format!(
                "wrote SyncSteward config scaffold to {}",
                output_path.display()
            )
        },
        path: output_path,
        overwritten: overwrite,
        folder_policy_count: config.policy.folders.len(),
        file_class_policy_count: config.policy.file_classes.len(),
    })
}

fn collect_status(config: &AppConfig, config_source: String) -> StatusReport {
    let acknowledged_log = load_state(&config.state_path)
        .ok()
        .and_then(|state| state.acknowledged_log);
    let launch_agent = probe_launch_agent(&config.launch_agent_label);
    let remote = probe_remote_service(config);
    let artifacts = scan_artifacts(config);
    let latest_log = summarize_latest_log(&config.rclone_log_dir, config.scan.max_examples);
    let policy = PolicySummary {
        folder_policies: config.policy.folders.clone(),
        file_class_policies: config.policy.file_classes.clone(),
    };

    StatusReport {
        config_source,
        policy,
        launch_agent,
        remote,
        artifacts,
        acknowledged_log,
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
        Some(log) if matches_acknowledged_log(status.acknowledged_log.as_ref(), log) => warn_check(
            "latest_log_clean",
            "latest rclone log issues are acknowledged as historical baseline".to_string(),
            format!(
                "{} out_of_sync, {} errors, {} warnings in {}",
                log.out_of_sync_count,
                log.error_count,
                log.warning_count,
                log.path.display()
            ),
        ),
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
            format!("{} warnings in {}", log.warning_count, log.path.display()),
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

fn execute_pause(config: &AppConfig, config_source: &str, target: ActionTarget) -> ControlReport {
    let mut steps = Vec::new();

    if target.includes_local() {
        steps.push(pause_local_launch_agent(config));
    }
    if target.includes_remote() {
        steps.push(pause_remote_onedrive(config));
    }

    let status = collect_status(config, config_source.to_string());
    let outcome = summarize_outcome(&steps);
    let summary = summarize_control_action(ControlAction::Pause, target, outcome, &steps);

    ControlReport {
        action: ControlAction::Pause,
        target,
        outcome,
        summary,
        steps,
        preflight: None,
        status,
    }
}

fn execute_resume(config: &AppConfig, config_source: &str, target: ActionTarget) -> ControlReport {
    let preflight = evaluate_preflight(collect_status(config, config_source.to_string()));
    if !preflight.ready {
        let mut steps = vec![blocked_step(
            "preflight_gate",
            "resume blocked by preflight failures".to_string(),
            failed_check_ids(&preflight),
        )];
        let outcome = ActionOutcome::Blocked;
        let summary = summarize_control_action(ControlAction::Resume, target, outcome, &steps);
        return ControlReport {
            action: ControlAction::Resume,
            target,
            outcome,
            summary,
            steps: std::mem::take(&mut steps),
            preflight: Some(preflight.clone()),
            status: preflight.status,
        };
    }

    let mut steps = Vec::new();

    if target.includes_remote() {
        steps.push(resume_remote_onedrive(config));
    }
    if target.includes_local() {
        steps.push(resume_local_launch_agent(config));
    }

    let status = collect_status(config, config_source.to_string());
    let outcome = summarize_outcome(&steps);
    let summary = summarize_control_action(ControlAction::Resume, target, outcome, &steps);

    ControlReport {
        action: ControlAction::Resume,
        target,
        outcome,
        summary,
        steps,
        preflight: Some(preflight),
        status,
    }
}

fn pause_local_launch_agent(config: &AppConfig) -> ActionStep {
    let launch_agent = probe_launch_agent(&config.launch_agent_label);
    if !launch_agent.loaded {
        return skipped_step(
            "pause_local_launch_agent",
            format!("{} was already paused", config.launch_agent_label),
            launch_agent.detail,
        );
    }

    let uid = current_uid();
    let domain = format!("gui/{uid}");
    let plist = config.launch_agent_path.to_string_lossy().to_string();
    let primary = run_command("launchctl", ["bootout", domain.as_str(), plist.as_str()]);
    if primary.success {
        return applied_step(
            "pause_local_launch_agent",
            format!("paused {}", config.launch_agent_label),
            primary.trim_or(&primary.stdout).to_string(),
        );
    }

    let fallback = run_command("launchctl", ["unload", plist.as_str()]);
    if fallback.success {
        applied_step(
            "pause_local_launch_agent",
            format!("paused {} via unload fallback", config.launch_agent_label),
            fallback.trim_or(&fallback.stdout).to_string(),
        )
    } else {
        failed_step(
            "pause_local_launch_agent",
            format!("failed to pause {}", config.launch_agent_label),
            format!(
                "bootout: {}; unload fallback: {}",
                primary.trim_or(&primary.stdout),
                fallback.trim_or(&fallback.stdout)
            ),
        )
    }
}

fn resume_local_launch_agent(config: &AppConfig) -> ActionStep {
    let launch_agent = probe_launch_agent(&config.launch_agent_label);
    if launch_agent.loaded {
        return skipped_step(
            "resume_local_launch_agent",
            format!("{} was already loaded", config.launch_agent_label),
            launch_agent.detail,
        );
    }
    if !config.launch_agent_path.exists() {
        return failed_step(
            "resume_local_launch_agent",
            format!("cannot resume {}", config.launch_agent_label),
            format!(
                "launch agent plist does not exist at {}",
                config.launch_agent_path.display()
            ),
        );
    }

    let uid = current_uid();
    let domain = format!("gui/{uid}");
    let plist = config.launch_agent_path.to_string_lossy().to_string();
    let output = run_command("launchctl", ["bootstrap", domain.as_str(), plist.as_str()]);
    if output.success {
        applied_step(
            "resume_local_launch_agent",
            format!("resumed {}", config.launch_agent_label),
            output.trim_or(&output.stdout).to_string(),
        )
    } else {
        failed_step(
            "resume_local_launch_agent",
            format!("failed to resume {}", config.launch_agent_label),
            output.trim_or(&output.stdout).to_string(),
        )
    }
}

fn pause_remote_onedrive(config: &AppConfig) -> ActionStep {
    let remote = probe_remote_service(config);
    if matches!(remote.service_state, ServiceState::Inactive) {
        return skipped_step(
            "pause_remote_onedrive",
            "remote OneDrive service was already inactive".to_string(),
            remote.detail,
        );
    }
    let Some(host) = remote.selected_host.as_deref() else {
        return failed_step(
            "pause_remote_onedrive",
            "cannot pause remote OneDrive service".to_string(),
            remote.detail,
        );
    };

    let stop = run_remote_systemctl(config, host, "stop");
    if stop.success {
        applied_step(
            "pause_remote_onedrive",
            format!("paused {} on {}", config.remote.onedrive_service, host),
            stop.trim_or(&stop.stdout).to_string(),
        )
    } else {
        failed_step(
            "pause_remote_onedrive",
            format!(
                "failed to pause {} on {}",
                config.remote.onedrive_service, host
            ),
            stop.trim_or(&stop.stdout).to_string(),
        )
    }
}

fn resume_remote_onedrive(config: &AppConfig) -> ActionStep {
    let remote = probe_remote_service(config);
    if matches!(remote.service_state, ServiceState::Active) {
        return skipped_step(
            "resume_remote_onedrive",
            "remote OneDrive service was already active".to_string(),
            remote.detail,
        );
    }
    let Some(host) = remote.selected_host.as_deref() else {
        let fallback_host = config
            .remote
            .preferred_hosts
            .first()
            .cloned()
            .unwrap_or_default();
        if fallback_host.is_empty() {
            return failed_step(
                "resume_remote_onedrive",
                "cannot resume remote OneDrive service".to_string(),
                "no configured remote host is available".to_string(),
            );
        }
        return resume_remote_onedrive_on_host(config, &fallback_host);
    };

    resume_remote_onedrive_on_host(config, host)
}

fn resume_remote_onedrive_on_host(config: &AppConfig, host: &str) -> ActionStep {
    let start = run_remote_systemctl(config, host, "start");
    if start.success {
        applied_step(
            "resume_remote_onedrive",
            format!("resumed {} on {}", config.remote.onedrive_service, host),
            start.trim_or(&start.stdout).to_string(),
        )
    } else {
        failed_step(
            "resume_remote_onedrive",
            format!(
                "failed to resume {} on {}",
                config.remote.onedrive_service, host
            ),
            start.trim_or(&start.stdout).to_string(),
        )
    }
}

fn record_audit_event(config: &AppConfig, report: &mut ControlReport) {
    if let Err(error) = append_audit_record(&config.audit_log_path, report) {
        report.steps.push(failed_step(
            "audit_log_write",
            format!(
                "failed to record {} action audit log",
                action_name(report.action)
            ),
            error.to_string(),
        ));
        if report.outcome != ActionOutcome::Blocked {
            report.outcome = ActionOutcome::Failed;
        }
        report.summary =
            summarize_control_action(report.action, report.target, report.outcome, &report.steps);
    }
}

fn append_audit_record(path: &Path, report: &ControlReport) -> Result<()> {
    #[derive(Serialize)]
    struct AuditRecord<'a> {
        timestamp_unix_ms: u128,
        action: ControlAction,
        target: ActionTarget,
        outcome: ActionOutcome,
        summary: &'a str,
        blocked_check_ids: Vec<&'a str>,
        step_ids: Vec<&'a str>,
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let blocked_check_ids = report
        .preflight
        .as_ref()
        .map(|preflight| {
            preflight
                .checks
                .iter()
                .filter(|check| check.status == CheckStatus::Fail)
                .map(|check| check.id.as_str())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let step_ids = report
        .steps
        .iter()
        .map(|step| step.id.as_str())
        .collect::<Vec<_>>();

    let record = AuditRecord {
        timestamp_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        action: report.action,
        target: report.target,
        outcome: report.outcome,
        summary: &report.summary,
        blocked_check_ids,
        step_ids,
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn probe_launch_agent(label: &str) -> LaunchAgentStatus {
    let output = run_command("launchctl", ["list"]);
    if !output.success {
        return LaunchAgentStatus {
            label: label.to_string(),
            loaded: false,
            running: false,
            detail: format!("launchctl list failed: {}", output.trim_or(&output.stdout)),
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

        let output = run_remote_service_state(config, host);
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

fn run_remote_systemctl(config: &AppConfig, host: &str, action: &str) -> CommandOutput {
    let primary = run_remote_systemctl_raw(config, host, action);
    if primary.success {
        return primary;
    }

    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let command = format!("systemctl {} {}", action, config.remote.onedrive_service);
    let fallback = run_command(
        "ssh",
        [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-i",
            config.ssh_key_path.to_string_lossy().as_ref(),
            remote.as_str(),
            command.as_str(),
        ],
    );

    if fallback.success {
        fallback
    } else {
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: format!(
                "sudo path: {}; direct path: {}",
                primary.trim_or(&primary.stdout),
                fallback.trim_or(&fallback.stdout)
            ),
        }
    }
}

fn run_remote_systemctl_raw(config: &AppConfig, host: &str, action: &str) -> CommandOutput {
    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let command = format!(
        "sudo -n systemctl {} {}",
        action, config.remote.onedrive_service
    );
    run_command(
        "ssh",
        [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-i",
            config.ssh_key_path.to_string_lossy().as_ref(),
            remote.as_str(),
            command.as_str(),
        ],
    )
}

fn run_remote_service_state(config: &AppConfig, host: &str) -> CommandOutput {
    let primary = run_remote_systemctl_raw(config, host, "is-active");
    if primary.success || !primary.stdout.trim().is_empty() {
        return primary;
    }

    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let command = format!("systemctl is-active {}", config.remote.onedrive_service);
    let fallback = run_command(
        "ssh",
        [
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            "-i",
            config.ssh_key_path.to_string_lossy().as_ref(),
            remote.as_str(),
            command.as_str(),
        ],
    );

    if fallback.success || !fallback.stdout.trim().is_empty() {
        fallback
    } else {
        primary
    }
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
    let error_count = contents.matches("ERROR:").count()
        + contents.matches("ERROR :").count()
        + contents.matches("Fatal error").count();
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

fn applied_step(id: &str, summary: String, detail: String) -> ActionStep {
    ActionStep {
        id: id.to_string(),
        status: ActionStepStatus::Applied,
        summary,
        detail,
    }
}

fn skipped_step(id: &str, summary: String, detail: String) -> ActionStep {
    ActionStep {
        id: id.to_string(),
        status: ActionStepStatus::Skipped,
        summary,
        detail,
    }
}

fn blocked_step(id: &str, summary: String, detail: String) -> ActionStep {
    ActionStep {
        id: id.to_string(),
        status: ActionStepStatus::Blocked,
        summary,
        detail,
    }
}

fn failed_step(id: &str, summary: String, detail: String) -> ActionStep {
    ActionStep {
        id: id.to_string(),
        status: ActionStepStatus::Failed,
        summary,
        detail,
    }
}

fn summarize_outcome(steps: &[ActionStep]) -> ActionOutcome {
    if steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Failed)
    {
        ActionOutcome::Failed
    } else if steps
        .iter()
        .all(|step| step.status == ActionStepStatus::Skipped)
    {
        ActionOutcome::NoOp
    } else {
        ActionOutcome::Success
    }
}

fn summarize_control_action(
    action: ControlAction,
    target: ActionTarget,
    outcome: ActionOutcome,
    steps: &[ActionStep],
) -> String {
    let target_name = target.label();
    let action_name = action_name(action);
    match outcome {
        ActionOutcome::Success => {
            let applied = steps
                .iter()
                .filter(|step| step.status == ActionStepStatus::Applied)
                .count();
            format!("{action_name} succeeded for {target_name} ({applied} applied steps)")
        }
        ActionOutcome::NoOp => format!("nothing changed; {target_name} was already {action_name}d"),
        ActionOutcome::Blocked => format!("{action_name} blocked for {target_name}"),
        ActionOutcome::Failed => format!("{action_name} completed with failures for {target_name}"),
    }
}

fn failed_check_ids(report: &PreflightReport) -> String {
    let failed = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .map(|check| check.id.as_str())
        .collect::<Vec<_>>();
    if failed.is_empty() {
        "preflight did not expose specific failed checks".to_string()
    } else {
        failed.join(", ")
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

fn action_name(action: ControlAction) -> &'static str {
    match action {
        ControlAction::Pause => "pause",
        ControlAction::Resume => "resume",
    }
}

fn current_uid() -> String {
    let output = run_command("id", ["-u"]);
    let uid = output.stdout.trim();
    if output.success && !uid.is_empty() {
        uid.to_string()
    } else {
        "0".to_string()
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

impl ActionTarget {
    fn includes_local(self) -> bool {
        matches!(self, Self::Local | Self::All)
    }

    fn includes_remote(self) -> bool {
        matches!(self, Self::Remote | Self::All)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::All => "all targets",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionOutcome, ActionStepStatus, analyze_log_contents, evaluate_preflight,
        summarize_outcome,
    };
    use crate::config::PolicyConfig;
    use crate::model::{
        AcknowledgedLogSummary, ActionStep, ArtifactReport, CheckStatus, LaunchAgentStatus,
        PolicySummary, RemoteStatus, ServiceState, StatusReport,
    };
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

    #[test]
    fn summarize_outcome_detects_blocking_failures() {
        let failed = summarize_outcome(&[ActionStep {
            id: "x".to_string(),
            status: ActionStepStatus::Failed,
            summary: "failed".to_string(),
            detail: "detail".to_string(),
        }]);
        let noop = summarize_outcome(&[ActionStep {
            id: "x".to_string(),
            status: ActionStepStatus::Skipped,
            summary: "skipped".to_string(),
            detail: "detail".to_string(),
        }]);
        let success = summarize_outcome(&[
            ActionStep {
                id: "x".to_string(),
                status: ActionStepStatus::Applied,
                summary: "applied".to_string(),
                detail: "detail".to_string(),
            },
            ActionStep {
                id: "y".to_string(),
                status: ActionStepStatus::Skipped,
                summary: "skipped".to_string(),
                detail: "detail".to_string(),
            },
        ]);

        assert_eq!(failed, ActionOutcome::Failed);
        assert_eq!(noop, ActionOutcome::NoOp);
        assert_eq!(success, ActionOutcome::Success);
    }

    #[test]
    fn acknowledged_latest_log_downgrades_log_blocker_to_warning() {
        let latest_log = analyze_log_contents(
            PathBuf::from("/tmp/sync-2026-03-29.log"),
            "\
[2026-03-29 10:00:00] ========== Cloud Sync Started ==========\n\
[2026-03-29 10:00:01] WARNING: Ministry had issues\n\
path1 and path2 are out of sync, run --resync to recover\n\
[2026-03-29 10:00:05] ERROR: neither remote is reachable\n\
[2026-03-29 10:05:00] ========== Cloud Sync Completed ==========\n",
            5,
        );
        let acknowledged_log = AcknowledgedLogSummary {
            path: latest_log.path.clone(),
            warning_count: latest_log.warning_count,
            error_count: latest_log.error_count,
            out_of_sync_count: latest_log.out_of_sync_count,
            last_started_line: latest_log.last_started_line.clone(),
            last_completed_line: latest_log.last_completed_line.clone(),
            acknowledged_at_unix_ms: 1,
        };
        let report = evaluate_preflight(StatusReport {
            config_source: "test".to_string(),
            policy: PolicySummary {
                folder_policies: Vec::new(),
                file_class_policies: PolicyConfig::default().file_classes,
            },
            launch_agent: LaunchAgentStatus {
                label: "com.example.test".to_string(),
                loaded: false,
                running: false,
                detail: "not loaded".to_string(),
            },
            remote: RemoteStatus {
                selected_host: Some("127.0.0.1".to_string()),
                reachable: true,
                service_state: ServiceState::Inactive,
                detail: "inactive".to_string(),
            },
            artifacts: ArtifactReport {
                roots_scanned: Vec::new(),
                conflict_count: 0,
                conflict_examples: Vec::new(),
                safe_backup_count: 0,
                safe_backup_examples: Vec::new(),
            },
            acknowledged_log: Some(acknowledged_log),
            latest_log: Some(latest_log),
        });

        assert!(report.ready);
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "latest_log_clean")
            .expect("latest_log_clean");
        assert_eq!(check.status, CheckStatus::Warn);
    }
}
