use crate::config::{
    AppConfig, FolderPolicy, ManagedTarget, PolicyMode, RunnerLaunchAgentConfig,
    default_config_path, expand_path, load_config, normalize_app_config,
};
use crate::inventory::build_target_inventory;
use crate::model::{
    ActionOutcome, ActionStep, ActionStepStatus, ActionTarget, AddManagedTargetReport, AlertRecord,
    AlertReport, AlertSeverity, ArtifactReport, CheckStatus, ConfigScaffoldReport, ControlAction,
    ControlReport, CycleSkippedTarget, EnsureTargetIdsReport, LaunchAgentStatus,
    LogAcknowledgeReport, LogSummary, ManagedTargetIdAssignment, ManagedTargetIdAssignmentReason,
    NotifyAlertsReport, PolicySummary, PreflightCheck, PreflightReport,
    RelocateManagedTargetReport, RemoteStatus, RunCycleReport, RunnerAgentAction,
    RunnerAgentControlReport, RunnerAgentStatusReport, RunnerTickReport, ServiceState,
    StatusReport, TargetBlocker, TargetCheckReport, TargetCheckSetReport, TargetEvaluation,
    TargetRunReport,
};
use crate::state::{
    RunnerCycleState, RunnerTickState, TargetRunState, load_state, matches_acknowledged_log,
    save_acknowledged_log, save_runner_cycle, save_runner_tick, save_target_run,
};
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;
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

pub fn alerts(config_path: Option<&Path>) -> Result<AlertReport> {
    let loaded = load_config(config_path)?;
    let report = evaluate_alerts(&loaded.config, loaded.source.description())?;
    Ok(report)
}

pub fn notify_alerts(config_path: Option<&Path>, dry_run: bool) -> Result<NotifyAlertsReport> {
    let loaded = load_config(config_path)?;
    let report = evaluate_alerts(&loaded.config, loaded.source.description())?;
    let mut steps = Vec::new();

    if report.alerts.is_empty() {
        steps.push(skipped_step(
            "alerts_snapshot",
            "no active alerts to notify".to_string(),
            "alert evaluation returned no active issues".to_string(),
        ));
        return Ok(NotifyAlertsReport {
            outcome: ActionOutcome::NoOp,
            summary: "no active alerts".to_string(),
            dry_run,
            alerts: report.alerts,
            steps,
        });
    }

    let summary = summarize_alerts_notification(&report.alerts);
    if !loaded.config.alerts.enable_macos_notifications {
        steps.push(skipped_step(
            "macos_notifications",
            "macOS notifications are disabled in config".to_string(),
            summary.clone(),
        ));
        return Ok(NotifyAlertsReport {
            outcome: ActionOutcome::NoOp,
            summary: "notifications are disabled".to_string(),
            dry_run,
            alerts: report.alerts,
            steps,
        });
    }

    if dry_run {
        steps.push(applied_step(
            "macos_notifications",
            "dry run prepared a macOS notification".to_string(),
            summary.clone(),
        ));
        return Ok(NotifyAlertsReport {
            outcome: ActionOutcome::Success,
            summary: format!("dry run would notify {} active alerts", report.alerts.len()),
            dry_run,
            alerts: report.alerts,
            steps,
        });
    }

    let output = run_command(
        "osascript",
        [
            "-e",
            format!(
                "display notification {} with title {} subtitle {}",
                apple_script_string(&summary),
                apple_script_string("SyncSteward"),
                apple_script_string(&format!(
                    "{} active alert{}",
                    report.alerts.len(),
                    if report.alerts.len() == 1 { "" } else { "s" }
                )),
            )
            .as_str(),
        ],
    );

    if output.success {
        steps.push(applied_step(
            "macos_notifications",
            "sent a macOS notification".to_string(),
            summary.clone(),
        ));
        Ok(NotifyAlertsReport {
            outcome: ActionOutcome::Success,
            summary: format!(
                "sent notification for {} active alerts",
                report.alerts.len()
            ),
            dry_run,
            alerts: report.alerts,
            steps,
        })
    } else {
        steps.push(failed_step(
            "macos_notifications",
            "failed to send a macOS notification".to_string(),
            summarize_command_output(&output),
        ));
        Ok(NotifyAlertsReport {
            outcome: ActionOutcome::Failed,
            summary: "failed to send notification".to_string(),
            dry_run,
            alerts: report.alerts,
            steps,
        })
    }
}

pub fn check_targets(config_path: Option<&Path>) -> Result<TargetCheckSetReport> {
    let loaded = load_config(config_path)?;
    let preflight = evaluate_preflight(collect_status(&loaded.config, loaded.source.description()));
    let inventory = build_target_inventory(&loaded.config, loaded.source.description())?;
    let evaluations = inventory
        .targets
        .into_iter()
        .map(|target| evaluate_target(&preflight, target))
        .collect();

    Ok(TargetCheckSetReport {
        config_source: loaded.source.description(),
        preflight_ready: preflight.ready,
        evaluations,
    })
}

pub fn check_target(config_path: Option<&Path>, selector: &str) -> Result<TargetCheckReport> {
    let loaded = load_config(config_path)?;
    let preflight = evaluate_preflight(collect_status(&loaded.config, loaded.source.description()));
    let inventory = build_target_inventory(&loaded.config, loaded.source.description())?;
    let target = resolve_inventory_target(inventory.targets, selector)?;

    Ok(TargetCheckReport {
        config_source: loaded.source.description(),
        selector: selector.to_string(),
        preflight_ready: preflight.ready,
        evaluation: evaluate_target(&preflight, target),
    })
}

pub fn run_target(
    config_path: Option<&Path>,
    selector: &str,
    dry_run: bool,
) -> Result<TargetRunReport> {
    run_target_inner(config_path, selector, dry_run, false)
}

fn run_target_inner(
    config_path: Option<&Path>,
    selector: &str,
    dry_run: bool,
    reuse_existing_lock: bool,
) -> Result<TargetRunReport> {
    let loaded = load_config(config_path)?;
    let preflight = evaluate_preflight(collect_status(&loaded.config, loaded.source.description()));
    let inventory = build_target_inventory(&loaded.config, loaded.source.description())?;
    let target = resolve_inventory_target(inventory.targets, selector)?;
    let evaluation = evaluate_target(&preflight, target);
    let mut steps = Vec::new();

    if !preflight.ready {
        steps.push(blocked_step(
            "preflight_gate",
            "target execution blocked by global preflight failures".to_string(),
            failed_check_ids(&preflight),
        ));
    }

    if !evaluation.ready {
        steps.push(blocked_step(
            "target_gate",
            format!("target {} is not ready to run", evaluation.target.name),
            format_target_blockers(&evaluation.blockers),
        ));
    }

    if evaluation.effective_mode != crate::config::PolicyMode::BackupOnly {
        steps.push(blocked_step(
            "execution_mode_unsupported",
            format!(
                "{} is configured as {}",
                evaluation.target.name,
                describe_policy_mode(evaluation.effective_mode)
            ),
            "folder-scoped execution currently supports backup-only targets only".to_string(),
        ));
    }

    if !evaluation.target.local_path.is_dir() {
        steps.push(blocked_step(
            "local_path_not_directory",
            format!(
                "{} is not a directory",
                evaluation.target.local_path.display()
            ),
            "folder-scoped execution currently expects directory targets".to_string(),
        ));
    }

    if !steps.is_empty() {
        let outcome = ActionOutcome::Blocked;
        let summary = format!(
            "{} blocked for {}",
            if dry_run { "dry run" } else { "run" },
            evaluation.target.name
        );
        let report = TargetRunReport {
            config_source: loaded.source.description(),
            selector: selector.to_string(),
            dry_run,
            outcome,
            summary,
            preflight_ready: preflight.ready,
            evaluation: evaluation.clone(),
            steps,
        };
        record_target_run(&loaded.config, &report);
        return Ok(report);
    }

    let mut acquired_lock = None;
    if reuse_existing_lock {
        steps.push(skipped_step(
            "legacy_lock",
            format!(
                "reusing cycle-held legacy lock {}",
                loaded.config.legacy_lock_path.display()
            ),
            "run-cycle already owns the legacy sync lock for this execution".to_string(),
        ));
    } else {
        steps.push(acquire_legacy_lock(&loaded.config, &mut acquired_lock)?);
        if steps
            .last()
            .is_some_and(|step| step.status == ActionStepStatus::Blocked)
        {
            let report = TargetRunReport {
                config_source: loaded.source.description(),
                selector: selector.to_string(),
                dry_run,
                outcome: ActionOutcome::Blocked,
                summary: format!(
                    "{} blocked for {}",
                    if dry_run { "dry run" } else { "run" },
                    evaluation.target.name
                ),
                preflight_ready: preflight.ready,
                evaluation: evaluation.clone(),
                steps,
            };
            record_target_run(&loaded.config, &report);
            return Ok(report);
        }
    }

    let Some(host) = probe_remote_service(&loaded.config).selected_host else {
        steps.push(failed_step(
            "select_remote_host",
            "no reachable remote host was available".to_string(),
            "could not establish an SSH-backed rclone target".to_string(),
        ));
        let report = TargetRunReport {
            config_source: loaded.source.description(),
            selector: selector.to_string(),
            dry_run,
            outcome: ActionOutcome::Failed,
            summary: format!(
                "{} failed for {}",
                if dry_run { "dry run" } else { "run" },
                evaluation.target.name
            ),
            preflight_ready: preflight.ready,
            evaluation: evaluation.clone(),
            steps,
        };
        record_target_run(&loaded.config, &report);
        return Ok(report);
    };
    steps.push(applied_step(
        "select_remote_host",
        format!("selected remote host {}", host),
        format!("using {} for {}", host, evaluation.target.remote_path),
    ));

    let temp_dir = make_temp_workdir(&evaluation.target.name)?;
    let result = if let Some(snapshot_policy) =
        target_snapshot_policy(&loaded.config, &evaluation.target.name)
    {
        execute_snapshot_backup_target(
            &loaded.config,
            &evaluation.target,
            snapshot_policy,
            &host,
            dry_run,
            &temp_dir,
            &mut steps,
        )
    } else {
        execute_backup_only_target(
            &loaded.config,
            &evaluation.target,
            &host,
            dry_run,
            &temp_dir,
            &mut steps,
        )
    };
    let cleanup_result = fs::remove_dir_all(&temp_dir);
    drop(acquired_lock);
    if let Err(error) = cleanup_result {
        steps.push(failed_step(
            "cleanup_temp_workdir",
            format!("failed to remove {}", temp_dir.display()),
            error.to_string(),
        ));
    }

    let outcome = match result {
        Ok(()) => summarize_run_outcome(&steps),
        Err(error) => {
            steps.push(failed_step(
                "execute_target",
                format!(
                    "{} failed for {}",
                    if dry_run { "dry run" } else { "run" },
                    evaluation.target.name
                ),
                error.to_string(),
            ));
            summarize_run_outcome(&steps)
        }
    };

    let summary = summarize_target_run(&evaluation.target.name, dry_run, outcome, &steps);
    let report = TargetRunReport {
        config_source: loaded.source.description(),
        selector: selector.to_string(),
        dry_run,
        outcome,
        summary,
        preflight_ready: preflight.ready,
        evaluation,
        steps,
    };
    record_target_run(&loaded.config, &report);
    Ok(report)
}

pub fn run_cycle(config_path: Option<&Path>, dry_run: bool) -> Result<RunCycleReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let started_at_unix_ms = now_unix_ms();
    let preflight = evaluate_preflight(collect_status(&loaded.config, config_source.clone()));
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let approved_selectors = loaded.config.runner.approved_targets.clone();

    let mut target_runs = Vec::new();
    let mut skipped_targets = Vec::new();
    let mut cycle_lock = None;
    let cycle_lock_step = acquire_legacy_lock(&loaded.config, &mut cycle_lock)?;
    if cycle_lock_step.status == ActionStepStatus::Blocked {
        skipped_targets.push(CycleSkippedTarget {
            selector: "*cycle*".to_string(),
            summary: cycle_lock_step.summary,
            detail: cycle_lock_step.detail,
        });
    }

    if skipped_targets.is_empty() {
        for selector in &approved_selectors {
        let selector_path = expand_path(Path::new(selector));
        let resolved = inventory
            .targets
            .iter()
            .find(|target| target_matches_selector(target, selector, &selector_path))
            .cloned();

        let Some(target) = resolved else {
            skipped_targets.push(CycleSkippedTarget {
                selector: selector.clone(),
                summary: format!("approved target selector {} did not resolve", selector),
                detail: "update runner.approved_targets so every selector matches a current target"
                    .to_string(),
            });
            continue;
        };

        let selector_for_run = target
            .target_id
            .clone()
            .unwrap_or_else(|| target.name.clone());
        let report = run_target_inner(config_path, &selector_for_run, dry_run, true)?;
        if report.outcome == ActionOutcome::Blocked
            && !report.evaluation.blockers.is_empty()
            && report
                .steps
                .iter()
                .all(|step| step.status == ActionStepStatus::Blocked)
        {
            skipped_targets.push(CycleSkippedTarget {
                selector: selector.clone(),
                summary: format!(
                    "{} was not ready during cycle execution",
                    report.evaluation.target.name
                ),
                detail: format_target_blockers(&report.evaluation.blockers),
            });
        }
        target_runs.push(report);
    }
    }

    drop(cycle_lock);

    let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
    let notification = if loaded.config.runner.notify_after_cycle {
        Some(notify_alerts(config_path, dry_run)?)
    } else {
        None
    };

    let outcome = summarize_cycle_outcome(&target_runs, &skipped_targets, notification.as_ref());
    let summary = summarize_cycle_report(
        &approved_selectors,
        &target_runs,
        &skipped_targets,
        &alert_report.alerts,
        dry_run,
        outcome,
    );

    let report = RunCycleReport {
        config_source,
        dry_run,
        outcome,
        summary,
        preflight_ready: preflight.ready,
        approved_target_count: approved_selectors.len(),
        target_runs,
        skipped_targets,
        alerts: alert_report.alerts,
        notification,
    };
    record_cycle_run(&loaded.config, &report, started_at_unix_ms);
    Ok(report)
}

pub fn runner_tick(config_path: Option<&Path>, dry_run: bool) -> Result<RunnerTickReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let state = load_state(&loaded.config.state_path)?;
    let now_unix_ms = now_unix_ms();
    let interval_ms = u128::from(loaded.config.runner.cycle_interval_minutes) * 60 * 1000;
    let last_live_cycle_finished_at_unix_ms = state.runner.last_live_cycle_finished_at_unix_ms;
    let (due, next_due_at_unix_ms) = runner_due_status(
        last_live_cycle_finished_at_unix_ms,
        interval_ms,
        now_unix_ms,
    );

    let mut steps = Vec::new();
    let (outcome, summary, preflight_ready, cycle, alerts, notification) = if due {
        steps.push(applied_step(
            "runner_due",
            "approved target cycle is due".to_string(),
            match last_live_cycle_finished_at_unix_ms {
                Some(last_finished) => format!(
                    "last live cycle finished at {last_finished}; cadence is {} minutes",
                    loaded.config.runner.cycle_interval_minutes
                ),
                None => "no prior live cycle recorded".to_string(),
            },
        ));

        let cycle = run_cycle(config_path, dry_run)?;
        steps.push(applied_step(
            "runner_cycle",
            format!(
                "executed approved target cycle ({})",
                describe_action_outcome(cycle.outcome)
            ),
            cycle.summary.clone(),
        ));

        (
            cycle.outcome,
            summarize_runner_tick(true, dry_run, cycle.outcome, &cycle.alerts),
            cycle.preflight_ready,
            Some(cycle.clone()),
            cycle.alerts.clone(),
            cycle.notification.clone(),
        )
    } else {
        let next_due_at_unix_ms = next_due_at_unix_ms;
        let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
        steps.push(skipped_step(
            "runner_due",
            "approved target cycle is not due yet".to_string(),
            match next_due_at_unix_ms {
                Some(next_due) => format!(
                    "next due at {next_due}; cadence is {} minutes",
                    loaded.config.runner.cycle_interval_minutes
                ),
                None => "next due time is not available".to_string(),
            },
        ));

        let notification = if loaded.config.runner.notify_after_tick {
            let report = notify_alerts(config_path, dry_run)?;
            steps.push(match report.outcome {
                ActionOutcome::NoOp => skipped_step(
                    "runner_notify_alerts",
                    "no post-tick notification sent".to_string(),
                    report.summary.clone(),
                ),
                ActionOutcome::Success => applied_step(
                    "runner_notify_alerts",
                    "sent post-tick notification".to_string(),
                    report.summary.clone(),
                ),
                ActionOutcome::Failed => failed_step(
                    "runner_notify_alerts",
                    "failed to send post-tick notification".to_string(),
                    report.summary.clone(),
                ),
                ActionOutcome::Blocked => blocked_step(
                    "runner_notify_alerts",
                    "post-tick notification was blocked".to_string(),
                    report.summary.clone(),
                ),
            });
            Some(report)
        } else {
            steps.push(skipped_step(
                "runner_notify_alerts",
                "post-tick notifications are disabled".to_string(),
                "runner.notify_after_tick is false".to_string(),
            ));
            None
        };

        (
            ActionOutcome::NoOp,
            summarize_runner_tick(false, dry_run, ActionOutcome::NoOp, &alert_report.alerts),
            alert_report.preflight_ready,
            None,
            alert_report.alerts,
            notification,
        )
    };

    let report = RunnerTickReport {
        config_source,
        dry_run,
        outcome,
        summary,
        due,
        cycle_interval_minutes: loaded.config.runner.cycle_interval_minutes,
        last_live_cycle_finished_at_unix_ms,
        next_due_at_unix_ms,
        preflight_ready,
        cycle,
        alerts,
        notification,
        steps,
    };
    record_runner_tick(&loaded.config, &report);
    Ok(report)
}

pub fn runner_agent_status(config_path: Option<&Path>) -> Result<RunnerAgentStatusReport> {
    let loaded = load_config(config_path)?;
    Ok(RunnerAgentStatusReport {
        config_source: loaded.source.description(),
        status: probe_launch_agent(
            &loaded.config.runner.launch_agent.label,
            Some(&loaded.config.runner.launch_agent.plist_path),
        ),
    })
}

pub fn install_runner_agent(
    config_path: Option<&Path>,
    write_only: bool,
) -> Result<RunnerAgentControlReport> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    if !output_path.exists() {
        return Err(anyhow!(
            "config does not exist at {} (create or scaffold it first)",
            output_path.display()
        ));
    }

    let loaded = load_config(Some(output_path.as_path()))?;
    let executable_path = std::env::current_exe()
        .map_err(|error| anyhow!("resolve current executable: {error}"))?;
    let agent = &loaded.config.runner.launch_agent;
    let mut steps = Vec::new();

    if let Some(parent) = agent.plist_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = agent.stdout_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = agent.stderr_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let plist = render_runner_launch_agent_plist(agent, &executable_path, &output_path);
    fs::write(&agent.plist_path, plist).map_err(|error| {
        anyhow!(
            "write runner launch agent plist at {}: {error}",
            agent.plist_path.display()
        )
    })?;
    steps.push(applied_step(
        "write_runner_launch_agent_plist",
        format!("wrote {}", agent.plist_path.display()),
        format!(
            "configured {} to run {} --config {} runner-tick every {} minutes",
            agent.label,
            executable_path.display(),
            output_path.display(),
            agent.tick_interval_minutes
        ),
    ));

    if !write_only {
        let bootout = bootout_launch_agent(agent);
        steps.push(match bootout {
            Some(output) if output.success => applied_step(
                "bootout_runner_launch_agent",
                format!("removed any existing {}", agent.label),
                output.trim_or(&output.stdout).to_string(),
            ),
            Some(output) => skipped_step(
                "bootout_runner_launch_agent",
                format!("no existing {} instance needed removal", agent.label),
                output.trim_or(&output.stdout).to_string(),
            ),
            None => skipped_step(
                "bootout_runner_launch_agent",
                format!("no existing {} instance needed removal", agent.label),
                "launch agent was not loaded".to_string(),
            ),
        });

        let domain = launchctl_domain();
        let plist = agent.plist_path.to_string_lossy().to_string();
        let bootstrap = run_command("launchctl", ["bootstrap", domain.as_str(), plist.as_str()]);
        if bootstrap.success {
            steps.push(applied_step(
                "bootstrap_runner_launch_agent",
                format!("loaded {}", agent.label),
                bootstrap.trim_or(&bootstrap.stdout).to_string(),
            ));
        } else {
            steps.push(failed_step(
                "bootstrap_runner_launch_agent",
                format!("failed to load {}", agent.label),
                bootstrap.trim_or(&bootstrap.stdout).to_string(),
            ));
        }
    } else {
        steps.push(skipped_step(
            "bootstrap_runner_launch_agent",
            format!("left {} written but not loaded", agent.label),
            "install was requested in write-only mode".to_string(),
        ));
    }

    let status = probe_launch_agent(&agent.label, Some(&agent.plist_path));
    let outcome = summarize_runner_agent_outcome(&steps);
    let summary = summarize_runner_agent_control(
        RunnerAgentAction::Install,
        outcome,
        write_only,
        &status,
        &steps,
    );

    Ok(RunnerAgentControlReport {
        config_source: loaded.source.description(),
        action: RunnerAgentAction::Install,
        outcome,
        summary,
        status,
        steps,
    })
}

pub fn uninstall_runner_agent(
    config_path: Option<&Path>,
    keep_plist: bool,
) -> Result<RunnerAgentControlReport> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    if !output_path.exists() {
        return Err(anyhow!(
            "config does not exist at {} (create or scaffold it first)",
            output_path.display()
        ));
    }

    let loaded = load_config(Some(output_path.as_path()))?;
    let agent = &loaded.config.runner.launch_agent;
    let mut steps = Vec::new();

    let bootout = bootout_launch_agent(agent);
    steps.push(match bootout {
        Some(output) if output.success => applied_step(
            "bootout_runner_launch_agent",
            format!("unloaded {}", agent.label),
            output.trim_or(&output.stdout).to_string(),
        ),
        Some(output) => skipped_step(
            "bootout_runner_launch_agent",
            format!("{} was already unloaded", agent.label),
            output.trim_or(&output.stdout).to_string(),
        ),
        None => skipped_step(
            "bootout_runner_launch_agent",
            format!("{} was already unloaded", agent.label),
            "launch agent was not loaded".to_string(),
        ),
    });

    if keep_plist {
        steps.push(skipped_step(
            "remove_runner_launch_agent_plist",
            format!("kept {}", agent.plist_path.display()),
            "uninstall was requested with keep-plist enabled".to_string(),
        ));
    } else if agent.plist_path.exists() {
        fs::remove_file(&agent.plist_path).map_err(|error| {
            anyhow!(
                "remove runner launch agent plist at {}: {error}",
                agent.plist_path.display()
            )
        })?;
        steps.push(applied_step(
            "remove_runner_launch_agent_plist",
            format!("removed {}", agent.plist_path.display()),
            "runner launch agent plist was deleted".to_string(),
        ));
    } else {
        steps.push(skipped_step(
            "remove_runner_launch_agent_plist",
            format!("{} was already absent", agent.plist_path.display()),
            "runner launch agent plist did not exist".to_string(),
        ));
    }

    let status = probe_launch_agent(&agent.label, Some(&agent.plist_path));
    let outcome = summarize_runner_agent_outcome(&steps);
    let summary = summarize_runner_agent_control(
        RunnerAgentAction::Uninstall,
        outcome,
        keep_plist,
        &status,
        &steps,
    );

    Ok(RunnerAgentControlReport {
        config_source: loaded.source.description(),
        action: RunnerAgentAction::Uninstall,
        outcome,
        summary,
        status,
        steps,
    })
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

pub fn ensure_target_ids(config_path: Option<&Path>) -> Result<EnsureTargetIdsReport> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    if !output_path.exists() {
        return Err(anyhow!(
            "config does not exist at {} (create or scaffold it first)",
            output_path.display()
        ));
    }

    let raw = fs::read_to_string(&output_path)?;
    let mut config: AppConfig = toml::from_str(&raw)?;

    if config.managed_targets.is_empty() {
        return Ok(EnsureTargetIdsReport {
            outcome: ActionOutcome::NoOp,
            summary: "no managed targets are configured".to_string(),
            path: output_path,
            assigned_count: 0,
            preserved_count: 0,
            assignments: Vec::new(),
        });
    }

    let mut seen = std::collections::BTreeSet::new();
    let mut assignments = Vec::new();
    let mut preserved_count = 0usize;

    for target in &mut config.managed_targets {
        let normalized = target
            .target_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        let reason = match &normalized {
            Some(target_id) if seen.insert(target_id.clone()) => {
                target.target_id = Some(target_id.clone());
                preserved_count += 1;
                None
            }
            Some(_) => Some(ManagedTargetIdAssignmentReason::Duplicate),
            None => Some(ManagedTargetIdAssignmentReason::Missing),
        };

        if let Some(reason) = reason {
            let target_id = Uuid::now_v7().to_string();
            seen.insert(target_id.clone());
            target.target_id = Some(target_id.clone());
            assignments.push(ManagedTargetIdAssignment {
                target_name: target.name.clone(),
                target_id,
                reason,
            });
        }
    }

    if assignments.is_empty() {
        return Ok(EnsureTargetIdsReport {
            outcome: ActionOutcome::NoOp,
            summary: "all managed targets already have unique IDs".to_string(),
            path: output_path,
            assigned_count: 0,
            preserved_count,
            assignments,
        });
    }

    let encoded = toml::to_string_pretty(&config)?;
    fs::write(&output_path, encoded)?;

    Ok(EnsureTargetIdsReport {
        outcome: ActionOutcome::Success,
        summary: format!(
            "assigned {} managed target IDs in {}",
            assignments.len(),
            output_path.display()
        ),
        path: output_path,
        assigned_count: assignments.len(),
        preserved_count,
        assignments,
    })
}

pub fn add_managed_target(
    config_path: Option<&Path>,
    name: &str,
    local_path: &Path,
    remote_path: &str,
    mode: PolicyMode,
    rationale: Option<&str>,
) -> Result<AddManagedTargetReport> {
    let (output_path, mut config) = load_editable_config(config_path)?;
    let target_name = name.trim();
    if target_name.is_empty() {
        return Err(anyhow!("managed target name must not be empty"));
    }

    let local_path = expand_path(local_path);
    if !local_path.exists() {
        return Err(anyhow!(
            "managed target local path does not exist: {}",
            local_path.display()
        ));
    }
    if !local_path.is_dir() {
        return Err(anyhow!(
            "managed target local path is not a directory: {}",
            local_path.display()
        ));
    }

    let remote_path = remote_path.trim();
    if remote_path.is_empty() {
        return Err(anyhow!("managed target remote path must not be empty"));
    }

    let rationale = rationale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    ensure_managed_target_slot_is_available(&config, target_name, &local_path, remote_path, None)?;

    let target_id = Uuid::now_v7().to_string();
    config.managed_targets.push(ManagedTarget {
        target_id: Some(target_id.clone()),
        name: target_name.to_string(),
        local_path: local_path.clone(),
        remote_path: remote_path.to_string(),
        mode,
        rationale,
    });

    let normalized = write_config(&output_path, config)?;
    let target = resolve_managed_inventory_target(&normalized, &target_id, &output_path)?;

    Ok(AddManagedTargetReport {
        outcome: ActionOutcome::Success,
        summary: format!("added managed target {}", target.name),
        path: output_path,
        target,
    })
}

pub fn relocate_managed_target(
    config_path: Option<&Path>,
    selector: &str,
    local_path: &Path,
    remote_path: Option<&str>,
) -> Result<RelocateManagedTargetReport> {
    let (output_path, mut config) = load_editable_config(config_path)?;
    let selector_path = expand_path(Path::new(selector));
    let target_index = config
        .managed_targets
        .iter()
        .position(|target| managed_target_matches_selector(target, selector, &selector_path))
        .ok_or_else(|| anyhow!("no managed target matched selector {selector}"))?;

    let previous_local_path = config.managed_targets[target_index].local_path.clone();
    let previous_remote_path = config.managed_targets[target_index].remote_path.clone();
    let target_name = config.managed_targets[target_index].name.clone();
    let target_id = config.managed_targets[target_index]
        .target_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());

    let local_path = expand_path(local_path);
    if !local_path.exists() {
        return Err(anyhow!(
            "managed target local path does not exist: {}",
            local_path.display()
        ));
    }
    if !local_path.is_dir() {
        return Err(anyhow!(
            "managed target local path is not a directory: {}",
            local_path.display()
        ));
    }

    let remote_path = match remote_path {
        Some(path) => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(anyhow!("managed target remote path must not be empty"));
            }
            trimmed.to_string()
        }
        None => previous_remote_path.clone(),
    };

    if previous_local_path == local_path && previous_remote_path == remote_path {
        let normalized = normalize_app_config(config)?;
        let target = resolve_managed_inventory_target(&normalized, &target_id, &output_path)?;
        return Ok(RelocateManagedTargetReport {
            outcome: ActionOutcome::NoOp,
            summary: format!(
                "managed target {} is already at the requested location",
                target.name
            ),
            path: output_path,
            selector: selector.to_string(),
            previous_local_path,
            previous_remote_path,
            target,
        });
    }

    ensure_managed_target_slot_is_available(
        &config,
        &target_name,
        &local_path,
        &remote_path,
        Some(&target_id),
    )?;

    config.managed_targets[target_index].target_id = Some(target_id.clone());
    config.managed_targets[target_index].local_path = local_path;
    config.managed_targets[target_index].remote_path = remote_path;

    let normalized = write_config(&output_path, config)?;
    let target = resolve_managed_inventory_target(&normalized, &target_id, &output_path)?;

    Ok(RelocateManagedTargetReport {
        outcome: ActionOutcome::Success,
        summary: format!("relocated managed target {}", target.name),
        path: output_path,
        selector: selector.to_string(),
        previous_local_path,
        previous_remote_path,
        target,
    })
}

fn load_editable_config(config_path: Option<&Path>) -> Result<(PathBuf, AppConfig)> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    if !output_path.exists() {
        return Err(anyhow!(
            "config does not exist at {} (create or scaffold it first)",
            output_path.display()
        ));
    }

    let raw = fs::read_to_string(&output_path)
        .map_err(|error| anyhow!("read config at {}: {error}", output_path.display()))?;
    let config: AppConfig = toml::from_str(&raw)
        .map_err(|error| anyhow!("parse config at {}: {error}", output_path.display()))?;
    Ok((output_path, config))
}

fn write_config(path: &Path, config: AppConfig) -> Result<AppConfig> {
    let normalized = normalize_app_config(config)?;
    let encoded = toml::to_string_pretty(&normalized)?;
    fs::write(path, encoded)?;
    Ok(normalized)
}

fn resolve_inventory_target(
    targets: Vec<crate::model::SyncTargetRecord>,
    selector: &str,
) -> Result<crate::model::SyncTargetRecord> {
    let selector_path = expand_path(Path::new(selector));
    targets
        .into_iter()
        .find(|target| target_matches_selector(target, selector, &selector_path))
        .ok_or_else(|| anyhow!("no sync target matched selector {selector}"))
}

fn resolve_managed_inventory_target(
    config: &AppConfig,
    target_id: &str,
    config_path: &Path,
) -> Result<crate::model::SyncTargetRecord> {
    let inventory =
        build_target_inventory(config, format!("explicit config {}", config_path.display()))?;
    inventory
        .targets
        .into_iter()
        .find(|target| target.target_id.as_deref() == Some(target_id))
        .ok_or_else(|| anyhow!("managed target with id {target_id} was not found after update"))
}

fn ensure_managed_target_slot_is_available(
    config: &AppConfig,
    target_name: &str,
    local_path: &Path,
    remote_path: &str,
    current_target_id: Option<&str>,
) -> Result<()> {
    let current_target_id = current_target_id.unwrap_or_default();
    for target in &config.managed_targets {
        let same_target = target.target_id.as_deref().unwrap_or_default() == current_target_id
            && !current_target_id.is_empty();
        if same_target {
            continue;
        }
        if target.name == target_name {
            return Err(anyhow!(
                "managed target name already exists: {}",
                target_name
            ));
        }
        if target.local_path == local_path {
            return Err(anyhow!(
                "managed target local path already exists: {}",
                local_path.display()
            ));
        }
        if target.remote_path == remote_path {
            return Err(anyhow!(
                "managed target remote path already exists: {}",
                remote_path
            ));
        }
    }

    let normalized = normalize_app_config(config.clone())?;
    let inventory = build_target_inventory(&normalized, "managed target edit".to_string())?;
    for target in inventory.targets {
        let same_target = target.target_id.as_deref().unwrap_or_default() == current_target_id;
        if same_target && !current_target_id.is_empty() {
            continue;
        }
        if target.name == target_name {
            return Err(anyhow!("sync target name already exists: {}", target_name));
        }
        if target.local_path == local_path {
            return Err(anyhow!(
                "sync target local path already exists: {}",
                local_path.display()
            ));
        }
        if target.remote_path == remote_path {
            return Err(anyhow!(
                "sync target remote path already exists: {}",
                remote_path
            ));
        }
    }
    Ok(())
}

fn target_matches_selector(
    target: &crate::model::SyncTargetRecord,
    selector: &str,
    selector_path: &Path,
) -> bool {
    target.target_id.as_deref() == Some(selector)
        || target.name == selector
        || target.local_path == selector_path
}

fn managed_target_matches_selector(
    target: &ManagedTarget,
    selector: &str,
    selector_path: &Path,
) -> bool {
    target.target_id.as_deref() == Some(selector)
        || target.name == selector
        || target.local_path == selector_path
}

fn collect_status(config: &AppConfig, config_source: String) -> StatusReport {
    let acknowledged_log = load_state(&config.state_path)
        .ok()
        .and_then(|state| state.acknowledged_log);
    let launch_agent = probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
    let runner_agent = probe_launch_agent(
        &config.runner.launch_agent.label,
        Some(&config.runner.launch_agent.plist_path),
    );
    let remote = probe_remote_service(config);
    let artifacts = scan_artifacts(config);
    let latest_log = summarize_latest_log(&config.rclone_log_dir, config.scan.max_examples);
    let policy = PolicySummary {
        folder_policies: config.policy.folders.clone(),
        file_class_policies: config.policy.file_classes.clone(),
        target_exclusions: config.policy.target_exclusions.clone(),
        target_snapshots: config.policy.target_snapshots.clone(),
    };

    StatusReport {
        config_source,
        policy,
        launch_agent,
        runner_agent,
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

fn evaluate_target(
    preflight: &PreflightReport,
    target: crate::model::SyncTargetRecord,
) -> TargetEvaluation {
    let effective_mode = target.configured_mode.unwrap_or(target.recommended_mode);
    let mut blockers = Vec::new();

    for check in &preflight.checks {
        if check.status == CheckStatus::Fail {
            blockers.push(TargetBlocker {
                id: format!("preflight_{}", check.id),
                summary: check.summary.clone(),
                detail: check.detail.clone(),
            });
        }
    }

    match effective_mode {
        crate::config::PolicyMode::Hold => blockers.push(TargetBlocker {
            id: "policy_hold".to_string(),
            summary: format!("{} is still on hold", target.name),
            detail: "this folder has not been approved for re-enablement yet".to_string(),
        }),
        crate::config::PolicyMode::Excluded => blockers.push(TargetBlocker {
            id: "policy_excluded".to_string(),
            summary: format!("{} is excluded from managed sync", target.name),
            detail: "this target needs a dedicated workflow outside broad folder sync".to_string(),
        }),
        _ => {}
    }

    if !target.local_path.exists() {
        blockers.push(TargetBlocker {
            id: "local_path_missing".to_string(),
            summary: format!("{} does not exist locally", target.local_path.display()),
            detail: format!(
                "target {} cannot run until the local path exists",
                target.name
            ),
        });
    }

    TargetEvaluation {
        target,
        effective_mode,
        ready: blockers.is_empty(),
        blockers,
    }
}

fn evaluate_alerts(config: &AppConfig, config_source: String) -> Result<AlertReport> {
    let status = collect_status(config, config_source.clone());
    let preflight = evaluate_preflight(status);
    let inventory = build_target_inventory(config, config_source.clone())?;
    let state = load_state(&config.state_path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let stale_after_ms = (config.alerts.stale_success_after_hours as u128) * 60 * 60 * 1000;

    let mut alerts = Vec::new();

    for check in &preflight.checks {
        if check.status == CheckStatus::Fail {
            alerts.push(AlertRecord {
                id: format!("preflight_{}", check.id),
                severity: AlertSeverity::Critical,
                summary: check.summary.clone(),
                detail: check.detail.clone(),
                target_name: None,
            });
        }
    }

    for target in inventory.targets {
        let evaluation = evaluate_target(&preflight, target);
        if evaluation.effective_mode != crate::config::PolicyMode::BackupOnly {
            continue;
        }

        if !evaluation.ready {
            alerts.push(AlertRecord {
                id: format!("target_{}_blocked", evaluation.target.name),
                severity: AlertSeverity::Warn,
                summary: format!("{} cannot run yet", evaluation.target.name),
                detail: format_target_blockers(&evaluation.blockers),
                target_name: Some(evaluation.target.name.clone()),
            });
            continue;
        }

        let Some(run_state) = lookup_target_run_state(&state, &evaluation.target) else {
            alerts.push(AlertRecord {
                id: format!("target_{}_never_ran", evaluation.target.name),
                severity: AlertSeverity::Warn,
                summary: format!("{} has no recorded run history", evaluation.target.name),
                detail: "run-target has not completed for this executable target yet".to_string(),
                target_name: Some(evaluation.target.name.clone()),
            });
            continue;
        };

        if run_state.outcome != ActionOutcome::Success {
            alerts.push(AlertRecord {
                id: format!("target_{}_last_run_failed", evaluation.target.name),
                severity: AlertSeverity::Warn,
                summary: format!(
                    "{} last completed with {:?}",
                    evaluation.target.name, run_state.outcome
                ),
                detail: run_state.summary.clone(),
                target_name: Some(evaluation.target.name.clone()),
            });
            continue;
        }

        let Some(last_success_at) = run_state.last_success_at_unix_ms else {
            alerts.push(AlertRecord {
                id: format!("target_{}_no_live_success", evaluation.target.name),
                severity: AlertSeverity::Warn,
                summary: format!("{} has no non-dry-run success yet", evaluation.target.name),
                detail: "dry runs do not count as completed backups for stale-success tracking"
                    .to_string(),
                target_name: Some(evaluation.target.name.clone()),
            });
            continue;
        };

        if now.saturating_sub(last_success_at) > stale_after_ms {
            alerts.push(AlertRecord {
                id: format!("target_{}_stale_success", evaluation.target.name),
                severity: AlertSeverity::Warn,
                summary: format!(
                    "{} has not completed a successful live run in {} hours",
                    evaluation.target.name, config.alerts.stale_success_after_hours
                ),
                detail: format!(
                    "last successful live run recorded at unix_ms {}",
                    last_success_at
                ),
                target_name: Some(evaluation.target.name.clone()),
            });
        }
    }

    Ok(AlertReport {
        config_source,
        generated_at_unix_ms: now,
        preflight_ready: preflight.ready,
        stale_success_after_hours: config.alerts.stale_success_after_hours,
        alerts,
    })
}

struct LegacyLockGuard {
    path: PathBuf,
}

impl Drop for LegacyLockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_legacy_lock(
    config: &AppConfig,
    slot: &mut Option<LegacyLockGuard>,
) -> Result<ActionStep> {
    let lock_path = &config.legacy_lock_path;
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if lock_path.exists() {
        let pid = fs::read_to_string(lock_path)
            .unwrap_or_default()
            .trim()
            .to_string();
        if !pid.is_empty() {
            let running = run_command("ps", ["-p", pid.as_str()]);
            if running.success {
                return Ok(blocked_step(
                    "legacy_lock",
                    format!(
                        "legacy sync lock is still active at {}",
                        lock_path.display()
                    ),
                    format!("process {} still owns the legacy sync lock", pid),
                ));
            }
        }
        let _ = fs::remove_file(lock_path);
    }

    fs::write(lock_path, std::process::id().to_string())?;
    *slot = Some(LegacyLockGuard {
        path: lock_path.clone(),
    });

    Ok(applied_step(
        "legacy_lock",
        format!("acquired legacy sync lock {}", lock_path.display()),
        "single-target execution is now protected from concurrent legacy runs".to_string(),
    ))
}

fn make_temp_workdir(target_name: &str) -> Result<PathBuf> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let safe_name = target_name.replace('/', "_").replace(' ', "_");
    let path = std::env::temp_dir().join(format!("syncsteward-{}-{}", safe_name, now));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn execute_backup_only_target(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    host: &str,
    dry_run: bool,
    temp_dir: &Path,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let rclone_config_path = write_target_rclone_config(config, host, temp_dir)?;
    steps.push(applied_step(
        "write_rclone_config",
        format!("wrote temporary rclone config for {}", target.name),
        rclone_config_path.display().to_string(),
    ));

    let filter_path = build_filter_file(config, target, temp_dir)?;
    steps.push(applied_step(
        "prepare_filters",
        format!("prepared filter rules for {}", target.name),
        filter_path.display().to_string(),
    ));

    let remote_path = format!("syncsteward-target:{}", target.remote_path);
    let mut command = Command::new("rclone");
    command
        .env("RCLONE_CONFIG", &rclone_config_path)
        .arg("sync")
        .arg(&target.local_path)
        .arg(&remote_path)
        .arg("--filter-from")
        .arg(&filter_path)
        .arg("--skip-links")
        .arg("--exclude")
        .arg("*.db-journal")
        .arg("--exclude")
        .arg("*.db-wal")
        .arg("--exclude")
        .arg("*.db-shm");
    if dry_run {
        command.arg("--dry-run");
    }

    let output = match command.output() {
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
    };

    if output.success {
        steps.push(applied_step(
            "rclone_sync",
            format!(
                "{} completed for {}",
                if dry_run {
                    "dry run"
                } else {
                    "backup-only sync"
                },
                target.name
            ),
            summarize_command_output(&output),
        ));
        Ok(())
    } else {
        steps.push(failed_step(
            "rclone_sync",
            format!(
                "{} failed for {}",
                if dry_run {
                    "dry run"
                } else {
                    "backup-only sync"
                },
                target.name
            ),
            summarize_command_output(&output),
        ));
        Ok(())
    }
}

fn execute_snapshot_backup_target(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    snapshot_policy: &crate::config::TargetSnapshot,
    host: &str,
    dry_run: bool,
    temp_dir: &Path,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let rclone_config_path = write_target_rclone_config(config, host, temp_dir)?;
    steps.push(applied_step(
        "write_rclone_config",
        format!("wrote temporary rclone config for {}", target.name),
        rclone_config_path.display().to_string(),
    ));

    let filter_path = build_filter_file(config, target, temp_dir)?;
    steps.push(applied_step(
        "prepare_filters",
        format!("prepared filter rules for {}", target.name),
        filter_path.display().to_string(),
    ));

    let remote_path = format!("syncsteward-target:{}", target.remote_path);
    let mut command = Command::new("rclone");
    command
        .env("RCLONE_CONFIG", &rclone_config_path)
        .arg("sync")
        .arg(&target.local_path)
        .arg(&remote_path)
        .arg("--filter-from")
        .arg(&filter_path)
        .arg("--skip-links")
        .arg("--exclude")
        .arg("*.db")
        .arg("--exclude")
        .arg("*.sqlite")
        .arg("--exclude")
        .arg("*.sqlite3")
        .arg("--exclude")
        .arg("*.db-journal")
        .arg("--exclude")
        .arg("*.db-wal")
        .arg("--exclude")
        .arg("*.db-shm")
        .arg("--exclude")
        .arg("*.sqlite-journal")
        .arg("--exclude")
        .arg("*.sqlite-wal")
        .arg("--exclude")
        .arg("*.sqlite-shm")
        .arg("--exclude")
        .arg("*.sqlite3-journal")
        .arg("--exclude")
        .arg("*.sqlite3-wal")
        .arg("--exclude")
        .arg("*.sqlite3-shm");
    if dry_run {
        command.arg("--dry-run");
    }

    let non_db_output = run_spawned_command(command);
    if non_db_output.success {
        steps.push(applied_step(
            "rclone_sync_non_db",
            format!(
                "{} completed for non-database files in {}",
                if dry_run {
                    "dry run"
                } else {
                    "backup-only sync"
                },
                target.name
            ),
            summarize_command_output(&non_db_output),
        ));
    } else {
        steps.push(failed_step(
            "rclone_sync_non_db",
            format!(
                "{} failed for non-database files in {}",
                if dry_run {
                    "dry run"
                } else {
                    "backup-only sync"
                },
                target.name
            ),
            summarize_command_output(&non_db_output),
        ));
        return Ok(());
    }

    let snapshot_root = temp_dir.join("sqlite-snapshots");
    fs::create_dir_all(&snapshot_root)?;

    let mut snapshot_paths = Vec::new();
    for relative_path in &snapshot_policy.sqlite_paths {
        let source_path = target.local_path.join(relative_path);
        if !source_path.exists() {
            steps.push(skipped_step(
                "sqlite_snapshot_missing",
                format!(
                    "skipped missing SQLite source {} for {}",
                    relative_path.display(),
                    target.name
                ),
                source_path.display().to_string(),
            ));
            continue;
        }

        let destination_path = snapshot_root.join(relative_path);
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let backup_output = run_command(
            "sqlite3",
            [
                source_path.to_string_lossy().as_ref(),
                ".timeout 5000",
                &format!(
                    ".backup '{}'",
                    sqlite_string_literal(destination_path.as_path())
                ),
            ],
        );
        if backup_output.success {
            steps.push(applied_step(
                "sqlite_snapshot_backup",
                format!("created SQLite snapshot for {}", relative_path.display()),
                destination_path.display().to_string(),
            ));
            snapshot_paths.push((relative_path.clone(), destination_path));
        } else {
            steps.push(failed_step(
                "sqlite_snapshot_backup",
                format!("failed to snapshot SQLite file {}", relative_path.display()),
                summarize_command_output(&backup_output),
            ));
        }
    }

    if steps
        .iter()
        .any(|step| step.id == "sqlite_snapshot_backup" && step.status == ActionStepStatus::Failed)
    {
        return Ok(());
    }

    for (relative_path, snapshot_path) in snapshot_paths {
        let remote_file = format!(
            "syncsteward-target:{}/{}",
            target.remote_path,
            relative_path.to_string_lossy().replace('\\', "/")
        );
        let mut upload = Command::new("rclone");
        upload
            .env("RCLONE_CONFIG", &rclone_config_path)
            .arg("copyto")
            .arg(&snapshot_path)
            .arg(&remote_file);
        if dry_run {
            upload.arg("--dry-run");
        }
        let upload_output = run_spawned_command(upload);
        if upload_output.success {
            steps.push(applied_step(
                "sqlite_snapshot_upload",
                format!("uploaded SQLite snapshot for {}", relative_path.display()),
                summarize_command_output(&upload_output),
            ));
        } else {
            steps.push(failed_step(
                "sqlite_snapshot_upload",
                format!(
                    "failed to upload SQLite snapshot for {}",
                    relative_path.display()
                ),
                summarize_command_output(&upload_output),
            ));
        }
    }

    Ok(())
}

fn write_target_rclone_config(config: &AppConfig, host: &str, temp_dir: &Path) -> Result<PathBuf> {
    let path = temp_dir.join("rclone.conf");
    let contents = format!(
        "[syncsteward-target]\n\
type = sftp\n\
host = {host}\n\
port = 22\n\
user = {user}\n\
key_file = {key}\n\
shell_type = unix\n\
md5sum_command = md5sum\n\
sha1sum_command = sha1sum\n",
        user = config.remote.ssh_user,
        key = config.ssh_key_path.display()
    );
    fs::write(&path, contents)?;
    Ok(path)
}

fn build_filter_file(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    temp_dir: &Path,
) -> Result<PathBuf> {
    let target_exclusions = target_exclusion_lines(config, &target.name);
    if target.name != ".memloft" && target_exclusions.is_empty() {
        return Ok(config.sync_filter_path.clone());
    }

    let base = fs::read_to_string(&config.sync_filter_path)?;
    let merged_path = temp_dir.join("filters.txt");
    let mut sections = vec![base.trim_end().to_string()];

    if target.name == ".memloft" {
        sections.push(fs::read_to_string(&config.memloft_filter_path)?);
    }

    if !target_exclusions.is_empty() {
        sections.push(target_exclusions.join("\n"));
    }

    let merged = sections.join("\n");
    fs::write(&merged_path, merged)?;
    Ok(merged_path)
}

fn target_exclusion_lines(config: &AppConfig, target_name: &str) -> Vec<String> {
    config
        .policy
        .target_exclusions
        .iter()
        .filter(|entry| entry.target == target_name)
        .flat_map(|entry| entry.patterns.iter())
        .map(|pattern| format!("- {pattern}"))
        .collect()
}

fn target_snapshot_policy<'a>(
    config: &'a AppConfig,
    target_name: &str,
) -> Option<&'a crate::config::TargetSnapshot> {
    config
        .policy
        .target_snapshots
        .iter()
        .find(|entry| entry.target == target_name)
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
    let launch_agent = probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
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
    let launch_agent = probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
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

fn record_target_run(config: &AppConfig, report: &TargetRunReport) {
    if let Err(error) = append_target_run_audit(&config.audit_log_path, report) {
        eprintln!("syncsteward: failed to append target run audit: {error}");
    }

    if report.dry_run {
        return;
    }

    let finished_at_unix_ms = now_unix_ms();

    let state = TargetRunState {
        target_name: report.evaluation.target.name.clone(),
        target_id: report.evaluation.target.target_id.clone(),
        local_path: report.evaluation.target.local_path.clone(),
        effective_mode: report.evaluation.effective_mode,
        outcome: report.outcome,
        dry_run: false,
        finished_at_unix_ms,
        last_success_at_unix_ms: if report.outcome == ActionOutcome::Success {
            Some(finished_at_unix_ms)
        } else {
            None
        },
        summary: report.summary.clone(),
    };

    let state_key = target_state_key(&report.evaluation.target);
    if let Err(error) = save_target_run(&config.state_path, &state_key, state) {
        eprintln!("syncsteward: failed to record target run state: {error}");
    }
}

fn record_cycle_run(config: &AppConfig, report: &RunCycleReport, started_at_unix_ms: u128) {
    let finished_at_unix_ms = now_unix_ms();
    if let Err(error) = append_cycle_run_audit(&config.audit_log_path, report) {
        eprintln!("syncsteward: failed to append cycle audit: {error}");
    }

    let state = RunnerCycleState {
        dry_run: report.dry_run,
        started_at_unix_ms,
        finished_at_unix_ms,
        outcome: report.outcome,
        approved_target_count: report.approved_target_count,
        active_alert_count: report.alerts.len(),
        summary: report.summary.clone(),
    };

    let last_live_cycle_finished_at_unix_ms = if !report.dry_run {
        Some(finished_at_unix_ms)
    } else {
        None
    };

    if let Err(error) =
        save_runner_cycle(&config.state_path, state, last_live_cycle_finished_at_unix_ms)
    {
        eprintln!("syncsteward: failed to record cycle state: {error}");
    }
}

fn record_runner_tick(config: &AppConfig, report: &RunnerTickReport) {
    let finished_at_unix_ms = now_unix_ms();
    if let Err(error) = append_runner_tick_audit(&config.audit_log_path, report) {
        eprintln!("syncsteward: failed to append runner tick audit: {error}");
    }

    let state = RunnerTickState {
        dry_run: report.dry_run,
        finished_at_unix_ms,
        due: report.due,
        outcome: report.outcome,
        next_due_at_unix_ms: report.next_due_at_unix_ms,
        summary: report.summary.clone(),
    };

    if let Err(error) = save_runner_tick(&config.state_path, state) {
        eprintln!("syncsteward: failed to record runner tick state: {error}");
    }
}

fn target_state_key(target: &crate::model::SyncTargetRecord) -> String {
    target
        .target_id
        .clone()
        .unwrap_or_else(|| target.name.clone())
}

fn lookup_target_run_state<'a>(
    state: &'a crate::state::AppState,
    target: &crate::model::SyncTargetRecord,
) -> Option<&'a TargetRunState> {
    if let Some(target_id) = &target.target_id {
        if let Some(run_state) = state.target_runs.get(target_id) {
            return Some(run_state);
        }
    }

    state.target_runs.get(&target.name)
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

fn append_target_run_audit(path: &Path, report: &TargetRunReport) -> Result<()> {
    #[derive(Serialize)]
    struct TargetRunAuditRecord<'a> {
        timestamp_unix_ms: u128,
        kind: &'static str,
        selector: &'a str,
        target_name: &'a str,
        dry_run: bool,
        outcome: ActionOutcome,
        summary: &'a str,
        effective_mode: crate::config::PolicyMode,
        blocker_ids: Vec<&'a str>,
        step_ids: Vec<&'a str>,
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let blocker_ids = report
        .evaluation
        .blockers
        .iter()
        .map(|blocker| blocker.id.as_str())
        .collect::<Vec<_>>();
    let step_ids = report
        .steps
        .iter()
        .map(|step| step.id.as_str())
        .collect::<Vec<_>>();

    let record = TargetRunAuditRecord {
        timestamp_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        kind: "target_run",
        selector: &report.selector,
        target_name: &report.evaluation.target.name,
        dry_run: report.dry_run,
        outcome: report.outcome,
        summary: &report.summary,
        effective_mode: report.evaluation.effective_mode,
        blocker_ids,
        step_ids,
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn append_cycle_run_audit(path: &Path, report: &RunCycleReport) -> Result<()> {
    #[derive(Serialize)]
    struct CycleRunAuditRecord<'a> {
        timestamp_unix_ms: u128,
        kind: &'static str,
        dry_run: bool,
        outcome: ActionOutcome,
        summary: &'a str,
        approved_target_count: usize,
        target_run_count: usize,
        skipped_target_count: usize,
        alert_count: usize,
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let record = CycleRunAuditRecord {
        timestamp_unix_ms: now_unix_ms(),
        kind: "cycle_run",
        dry_run: report.dry_run,
        outcome: report.outcome,
        summary: &report.summary,
        approved_target_count: report.approved_target_count,
        target_run_count: report.target_runs.len(),
        skipped_target_count: report.skipped_targets.len(),
        alert_count: report.alerts.len(),
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn append_runner_tick_audit(path: &Path, report: &RunnerTickReport) -> Result<()> {
    #[derive(Serialize)]
    struct RunnerTickAuditRecord<'a> {
        timestamp_unix_ms: u128,
        kind: &'static str,
        dry_run: bool,
        due: bool,
        outcome: ActionOutcome,
        summary: &'a str,
        cycle_interval_minutes: u64,
        preflight_ready: bool,
        cycle_ran: bool,
        alert_count: usize,
        step_ids: Vec<&'a str>,
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let step_ids = report
        .steps
        .iter()
        .map(|step| step.id.as_str())
        .collect::<Vec<_>>();

    let record = RunnerTickAuditRecord {
        timestamp_unix_ms: now_unix_ms(),
        kind: "runner_tick",
        dry_run: report.dry_run,
        due: report.due,
        outcome: report.outcome,
        summary: &report.summary,
        cycle_interval_minutes: report.cycle_interval_minutes,
        preflight_ready: report.preflight_ready,
        cycle_ran: report.cycle.is_some(),
        alert_count: report.alerts.len(),
        step_ids,
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn probe_launch_agent(label: &str, plist_path: Option<&Path>) -> LaunchAgentStatus {
    let installed = plist_path.is_some_and(Path::exists);
    let output = run_command("launchctl", ["list"]);
    if !output.success {
        return LaunchAgentStatus {
            label: label.to_string(),
            plist_path: plist_path.map(Path::to_path_buf),
            installed,
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
                plist_path: plist_path.map(Path::to_path_buf),
                installed,
                loaded: true,
                running,
                detail: line.trim().to_string(),
            }
        }
        None => LaunchAgentStatus {
            label: label.to_string(),
            plist_path: plist_path.map(Path::to_path_buf),
            installed,
            loaded: false,
            running: false,
            detail: if installed {
                "launchctl list does not contain the label".to_string()
            } else {
                "launch agent plist does not exist yet".to_string()
            },
        },
    }
}

fn render_runner_launch_agent_plist(
    agent: &RunnerLaunchAgentConfig,
    executable_path: &Path,
    config_path: &Path,
) -> String {
    let interval_seconds = agent.tick_interval_minutes * 60;
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\">\n",
            "<dict>\n",
            "  <key>Label</key>\n",
            "  <string>{label}</string>\n",
            "  <key>ProgramArguments</key>\n",
            "  <array>\n",
            "    <string>{program}</string>\n",
            "    <string>--config</string>\n",
            "    <string>{config}</string>\n",
            "    <string>runner-tick</string>\n",
            "  </array>\n",
            "  <key>RunAtLoad</key>\n",
            "  <{run_at_load}/>\n",
            "  <key>StartInterval</key>\n",
            "  <integer>{interval_seconds}</integer>\n",
            "  <key>StandardOutPath</key>\n",
            "  <string>{stdout_path}</string>\n",
            "  <key>StandardErrorPath</key>\n",
            "  <string>{stderr_path}</string>\n",
            "</dict>\n",
            "</plist>\n"
        ),
        label = plist_xml_escape(&agent.label),
        program = plist_xml_escape(&executable_path.to_string_lossy()),
        config = plist_xml_escape(&config_path.to_string_lossy()),
        run_at_load = if agent.run_at_load { "true" } else { "false" },
        interval_seconds = interval_seconds,
        stdout_path = plist_xml_escape(&agent.stdout_path.to_string_lossy()),
        stderr_path = plist_xml_escape(&agent.stderr_path.to_string_lossy()),
    )
}

fn plist_xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn launchctl_domain() -> String {
    format!("gui/{}", current_uid())
}

fn bootout_launch_agent(agent: &RunnerLaunchAgentConfig) -> Option<CommandOutput> {
    let status = probe_launch_agent(&agent.label, Some(&agent.plist_path));
    if !status.loaded {
        return None;
    }

    let domain = launchctl_domain();
    let plist = agent.plist_path.to_string_lossy().to_string();
    let primary = run_command("launchctl", ["bootout", domain.as_str(), plist.as_str()]);
    if primary.success {
        Some(primary)
    } else {
        Some(run_command("launchctl", ["unload", plist.as_str()]))
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

    let mut candidate_roots = config.scan.roots.clone();
    for target in &config.managed_targets {
        if !candidate_roots.contains(&target.local_path) {
            candidate_roots.push(target.local_path.clone());
        }
    }

    for root in &candidate_roots {
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

fn summarize_run_outcome(steps: &[ActionStep]) -> ActionOutcome {
    if steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Failed)
    {
        ActionOutcome::Failed
    } else if steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Blocked)
    {
        ActionOutcome::Blocked
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

fn summarize_runner_agent_outcome(steps: &[ActionStep]) -> ActionOutcome {
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

fn summarize_runner_agent_control(
    action: RunnerAgentAction,
    outcome: ActionOutcome,
    passive_mode: bool,
    status: &LaunchAgentStatus,
    _steps: &[ActionStep],
) -> String {
    match action {
        RunnerAgentAction::Install => match outcome {
            ActionOutcome::Success => {
                if passive_mode {
                    format!(
                        "wrote {} without loading it",
                        status
                            .plist_path
                            .as_ref()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| status.label.clone())
                    )
                } else {
                    format!("installed and loaded {}", status.label)
                }
            }
            ActionOutcome::NoOp => format!("{} was already installed", status.label),
            ActionOutcome::Blocked => format!("installation blocked for {}", status.label),
            ActionOutcome::Failed => format!("failed to install {}", status.label),
        },
        RunnerAgentAction::Uninstall => match outcome {
            ActionOutcome::Success => {
                if passive_mode {
                    format!("unloaded {} and kept its plist", status.label)
                } else {
                    format!("uninstalled {}", status.label)
                }
            }
            ActionOutcome::NoOp => format!("{} was already absent", status.label),
            ActionOutcome::Blocked => format!("uninstall blocked for {}", status.label),
            ActionOutcome::Failed => format!("failed to uninstall {}", status.label),
        },
    }
}

fn summarize_target_run(
    target_name: &str,
    dry_run: bool,
    outcome: ActionOutcome,
    steps: &[ActionStep],
) -> String {
    let mode = if dry_run { "dry run" } else { "run" };
    match outcome {
        ActionOutcome::Success => format!("{mode} succeeded for {target_name}"),
        ActionOutcome::NoOp => format!("{mode} made no changes for {target_name}"),
        ActionOutcome::Blocked => format!("{mode} blocked for {target_name}"),
        ActionOutcome::Failed => {
            let failed = steps
                .iter()
                .filter(|step| step.status == ActionStepStatus::Failed)
                .count();
            format!("{mode} failed for {target_name} ({failed} failed steps)")
        }
    }
}

fn summarize_cycle_outcome(
    target_runs: &[TargetRunReport],
    skipped_targets: &[CycleSkippedTarget],
    notification: Option<&NotifyAlertsReport>,
) -> ActionOutcome {
    if target_runs
        .iter()
        .any(|report| report.outcome == ActionOutcome::Failed)
        || notification.is_some_and(|report| report.outcome == ActionOutcome::Failed)
    {
        ActionOutcome::Failed
    } else if !skipped_targets.is_empty()
        || target_runs
            .iter()
            .any(|report| report.outcome == ActionOutcome::Blocked)
    {
        ActionOutcome::Blocked
    } else if target_runs.is_empty()
        || target_runs
            .iter()
            .all(|report| report.outcome == ActionOutcome::NoOp)
    {
        ActionOutcome::NoOp
    } else {
        ActionOutcome::Success
    }
}

fn summarize_cycle_report(
    approved_selectors: &[String],
    target_runs: &[TargetRunReport],
    skipped_targets: &[CycleSkippedTarget],
    alerts: &[AlertRecord],
    dry_run: bool,
    outcome: ActionOutcome,
) -> String {
    let mode = if dry_run { "dry run" } else { "cycle" };
    match outcome {
        ActionOutcome::Success => format!(
            "{mode} succeeded for {} approved targets ({} active alerts)",
            target_runs.len(),
            alerts.len()
        ),
        ActionOutcome::NoOp => {
            if approved_selectors.is_empty() {
                "no approved targets are configured for cycle execution".to_string()
            } else {
                format!(
                    "{mode} made no changes for {} approved targets",
                    approved_selectors.len()
                )
            }
        }
        ActionOutcome::Blocked => format!(
            "{mode} blocked for {} approved targets ({} skipped, {} active alerts)",
            approved_selectors.len(),
            skipped_targets.len(),
            alerts.len()
        ),
        ActionOutcome::Failed => format!(
            "{mode} failed for {} approved targets",
            approved_selectors.len()
        ),
    }
}

fn summarize_runner_tick(
    due: bool,
    dry_run: bool,
    outcome: ActionOutcome,
    alerts: &[AlertRecord],
) -> String {
    if due {
        let mode = if dry_run { "dry run tick" } else { "tick" };
        match outcome {
            ActionOutcome::Success => format!(
                "{mode} executed approved cycle successfully ({} active alerts)",
                alerts.len()
            ),
            ActionOutcome::NoOp => format!(
                "{mode} executed approved cycle with no changes ({} active alerts)",
                alerts.len()
            ),
            ActionOutcome::Blocked => format!(
                "{mode} executed approved cycle but it was blocked ({} active alerts)",
                alerts.len()
            ),
            ActionOutcome::Failed => format!(
                "{mode} executed approved cycle but it failed ({} active alerts)",
                alerts.len()
            ),
        }
    } else {
        format!(
            "runner tick skipped cycle because it is not due ({} active alerts)",
            alerts.len()
        )
    }
}

fn runner_due_status(
    last_live_cycle_finished_at_unix_ms: Option<u128>,
    interval_ms: u128,
    now_unix_ms: u128,
) -> (bool, Option<u128>) {
    let next_due_at_unix_ms =
        last_live_cycle_finished_at_unix_ms.map(|finished| finished.saturating_add(interval_ms));
    let due = next_due_at_unix_ms.is_none_or(|next_due| now_unix_ms >= next_due);
    (due, next_due_at_unix_ms)
}

fn describe_action_outcome(outcome: ActionOutcome) -> &'static str {
    match outcome {
        ActionOutcome::Success => "success",
        ActionOutcome::NoOp => "no_op",
        ActionOutcome::Blocked => "blocked",
        ActionOutcome::Failed => "failed",
    }
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
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

fn format_target_blockers(blockers: &[TargetBlocker]) -> String {
    if blockers.is_empty() {
        "no blockers recorded".to_string()
    } else {
        blockers
            .iter()
            .map(|blocker| format!("{}: {}", blocker.id, blocker.summary))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn summarize_alerts_notification(alerts: &[AlertRecord]) -> String {
    let mut items = alerts
        .iter()
        .take(3)
        .map(|alert| alert.summary.clone())
        .collect::<Vec<_>>();
    if alerts.len() > 3 {
        items.push(format!("and {} more", alerts.len() - 3));
    }
    items.join("; ")
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

fn apple_script_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn summarize_command_output(output: &CommandOutput) -> String {
    let source = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    if source.is_empty() {
        return "command completed without output".to_string();
    }

    let lines = source.lines().collect::<Vec<_>>();
    let start = lines.len().saturating_sub(20);
    let mut summary = lines[start..].join("\n");
    if start > 0 {
        summary = format!("...truncated...\n{summary}");
    }
    if summary.len() > 4000 {
        summary.truncate(4000);
        summary.push_str("\n...truncated...");
    }
    summary
}

fn sqlite_string_literal(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn action_name(action: ControlAction) -> &'static str {
    match action {
        ControlAction::Pause => "pause",
        ControlAction::Resume => "resume",
    }
}

fn describe_policy_mode(mode: crate::config::PolicyMode) -> &'static str {
    match mode {
        crate::config::PolicyMode::TwoWayCurated => "two-way curated",
        crate::config::PolicyMode::BackupOnly => "backup only",
        crate::config::PolicyMode::Excluded => "excluded",
        crate::config::PolicyMode::Hold => "hold",
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

fn run_spawned_command(mut command: Command) -> CommandOutput {
    match command.output() {
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
        ActionOutcome, ActionStepStatus, add_managed_target, analyze_log_contents,
        ensure_target_ids, evaluate_preflight, relocate_managed_target, runner_due_status,
        summarize_outcome, target_state_key,
    };
    use crate::config::{AppConfig, ManagedTarget, PolicyConfig, PolicyMode, load_config};
    use crate::model::{
        AcknowledgedLogSummary, ActionStep, ArtifactReport, CheckStatus, LaunchAgentStatus,
        PolicySummary, RemoteStatus, ServiceState, StatusReport, SyncTargetRecord,
    };
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

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
                target_exclusions: PolicyConfig::default().target_exclusions,
                target_snapshots: PolicyConfig::default().target_snapshots,
            },
            launch_agent: LaunchAgentStatus {
                label: "com.example.test".to_string(),
                plist_path: Some(PathBuf::from("/tmp/com.example.test.plist")),
                installed: true,
                loaded: false,
                running: false,
                detail: "not loaded".to_string(),
            },
            runner_agent: LaunchAgentStatus {
                label: "com.example.runner".to_string(),
                plist_path: Some(PathBuf::from("/tmp/com.example.runner.plist")),
                installed: false,
                loaded: false,
                running: false,
                detail: "not installed".to_string(),
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

    #[test]
    fn target_state_key_prefers_target_id_when_present() {
        let with_id = SyncTargetRecord {
            target_id: Some("target-123".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: PathBuf::from("/tmp/notes"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            legacy_mode: crate::model::LegacySyncMode::Managed,
            recommended_mode: PolicyMode::BackupOnly,
            configured_mode: Some(PolicyMode::BackupOnly),
            rationale: "test".to_string(),
        };
        let without_id = SyncTargetRecord {
            target_id: None,
            name: "Notes/Personal".to_string(),
            local_path: PathBuf::from("/tmp/notes"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            legacy_mode: crate::model::LegacySyncMode::Managed,
            recommended_mode: PolicyMode::BackupOnly,
            configured_mode: Some(PolicyMode::BackupOnly),
            rationale: "test".to_string(),
        };

        assert_eq!(target_state_key(&with_id), "target-123");
        assert_eq!(target_state_key(&without_id), "Notes/Personal");
    }

    #[test]
    fn ensure_target_ids_assigns_missing_ids() {
        let temp_path =
            std::env::temp_dir().join(format!("syncsteward-test-{}.toml", Uuid::now_v7()));
        let mut config = AppConfig::default();
        config.managed_targets = vec![
            ManagedTarget {
                target_id: None,
                name: "Notes/Personal".to_string(),
                local_path: PathBuf::from("~/Notes/Personal"),
                remote_path: "OneDrive/Notes/Personal".to_string(),
                mode: PolicyMode::BackupOnly,
                rationale: None,
            },
            ManagedTarget {
                target_id: None,
                name: "Notes/Business".to_string(),
                local_path: PathBuf::from("~/Notes/Business"),
                remote_path: "OneDrive/Notes/Business".to_string(),
                mode: PolicyMode::BackupOnly,
                rationale: None,
            },
        ];
        fs::write(
            &temp_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let report = ensure_target_ids(Some(temp_path.as_path())).expect("ensure target ids");
        assert_eq!(report.outcome, ActionOutcome::Success);
        assert_eq!(report.assigned_count, 2);

        let loaded = load_config(Some(temp_path.as_path())).expect("load config");
        let ids: Vec<_> = loaded
            .config
            .managed_targets
            .iter()
            .map(|target| target.target_id.clone().expect("target id"))
            .collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);

        let _ = fs::remove_file(temp_path);
    }

    #[test]
    fn add_managed_target_assigns_id_and_persists() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let target_path = temp_root.join("Notes/Personal");
        let temp_path = temp_root.join("config.toml");
        let script_path = temp_root.join("cloud-sync.sh");

        fs::create_dir_all(&target_path).expect("create target path");
        fs::write(
            &script_path,
            "BISYNC_FOLDERS=(\n    \"Notes\"\n)\n\nBACKUP_FOLDERS=(\n    \".memloft:.memloft\"\n)\n",
        )
        .expect("write sync script");

        let mut config = AppConfig::default();
        config.sync_script_path = script_path;
        fs::write(
            &temp_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let report = add_managed_target(
            Some(temp_path.as_path()),
            "Notes/Personal",
            target_path.as_path(),
            "OneDrive/Notes/Personal",
            PolicyMode::BackupOnly,
            Some("test"),
        )
        .expect("add managed target");

        assert_eq!(report.outcome, ActionOutcome::Success);
        assert_eq!(report.target.name, "Notes/Personal");
        assert!(report.target.target_id.is_some());

        let loaded = load_config(Some(temp_path.as_path())).expect("load config");
        assert_eq!(loaded.config.managed_targets.len(), 1);
        assert_eq!(
            loaded.config.managed_targets[0].target_id,
            report.target.target_id
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn relocate_managed_target_preserves_id_and_updates_path() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let original_path = temp_root.join("Notes/Personal");
        let relocated_path = temp_root.join("Notes/RenamedPersonal");
        let temp_path = temp_root.join("config.toml");
        let script_path = temp_root.join("cloud-sync.sh");

        fs::create_dir_all(&original_path).expect("create original path");
        fs::create_dir_all(&relocated_path).expect("create relocated path");
        fs::write(
            &script_path,
            "BISYNC_FOLDERS=(\n    \"Notes\"\n)\n\nBACKUP_FOLDERS=(\n    \".memloft:.memloft\"\n)\n",
        )
        .expect("write sync script");

        let mut config = AppConfig::default();
        config.sync_script_path = script_path;
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("target-123".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: original_path,
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: Some("test".to_string()),
        }];
        fs::write(
            &temp_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let report = relocate_managed_target(
            Some(temp_path.as_path()),
            "target-123",
            relocated_path.as_path(),
            Some("OneDrive/Notes/RenamedPersonal"),
        )
        .expect("relocate managed target");

        assert_eq!(report.outcome, ActionOutcome::Success);
        assert_eq!(report.target.target_id.as_deref(), Some("target-123"));
        assert_eq!(report.target.local_path, relocated_path);
        assert_eq!(report.target.remote_path, "OneDrive/Notes/RenamedPersonal");

        let loaded = load_config(Some(temp_path.as_path())).expect("load config");
        assert_eq!(
            loaded.config.managed_targets[0].target_id.as_deref(),
            Some("target-123")
        );
        assert_eq!(loaded.config.managed_targets[0].local_path, relocated_path);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn runner_due_status_detects_when_cycle_is_due() {
        let (due_without_history, next_due_without_history) = runner_due_status(None, 60_000, 123);
        assert!(due_without_history);
        assert_eq!(next_due_without_history, None);

        let (due_too_soon, next_due_too_soon) =
            runner_due_status(Some(1_000), 60_000, 30_000);
        assert!(!due_too_soon);
        assert_eq!(next_due_too_soon, Some(61_000));

        let (due_after_interval, next_due_after_interval) =
            runner_due_status(Some(1_000), 60_000, 61_000);
        assert!(due_after_interval);
        assert_eq!(next_due_after_interval, Some(61_000));
    }
}
