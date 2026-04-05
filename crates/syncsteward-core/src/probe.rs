use crate::config::{
    AppConfig, FolderPolicy, ManagedTarget, PolicyMode, RunnerLaunchAgentConfig, VerificationMode,
    default_config_path, expand_path, load_config, normalize_app_config,
};
use crate::inventory::build_target_inventory;
use crate::model::{
    ActionOutcome, ActionStep, ActionStepStatus, ActionTarget, ActiveTargetOperationSummary,
    AddManagedTargetReport, AlertRecord, AlertReport, AlertSeverity, ApprovedTargetOverview,
    ArtifactKind, ArtifactQuarantineRecord, ArtifactQuarantineReport, ArtifactReport, CheckStatus,
    ConfigPatch, ConfigScaffoldReport, ConfigSchemaReport, ConfigSnapshotReport,
    ConfigUpdateReport, ControlAction, ControlReport, CycleSkippedTarget, EnsureTargetIdsReport,
    FailureClass, LaunchAgentStatus, LogAcknowledgeReport, LogSummary, ManagedTargetIdAssignment,
    ManagedTargetIdAssignmentReason, NotifyAlertsReport, OverviewReport, PolicySummary,
    PreflightCheck, PreflightReport, PruneStateReport, RecentTargetRunSummary, RecoveryAction,
    RelocateManagedTargetReport, RemoteStatus, RunCycleReport, RunnerActiveCycleSummary,
    RunnerAgentAction, RunnerAgentControlReport, RunnerAgentStatusReport, RunnerCycleSummary,
    RunnerOverview, RunnerTickReport, RunnerTickSummary, ServiceState, StatusReport, TargetBlocker,
    TargetCheckReport, TargetCheckSetReport, TargetEvaluation, TargetHealthOverview,
    TargetOperationKind, TargetRecoveryReport, TargetRunReport, TargetVerifyReport,
};
use crate::state::{
    ActiveTargetOperationState, AlertNotificationState, AppState, RunnerActiveCycleState,
    RunnerCycleState, RunnerTickState, TargetRunState, load_state, matches_acknowledged_log,
    save_acknowledged_log, save_active_target_operation, save_alert_notification_state,
    save_runner_active_cycle, save_runner_cycle, save_runner_tick, save_target_run,
};
use anyhow::{Context, Result, anyhow, bail};
use md5::{Digest, Md5};
use schemars::schema_for;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;
use walkdir::{DirEntry, WalkDir};

pub fn status(config_path: Option<&Path>) -> Result<StatusReport> {
    let loaded = load_config(config_path)?;
    Ok(collect_status(&loaded.config, loaded.source.description()))
}

pub fn overview(config_path: Option<&Path>) -> Result<OverviewReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let generated_at_unix_ms = now_unix_ms();
    let status = collect_status(&loaded.config, config_source.clone());
    let preflight = evaluate_preflight(status.clone(), PreflightMode::ManagedRun);
    let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let state = load_runtime_state(&loaded.config)?;
    let evaluations = inventory
        .targets
        .into_iter()
        .map(|target| evaluate_target(&preflight, target))
        .collect::<Vec<_>>();
    let approved_targets = build_approved_target_overview(
        &loaded.config.runner.approved_targets,
        &evaluations,
        &state,
    );
    let chronic_failures = build_chronic_failure_overview(
        &loaded.config.runner.approved_targets,
        &evaluations,
        &state,
    );
    let targets = build_target_health_overview(
        &evaluations,
        &approved_targets,
        &state,
        chronic_failures.len(),
    );
    let runner = build_runner_overview(
        &loaded.config,
        &status.runner_agent,
        &state,
        generated_at_unix_ms,
    );

    Ok(OverviewReport {
        config_source,
        generated_at_unix_ms,
        preflight_ready: preflight.ready,
        failing_check_count: preflight
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .count(),
        warning_check_count: preflight
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Warn)
            .count(),
        active_alert_count: alert_report.alerts.len(),
        status,
        preflight_checks: preflight.checks,
        runner,
        targets,
        chronic_failures,
        approved_targets,
        recent_target_runs: build_recent_target_run_summaries(&state),
        alerts: alert_report.alerts,
    })
}

pub fn preflight(config_path: Option<&Path>) -> Result<PreflightReport> {
    let loaded = load_config(config_path)?;
    let status = collect_status(&loaded.config, loaded.source.description());
    Ok(evaluate_preflight(status, PreflightMode::ManagedRun))
}

pub fn alerts(config_path: Option<&Path>) -> Result<AlertReport> {
    let loaded = load_config(config_path)?;
    let report = evaluate_alerts(&loaded.config, loaded.source.description())?;
    Ok(report)
}

pub fn notify_alerts(config_path: Option<&Path>, dry_run: bool) -> Result<NotifyAlertsReport> {
    let loaded = load_config(config_path)?;
    let report = evaluate_alerts(&loaded.config, loaded.source.description())?;
    let notify = send_alert_notification(
        &loaded.config,
        dry_run,
        NotificationRequest {
            alerts: &report.alerts,
            allow_empty: false,
            title: "SyncSteward",
            subtitle: &format!(
                "{} active alert{}",
                report.alerts.len(),
                if report.alerts.len() == 1 { "" } else { "s" }
            ),
            body: &summarize_alerts_notification(&report.alerts),
            empty_summary: "no active alerts".to_string(),
            disabled_summary: "notifications are disabled".to_string(),
            dry_run_summary: format!("dry run would notify {} active alerts", report.alerts.len()),
            success_summary: format!(
                "sent notification for {} active alerts",
                report.alerts.len()
            ),
        },
    )?;

    if notify.outcome == ActionOutcome::Success && !dry_run {
        let now = now_unix_ms();
        let signature = Some(alert_signature(&report.alerts));
        let updated_state = AlertNotificationState {
            active_signature: signature.clone(),
            active_since_unix_ms: Some(now),
            last_notified_signature: signature,
            last_notified_at_unix_ms: Some(now),
            repeat_count: 1,
        };
        if let Err(error) = save_alert_notification_state(&loaded.config.state_path, updated_state)
        {
            eprintln!("syncsteward: failed to record direct alert notification state: {error}");
        }
    }

    Ok(NotifyAlertsReport {
        outcome: notify.outcome,
        summary: notify.summary,
        dry_run,
        alerts: report.alerts,
        steps: notify.steps,
    })
}

struct NotificationRequest<'a> {
    alerts: &'a [AlertRecord],
    allow_empty: bool,
    title: &'a str,
    subtitle: &'a str,
    body: &'a str,
    empty_summary: String,
    disabled_summary: String,
    dry_run_summary: String,
    success_summary: String,
}

struct NotificationDispatch {
    outcome: ActionOutcome,
    summary: String,
    steps: Vec<ActionStep>,
}

struct ScheduledNotificationDecision {
    report: NotifyAlertsReport,
    updated_state: Option<AlertNotificationState>,
}

fn send_alert_notification(
    config: &AppConfig,
    dry_run: bool,
    request: NotificationRequest<'_>,
) -> Result<NotificationDispatch> {
    let mut steps = Vec::new();

    if request.alerts.is_empty() && !request.allow_empty {
        steps.push(skipped_step(
            "alerts_snapshot",
            "no active alerts to notify".to_string(),
            "alert evaluation returned no active issues".to_string(),
        ));
        return Ok(NotificationDispatch {
            outcome: ActionOutcome::NoOp,
            summary: request.empty_summary,
            steps,
        });
    }

    if !config.alerts.enable_macos_notifications {
        steps.push(skipped_step(
            "macos_notifications",
            "macOS notifications are disabled in config".to_string(),
            request.body.to_string(),
        ));
        return Ok(NotificationDispatch {
            outcome: ActionOutcome::NoOp,
            summary: request.disabled_summary,
            steps,
        });
    }

    if dry_run {
        steps.push(applied_step(
            "macos_notifications",
            "dry run prepared a macOS notification".to_string(),
            request.body.to_string(),
        ));
        return Ok(NotificationDispatch {
            outcome: ActionOutcome::Success,
            summary: request.dry_run_summary,
            steps,
        });
    }

    let output = run_command(
        "osascript",
        [
            "-e",
            format!(
                "display notification {} with title {} subtitle {}",
                apple_script_string(request.body),
                apple_script_string(request.title),
                apple_script_string(request.subtitle),
            )
            .as_str(),
        ],
    );

    if output.success {
        steps.push(applied_step(
            "macos_notifications",
            "sent a macOS notification".to_string(),
            request.body.to_string(),
        ));
        Ok(NotificationDispatch {
            outcome: ActionOutcome::Success,
            summary: request.success_summary,
            steps,
        })
    } else {
        steps.push(failed_step(
            "macos_notifications",
            "failed to send a macOS notification".to_string(),
            summarize_command_output(&output),
        ));
        Ok(NotificationDispatch {
            outcome: ActionOutcome::Failed,
            summary: "failed to send notification".to_string(),
            steps,
        })
    }
}

fn scheduled_notify_alerts(
    config: &AppConfig,
    state: &crate::state::AppState,
    alerts: &[AlertRecord],
    dry_run: bool,
) -> Result<ScheduledNotificationDecision> {
    let now = now_unix_ms();
    let existing = state.alert_notifications.clone();

    if alerts.is_empty() {
        if existing.active_signature.is_none() {
            return Ok(ScheduledNotificationDecision {
                report: NotifyAlertsReport {
                    outcome: ActionOutcome::NoOp,
                    summary: "no active alerts".to_string(),
                    dry_run,
                    alerts: Vec::new(),
                    steps: vec![skipped_step(
                        "alerts_snapshot",
                        "no active alerts to notify".to_string(),
                        "alert evaluation returned no active issues".to_string(),
                    )],
                },
                updated_state: None,
            });
        }

        let cleared_state = AlertNotificationState::default();
        if config.alerts.recovery_notifications {
            let dispatch = send_alert_notification(
                config,
                dry_run,
                NotificationRequest {
                    alerts,
                    allow_empty: true,
                    title: "SyncSteward",
                    subtitle: "alerts cleared",
                    body: "all active SyncSteward alerts have cleared",
                    empty_summary: "no active alerts".to_string(),
                    disabled_summary: "notifications are disabled".to_string(),
                    dry_run_summary: "dry run would notify that alerts cleared".to_string(),
                    success_summary: "sent recovery notification".to_string(),
                },
            )?;
            return Ok(ScheduledNotificationDecision {
                report: NotifyAlertsReport {
                    outcome: dispatch.outcome,
                    summary: dispatch.summary,
                    dry_run,
                    alerts: Vec::new(),
                    steps: dispatch.steps,
                },
                updated_state: if dry_run { None } else { Some(cleared_state) },
            });
        }

        return Ok(ScheduledNotificationDecision {
            report: NotifyAlertsReport {
                outcome: ActionOutcome::NoOp,
                summary: "alerts cleared without recovery notification".to_string(),
                dry_run,
                alerts: Vec::new(),
                steps: vec![skipped_step(
                    "macos_notifications",
                    "recovery notifications are disabled".to_string(),
                    "the prior active alert set was cleared from state without sending a recovery notification"
                        .to_string(),
                )],
            },
            updated_state: if dry_run { None } else { Some(cleared_state) },
        });
    }

    let signature = alert_signature(alerts);
    let repeat_after_ms = u128::from(config.alerts.repeat_notification_after_minutes) * 60 * 1000;

    if existing.active_signature.as_deref() != Some(signature.as_str()) {
        let summary = summarize_alerts_notification(alerts);
        let body = summary.clone();
        let dispatch = send_alert_notification(
            config,
            dry_run,
            NotificationRequest {
                alerts,
                allow_empty: false,
                title: "SyncSteward",
                subtitle: &format!(
                    "{} active alert{}",
                    alerts.len(),
                    if alerts.len() == 1 { "" } else { "s" }
                ),
                body: &body,
                empty_summary: "no active alerts".to_string(),
                disabled_summary: "notifications are disabled".to_string(),
                dry_run_summary: format!("dry run would notify {} active alerts", alerts.len()),
                success_summary: format!("sent notification for {} active alerts", alerts.len()),
            },
        )?;

        let updated_state = AlertNotificationState {
            active_signature: Some(signature),
            active_since_unix_ms: Some(now),
            last_notified_signature: if dispatch.outcome == ActionOutcome::Success && !dry_run {
                Some(alert_signature(alerts))
            } else {
                None
            },
            last_notified_at_unix_ms: if dispatch.outcome == ActionOutcome::Success && !dry_run {
                Some(now)
            } else {
                None
            },
            repeat_count: if dispatch.outcome == ActionOutcome::Success && !dry_run {
                1
            } else {
                0
            },
        };

        return Ok(ScheduledNotificationDecision {
            report: NotifyAlertsReport {
                outcome: dispatch.outcome,
                summary: dispatch.summary,
                dry_run,
                alerts: alerts.to_vec(),
                steps: dispatch.steps,
            },
            updated_state: if dry_run { None } else { Some(updated_state) },
        });
    } else {
        let since_last_notification = existing
            .last_notified_at_unix_ms
            .map(|last| now.saturating_sub(last));
        if existing.last_notified_signature.as_deref() != Some(signature.as_str())
            || since_last_notification.is_none()
            || since_last_notification.is_some_and(|age| age >= repeat_after_ms)
        {
            let age_minutes = since_last_notification.unwrap_or_default() / 60_000;
            let repeat_count = existing.repeat_count.saturating_add(1).max(1);
            let summary = summarize_alerts_notification(alerts);
            let body = if repeat_count > 1 {
                format!(
                    "persistent alert set continues ({repeat_count} notifications, last sent {age_minutes} minutes ago): {summary}"
                )
            } else {
                summary
            };
            let dispatch = send_alert_notification(
                config,
                dry_run,
                NotificationRequest {
                    alerts,
                    allow_empty: false,
                    title: "SyncSteward",
                    subtitle: if repeat_count > 1 {
                        "persistent alerts"
                    } else {
                        "active alerts"
                    },
                    body: &body,
                    empty_summary: "no active alerts".to_string(),
                    disabled_summary: "notifications are disabled".to_string(),
                    dry_run_summary: if repeat_count > 1 {
                        "dry run would repeat a persistent alert notification".to_string()
                    } else {
                        format!("dry run would notify {} active alerts", alerts.len())
                    },
                    success_summary: if repeat_count > 1 {
                        format!(
                            "sent persistent notification for {} active alerts",
                            alerts.len()
                        )
                    } else {
                        format!("sent notification for {} active alerts", alerts.len())
                    },
                },
            )?;

            let updated_state = AlertNotificationState {
                active_signature: Some(signature),
                active_since_unix_ms: existing.active_since_unix_ms.or(Some(now)),
                last_notified_signature: if dispatch.outcome == ActionOutcome::Success && !dry_run {
                    Some(alert_signature(alerts))
                } else {
                    existing.last_notified_signature
                },
                last_notified_at_unix_ms: if dispatch.outcome == ActionOutcome::Success && !dry_run
                {
                    Some(now)
                } else {
                    existing.last_notified_at_unix_ms
                },
                repeat_count: if dispatch.outcome == ActionOutcome::Success && !dry_run {
                    repeat_count
                } else {
                    existing.repeat_count
                },
            };

            return Ok(ScheduledNotificationDecision {
                report: NotifyAlertsReport {
                    outcome: dispatch.outcome,
                    summary: dispatch.summary,
                    dry_run,
                    alerts: alerts.to_vec(),
                    steps: dispatch.steps,
                },
                updated_state: if dry_run { None } else { Some(updated_state) },
            });
        }

        let updated_state = AlertNotificationState {
            active_signature: Some(signature),
            active_since_unix_ms: existing.active_since_unix_ms.or(Some(now)),
            last_notified_signature: existing.last_notified_signature,
            last_notified_at_unix_ms: existing.last_notified_at_unix_ms,
            repeat_count: existing.repeat_count,
        };

        Ok(ScheduledNotificationDecision {
            report: NotifyAlertsReport {
                outcome: ActionOutcome::NoOp,
                summary: "suppressed repeat notification for unchanged alert set".to_string(),
                dry_run,
                alerts: alerts.to_vec(),
                steps: vec![skipped_step(
                    "macos_notifications",
                    "unchanged alert set is inside the repeat-notification window".to_string(),
                    format!(
                        "will repeat only after {} minutes unless the alert set changes",
                        config.alerts.repeat_notification_after_minutes
                    ),
                )],
            },
            updated_state: if dry_run { None } else { Some(updated_state) },
        })
    }
}

fn notification_step(id: &str, report: &NotifyAlertsReport) -> ActionStep {
    match report.outcome {
        ActionOutcome::NoOp => skipped_step(
            id,
            "no post-tick notification sent".to_string(),
            report.summary.clone(),
        ),
        ActionOutcome::Success => applied_step(
            id,
            "sent post-tick notification".to_string(),
            report.summary.clone(),
        ),
        ActionOutcome::Failed => failed_step(
            id,
            "failed to send post-tick notification".to_string(),
            report.summary.clone(),
        ),
        ActionOutcome::Blocked => blocked_step(
            id,
            "post-tick notification was blocked".to_string(),
            report.summary.clone(),
        ),
    }
}

pub fn check_targets(config_path: Option<&Path>) -> Result<TargetCheckSetReport> {
    let loaded = load_config(config_path)?;
    let preflight = evaluate_preflight(
        collect_status(&loaded.config, loaded.source.description()),
        PreflightMode::ManagedRun,
    );
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
    let preflight = evaluate_preflight(
        collect_status(&loaded.config, loaded.source.description()),
        PreflightMode::ManagedRun,
    );
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
    let _operation_guard = ActiveTargetOperationGuard::for_selector(
        config_path,
        selector,
        TargetOperationKind::Run,
        dry_run,
    )?;
    run_target_inner(
        config_path,
        selector,
        dry_run,
        false,
        false,
        TargetExecutionStrategy::Standard,
    )
}

pub fn verify_target(config_path: Option<&Path>, selector: &str) -> Result<TargetVerifyReport> {
    let _operation_guard = ActiveTargetOperationGuard::for_selector(
        config_path,
        selector,
        TargetOperationKind::Verify,
        false,
    )?;
    verify_target_inner(config_path, selector)
}

pub fn repair_target(
    config_path: Option<&Path>,
    selector: &str,
    dry_run: bool,
) -> Result<TargetRecoveryReport> {
    let _operation_guard = ActiveTargetOperationGuard::for_selector(
        config_path,
        selector,
        TargetOperationKind::Repair,
        dry_run,
    )?;
    recover_target(config_path, selector, RecoveryAction::Repair, dry_run, true)
}

pub fn rebaseline_target(
    config_path: Option<&Path>,
    selector: &str,
    dry_run: bool,
    confirmed: bool,
) -> Result<TargetRecoveryReport> {
    let _operation_guard = ActiveTargetOperationGuard::for_selector(
        config_path,
        selector,
        TargetOperationKind::Rebaseline,
        dry_run,
    )?;
    recover_target(
        config_path,
        selector,
        RecoveryAction::Rebaseline,
        dry_run,
        confirmed,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetExecutionStrategy {
    Standard,
    Rebaseline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerificationTrigger {
    ScheduledRun,
    ExplicitVerify,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CheckMismatchReport {
    differ: Vec<String>,
    missing_on_dst: Vec<String>,
    missing_on_src: Vec<String>,
    errors: Vec<String>,
}

const MAX_DIVERGENCE_RECOVERY_ROUNDS: usize = 3;

impl CheckMismatchReport {
    fn is_clean(&self) -> bool {
        self.differ.is_empty()
            && self.missing_on_dst.is_empty()
            && self.missing_on_src.is_empty()
            && self.errors.is_empty()
    }

    fn copy_paths(&self) -> Vec<String> {
        self.differ
            .iter()
            .chain(self.missing_on_dst.iter())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    fn delete_paths(&self) -> Vec<String> {
        self.missing_on_src
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

fn determine_verification_mode(
    config: &AppConfig,
    trigger: VerificationTrigger,
    existing_run: Option<&TargetRunState>,
) -> (VerificationMode, String) {
    if trigger == VerificationTrigger::ExplicitVerify {
        return (
            VerificationMode::Full,
            "explicit verify-target requests always run a full remote verification".to_string(),
        );
    }

    match config.verification.default_mode {
        VerificationMode::Full => (
            VerificationMode::Full,
            "verification.default_mode is set to full".to_string(),
        ),
        VerificationMode::SizeAndSample => {
            if config.verification.full_after_hours == 0 {
                return (
                    VerificationMode::SizeAndSample,
                    "verification.default_mode is size_and_sample and periodic full verification is disabled".to_string(),
                );
            }

            let Some(last_full_verified_at_unix_ms) = effective_last_full_verified_at(existing_run)
            else {
                return (
                    VerificationMode::SizeAndSample,
                    "no prior full verification is recorded for this target; scheduled runs will use bounded size-and-sample verification until an explicit full verification is recorded".to_string(),
                );
            };

            let full_after_ms =
                u128::from(config.verification.full_after_hours).saturating_mul(60 * 60 * 1000);
            let next_full_due_at = last_full_verified_at_unix_ms.saturating_add(full_after_ms);
            if now_unix_ms() >= next_full_due_at {
                (
                    VerificationMode::Full,
                    format!(
                        "last full verification is older than {} hours",
                        config.verification.full_after_hours
                    ),
                )
            } else {
                (
                    VerificationMode::SizeAndSample,
                    format!(
                        "using bounded size-and-sample verification until the next full verification window after {} hours",
                        config.verification.full_after_hours
                    ),
                )
            }
        }
    }
}

fn effective_last_full_verified_at(existing_run: Option<&TargetRunState>) -> Option<u128> {
    existing_run.and_then(|run| {
        run.last_full_verified_at_unix_ms.or_else(|| {
            if run.last_verification_mode.is_none() {
                run.last_verified_at_unix_ms
            } else {
                None
            }
        })
    })
}

fn run_target_inner(
    config_path: Option<&Path>,
    selector: &str,
    dry_run: bool,
    reuse_existing_lock: bool,
    assume_remote_paused: bool,
    strategy: TargetExecutionStrategy,
) -> Result<TargetRunReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let target = resolve_inventory_target(inventory.targets, selector)?;
    let existing_state = load_state(&loaded.config.state_path).ok();
    let selected_verification_mode = existing_state
        .as_ref()
        .and_then(|state| lookup_target_run_state(state, &target))
        .map(|run| {
            determine_verification_mode(
                &loaded.config,
                VerificationTrigger::ScheduledRun,
                Some(run),
            )
        })
        .unwrap_or_else(|| {
            determine_verification_mode(&loaded.config, VerificationTrigger::ScheduledRun, None)
        });
    let base_status = collect_target_status(
        &loaded.config,
        config_source.clone(),
        std::slice::from_ref(&target.local_path),
    );
    let base_preflight = evaluate_preflight(base_status.clone(), PreflightMode::ManagedRun);
    let base_evaluation = evaluate_target(&base_preflight, target.clone());
    let mut steps = Vec::new();

    if base_evaluation.effective_mode != crate::config::PolicyMode::BackupOnly {
        steps.push(blocked_step(
            "execution_mode_unsupported",
            format!(
                "{} is configured as {}",
                base_evaluation.target.name,
                describe_policy_mode(base_evaluation.effective_mode)
            ),
            "folder-scoped execution currently supports backup-only targets only".to_string(),
        ));
    }

    if !base_evaluation.target.local_path.is_dir() {
        steps.push(blocked_step(
            "local_path_not_directory",
            format!(
                "{} is not a directory",
                base_evaluation.target.local_path.display()
            ),
            "folder-scoped execution currently expects directory targets".to_string(),
        ));
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
                config_source,
                selector: selector.to_string(),
                dry_run,
                outcome: ActionOutcome::Blocked,
                summary: format!(
                    "{} blocked for {}",
                    if dry_run { "dry run" } else { "run" },
                    base_evaluation.target.name
                ),
                preflight_ready: base_preflight.ready,
                evaluation: base_evaluation.clone(),
                verified_at_unix_ms: None,
                verification_mode: None,
                failure_class: None,
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
            config_source,
            selector: selector.to_string(),
            dry_run,
            outcome: ActionOutcome::Failed,
            summary: format!(
                "{} failed for {}",
                if dry_run { "dry run" } else { "run" },
                base_evaluation.target.name
            ),
            preflight_ready: base_preflight.ready,
            evaluation: base_evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: None,
            failure_class: Some(FailureClass::Transport),
            steps,
        };
        record_target_run(&loaded.config, &report);
        return Ok(report);
    };
    steps.push(applied_step(
        "select_remote_host",
        format!("selected remote host {}", host),
        format!("using {} for {}", host, base_evaluation.target.remote_path),
    ));

    let mut effective_status = base_status.clone();
    let mut remote_paused = false;
    let remote_was_active = matches!(base_status.remote.service_state, ServiceState::Active);
    let should_coord_pause =
        !assume_remote_paused && loaded.config.coordination.pause_remote_on_sync;
    if assume_remote_paused {
        effective_status.remote.service_state = ServiceState::Inactive;
        effective_status.remote.detail = format!(
            "{} coordination is already active",
            loaded.config.remote.onedrive_service
        );
    } else if should_coord_pause && remote_was_active {
        if dry_run {
            steps.push(skipped_step(
                "pause_remote_onedrive",
                "would pause remote OneDrive service before sync".to_string(),
                base_status.remote.detail.clone(),
            ));
            effective_status.remote.service_state = ServiceState::Inactive;
            effective_status.remote.detail = format!(
                "{} would be paused before sync",
                loaded.config.remote.onedrive_service
            );
        } else {
            let pause_step = pause_remote_onedrive(&loaded.config);
            let pause_failed = pause_step.status == ActionStepStatus::Failed;
            steps.push(pause_step);
            if pause_failed {
                let report = TargetRunReport {
                    config_source,
                    selector: selector.to_string(),
                    dry_run,
                    outcome: ActionOutcome::Failed,
                    summary: format!(
                        "{} failed for {}",
                        if dry_run { "dry run" } else { "run" },
                        base_evaluation.target.name
                    ),
                    preflight_ready: base_preflight.ready,
                    evaluation: base_evaluation.clone(),
                    verified_at_unix_ms: None,
                    verification_mode: None,
                    failure_class: Some(FailureClass::Transport),
                    steps,
                };
                record_target_run(&loaded.config, &report);
                return Ok(report);
            }
            remote_paused = true;
            let materialize_step = materialize_remote_instruction_links(&loaded.config, &host);
            steps.push(materialize_step);
            if steps
                .last()
                .is_some_and(|step| step.status == ActionStepStatus::Failed)
            {
                steps.push(resume_remote_onedrive(&loaded.config));
                let report = TargetRunReport {
                    config_source,
                    selector: selector.to_string(),
                    dry_run,
                    outcome: ActionOutcome::Failed,
                    summary: format!(
                        "{} failed for {}",
                        if dry_run { "dry run" } else { "run" },
                        base_evaluation.target.name
                    ),
                    preflight_ready: base_preflight.ready,
                    evaluation: base_evaluation.clone(),
                    verified_at_unix_ms: None,
                    verification_mode: None,
                    failure_class: Some(FailureClass::Path),
                    steps,
                };
                record_target_run(&loaded.config, &report);
                return Ok(report);
            }
            effective_status = collect_target_status(
                &loaded.config,
                config_source.clone(),
                std::slice::from_ref(&base_evaluation.target.local_path),
            );
            effective_status.remote.service_state = ServiceState::Inactive;
            effective_status.remote.detail = format!(
                "{} was paused before sync",
                loaded.config.remote.onedrive_service
            );
        }
    }

    let preflight = evaluate_preflight(effective_status, PreflightMode::ManagedRun);
    let evaluation = evaluate_target(&preflight, target);

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

    if contains_blocking_steps(&steps) {
        if remote_paused {
            steps.push(resume_remote_onedrive(&loaded.config));
        }
        let outcome = summarize_run_outcome(&steps);
        let summary = format!(
            "{} blocked for {}",
            if dry_run { "dry run" } else { "run" },
            evaluation.target.name
        );
        let report = TargetRunReport {
            config_source,
            selector: selector.to_string(),
            dry_run,
            outcome,
            summary,
            preflight_ready: preflight.ready,
            evaluation: evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: None,
            failure_class: None,
            steps,
        };
        record_target_run(&loaded.config, &report);
        return Ok(report);
    }

    let snapshot_policy = target_snapshot_policy(&loaded.config, &evaluation.target.name);
    let temp_dir = make_temp_workdir(&evaluation.target.name)?;
    steps.push(applied_step(
        "select_verification_mode",
        format!(
            "using {} verification for {}",
            describe_verification_mode(selected_verification_mode.0),
            evaluation.target.name
        ),
        selected_verification_mode.1.clone(),
    ));
    if strategy == TargetExecutionStrategy::Rebaseline {
        execute_rebaseline_purge_target(
            &loaded.config,
            &evaluation.target,
            &host,
            dry_run,
            &temp_dir,
            &mut steps,
        )?;
    }
    let result = if let Some(snapshot_policy) = snapshot_policy {
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
    if result.is_ok() && !dry_run && summarize_run_outcome(&steps) == ActionOutcome::Success {
        if let Err(error) = execute_target_verification(
            &loaded.config,
            &evaluation.target,
            snapshot_policy,
            selected_verification_mode.0,
            &host,
            &temp_dir,
            &mut steps,
        ) {
            steps.push(failed_step(
                "verify_target",
                format!("verification failed for {}", evaluation.target.name),
                error.to_string(),
            ));
        }
    }
    let cleanup_result = fs::remove_dir_all(&temp_dir);
    drop(acquired_lock);
    if let Err(error) = cleanup_result {
        steps.push(failed_step(
            "cleanup_temp_workdir",
            format!("failed to remove {}", temp_dir.display()),
            error.to_string(),
        ));
    }

    if remote_paused {
        steps.push(resume_remote_onedrive(&loaded.config));
    }

    if let Err(error) = result {
        steps.push(failed_step(
            "execute_target",
            format!(
                "{} failed for {}",
                if dry_run { "dry run" } else { "run" },
                evaluation.target.name
            ),
            error.to_string(),
        ));
    }

    let outcome = summarize_run_outcome(&steps);
    let verification_steps = verification_steps_from_steps(&steps);
    let verified_at_unix_ms = if !dry_run
        && !verification_steps.is_empty()
        && summarize_run_outcome(&verification_steps) == ActionOutcome::Success
    {
        Some(now_unix_ms())
    } else {
        None
    };
    let failure_class = classify_failure_from_steps(outcome, &steps);
    let summary = summarize_target_run(&evaluation.target.name, dry_run, outcome, &steps);
    let report = TargetRunReport {
        config_source,
        selector: selector.to_string(),
        dry_run,
        outcome,
        summary,
        preflight_ready: preflight.ready,
        evaluation,
        verified_at_unix_ms,
        verification_mode: if dry_run || verification_steps.is_empty() {
            None
        } else {
            Some(selected_verification_mode.0)
        },
        failure_class,
        steps,
    };
    record_target_run(&loaded.config, &report);
    Ok(report)
}

fn verify_target_inner(config_path: Option<&Path>, selector: &str) -> Result<TargetVerifyReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let target = resolve_inventory_target(inventory.targets, selector)?;
    let selected_verification_mode =
        determine_verification_mode(&loaded.config, VerificationTrigger::ExplicitVerify, None);
    let base_status = collect_target_status(
        &loaded.config,
        config_source.clone(),
        std::slice::from_ref(&target.local_path),
    );
    let base_preflight = evaluate_preflight(base_status.clone(), PreflightMode::ManagedRun);
    let base_evaluation = evaluate_target(&base_preflight, target.clone());
    let mut steps = Vec::new();

    if base_evaluation.effective_mode != crate::config::PolicyMode::BackupOnly {
        steps.push(blocked_step(
            "verification_mode_unsupported",
            format!(
                "{} is configured as {}",
                base_evaluation.target.name,
                describe_policy_mode(base_evaluation.effective_mode)
            ),
            "folder-scoped verification currently supports backup-only targets only".to_string(),
        ));
    }

    if !base_evaluation.target.local_path.is_dir() {
        steps.push(blocked_step(
            "local_path_not_directory",
            format!(
                "{} is not a directory",
                base_evaluation.target.local_path.display()
            ),
            "folder-scoped verification currently expects directory targets".to_string(),
        ));
    }

    let mut acquired_lock = None;
    steps.push(acquire_legacy_lock(&loaded.config, &mut acquired_lock)?);
    if steps
        .last()
        .is_some_and(|step| step.status == ActionStepStatus::Blocked)
    {
        let report = TargetVerifyReport {
            config_source,
            selector: selector.to_string(),
            outcome: ActionOutcome::Blocked,
            summary: format!("verification blocked for {}", base_evaluation.target.name),
            preflight_ready: base_preflight.ready,
            evaluation: base_evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: selected_verification_mode.0,
            failure_class: None,
            steps,
        };
        record_target_verification(&loaded.config, &report);
        return Ok(report);
    }

    let Some(host) = probe_remote_service(&loaded.config).selected_host else {
        steps.push(failed_step(
            "select_remote_host",
            "no reachable remote host was available".to_string(),
            "could not establish an SSH-backed rclone target".to_string(),
        ));
        let report = TargetVerifyReport {
            config_source,
            selector: selector.to_string(),
            outcome: ActionOutcome::Failed,
            summary: format!("verification failed for {}", base_evaluation.target.name),
            preflight_ready: base_preflight.ready,
            evaluation: base_evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: selected_verification_mode.0,
            failure_class: Some(FailureClass::Transport),
            steps,
        };
        record_target_verification(&loaded.config, &report);
        return Ok(report);
    };
    steps.push(applied_step(
        "select_remote_host",
        format!("selected remote host {}", host),
        format!("using {} for {}", host, base_evaluation.target.remote_path),
    ));

    let mut effective_status = base_status.clone();
    let mut remote_paused = false;
    let remote_was_active = matches!(base_status.remote.service_state, ServiceState::Active);
    if loaded.config.coordination.pause_remote_on_sync && remote_was_active {
        let pause_step = pause_remote_onedrive(&loaded.config);
        let pause_failed = pause_step.status == ActionStepStatus::Failed;
        steps.push(pause_step);
        if pause_failed {
            let report = TargetVerifyReport {
                config_source,
                selector: selector.to_string(),
                outcome: ActionOutcome::Failed,
                summary: format!("verification failed for {}", base_evaluation.target.name),
                preflight_ready: base_preflight.ready,
                evaluation: base_evaluation.clone(),
                verified_at_unix_ms: None,
                verification_mode: selected_verification_mode.0,
                failure_class: Some(FailureClass::Transport),
                steps,
            };
            record_target_verification(&loaded.config, &report);
            return Ok(report);
        }
        remote_paused = true;
        let materialize_step = materialize_remote_instruction_links(&loaded.config, &host);
        steps.push(materialize_step);
        if steps
            .last()
            .is_some_and(|step| step.status == ActionStepStatus::Failed)
        {
            steps.push(resume_remote_onedrive(&loaded.config));
            let report = TargetVerifyReport {
                config_source,
                selector: selector.to_string(),
                outcome: ActionOutcome::Failed,
                summary: format!("verification failed for {}", base_evaluation.target.name),
                preflight_ready: base_preflight.ready,
                evaluation: base_evaluation.clone(),
                verified_at_unix_ms: None,
                verification_mode: selected_verification_mode.0,
                failure_class: Some(FailureClass::Path),
                steps,
            };
            record_target_verification(&loaded.config, &report);
            return Ok(report);
        }
        effective_status = collect_target_status(
            &loaded.config,
            config_source.clone(),
            std::slice::from_ref(&base_evaluation.target.local_path),
        );
        effective_status.remote.service_state = ServiceState::Inactive;
        effective_status.remote.detail = format!(
            "{} was paused before verification",
            loaded.config.remote.onedrive_service
        );
    }

    let preflight = evaluate_preflight(effective_status, PreflightMode::ManagedRun);
    let evaluation = evaluate_target(&preflight, target);

    if !preflight.ready {
        steps.push(blocked_step(
            "preflight_gate",
            "verification blocked by global preflight failures".to_string(),
            failed_check_ids(&preflight),
        ));
    }

    if !evaluation.ready {
        steps.push(blocked_step(
            "target_gate",
            format!("target {} is not ready to verify", evaluation.target.name),
            format_target_blockers(&evaluation.blockers),
        ));
    }

    if steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Blocked)
    {
        if remote_paused {
            steps.push(resume_remote_onedrive(&loaded.config));
        }
        let outcome = summarize_run_outcome(&steps);
        let report = TargetVerifyReport {
            config_source,
            selector: selector.to_string(),
            outcome,
            summary: summarize_target_verify(&evaluation.target.name, outcome, &steps),
            preflight_ready: preflight.ready,
            evaluation: evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: selected_verification_mode.0,
            failure_class: None,
            steps,
        };
        record_target_verification(&loaded.config, &report);
        return Ok(report);
    }

    let snapshot_policy = target_snapshot_policy(&loaded.config, &evaluation.target.name);
    let temp_dir = make_temp_workdir(&evaluation.target.name)?;
    steps.push(applied_step(
        "select_verification_mode",
        format!(
            "using {} verification for {}",
            describe_verification_mode(selected_verification_mode.0),
            evaluation.target.name
        ),
        selected_verification_mode.1.clone(),
    ));
    if let Err(error) = execute_target_verification(
        &loaded.config,
        &evaluation.target,
        snapshot_policy,
        selected_verification_mode.0,
        &host,
        &temp_dir,
        &mut steps,
    ) {
        steps.push(failed_step(
            "verify_target",
            format!("verification failed for {}", evaluation.target.name),
            error.to_string(),
        ));
    }
    let cleanup_result = fs::remove_dir_all(&temp_dir);
    drop(acquired_lock);
    if let Err(error) = cleanup_result {
        steps.push(failed_step(
            "cleanup_temp_workdir",
            format!("failed to remove {}", temp_dir.display()),
            error.to_string(),
        ));
    }

    if remote_paused {
        steps.push(resume_remote_onedrive(&loaded.config));
    }

    let outcome = summarize_run_outcome(&steps);
    let verified_at_unix_ms = if outcome == ActionOutcome::Success {
        Some(now_unix_ms())
    } else {
        None
    };
    let failure_class = classify_failure_from_steps(outcome, &steps);
    let report = TargetVerifyReport {
        config_source,
        selector: selector.to_string(),
        outcome,
        summary: summarize_target_verify(&evaluation.target.name, outcome, &steps),
        preflight_ready: preflight.ready,
        evaluation,
        verified_at_unix_ms,
        verification_mode: selected_verification_mode.0,
        failure_class,
        steps,
    };
    record_target_verification(&loaded.config, &report);
    Ok(report)
}

fn recover_target(
    config_path: Option<&Path>,
    selector: &str,
    action: RecoveryAction,
    dry_run: bool,
    confirmed: bool,
) -> Result<TargetRecoveryReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let target = resolve_inventory_target(inventory.targets, selector)?;
    let target_name = target.name.clone();
    let confirmation_required = action == RecoveryAction::Rebaseline && !dry_run;
    let mut recovery_steps = Vec::new();

    if action == RecoveryAction::Rebaseline && !dry_run && !confirmed {
        recovery_steps.push(blocked_step(
            "confirm_rebaseline",
            format!("rebaseline requires explicit confirmation for {}", target_name),
            "rebaseline clears the remote target before rebuilding it from the current local tree; rerun with explicit confirmation to proceed".to_string(),
        ));
        let report = TargetRecoveryReport {
            config_source,
            selector: selector.to_string(),
            target_name,
            action,
            dry_run,
            confirmation_required,
            confirmed,
            outcome: ActionOutcome::Blocked,
            summary: format!(
                "rebaseline blocked for {} until confirmation is provided",
                target.name
            ),
            recovery_steps,
            run: None,
            verification: None,
        };
        record_target_recovery(&loaded.config, &report);
        return Ok(report);
    }

    match action {
        RecoveryAction::Repair => recovery_steps.push(skipped_step(
            "repair_strategy",
            format!("repair will rerun guarded sync for {}", target.name),
            "repair uses the normal backup-only sync path and then verifies the target".to_string(),
        )),
        RecoveryAction::Rebaseline => {
            let step = if dry_run {
                skipped_step(
                    "confirm_rebaseline",
                    format!("would rebuild remote baseline for {}", target.name),
                    "dry run skips destructive remote changes but validates the full rebaseline path".to_string(),
                )
            } else {
                applied_step(
                    "confirm_rebaseline",
                    format!("confirmed remote rebuild for {}", target.name),
                    "rebaseline will clear the remote target before syncing the current local tree"
                        .to_string(),
                )
            };
            recovery_steps.push(step);
        }
    }

    let strategy = match action {
        RecoveryAction::Repair => TargetExecutionStrategy::Standard,
        RecoveryAction::Rebaseline => TargetExecutionStrategy::Rebaseline,
    };
    let run = run_target_inner(config_path, selector, dry_run, false, false, strategy)?;
    let mut verification = verification_report_from_run(&run);
    recovery_steps.extend(build_recovery_outcome_steps(&run));
    let mut outcome = run.outcome;

    if !dry_run
        && run.outcome == ActionOutcome::Failed
        && run.failure_class == Some(FailureClass::Divergence)
    {
        let (mut remediation_steps, remediation_verification) = attempt_target_divergence_recovery(
            config_path,
            &loaded.config,
            selector,
            &target,
            action,
        )?;
        recovery_steps.append(&mut remediation_steps);
        if let Some(remediation_verification) = remediation_verification {
            outcome = remediation_verification.outcome;
            verification = Some(remediation_verification);
        }
    } else if let Some(verification_report) = &verification {
        outcome = verification_report.outcome;
    }

    let report = TargetRecoveryReport {
        config_source,
        selector: selector.to_string(),
        target_name,
        action,
        dry_run,
        confirmation_required,
        confirmed: dry_run || confirmed,
        outcome,
        summary: summarize_target_recovery(action, outcome, &run, verification.as_ref()),
        recovery_steps,
        run: Some(run),
        verification,
    };
    record_target_recovery(&loaded.config, &report);
    Ok(report)
}

fn build_recovery_outcome_steps(run: &TargetRunReport) -> Vec<ActionStep> {
    if run.outcome != ActionOutcome::Failed || run.failure_class != Some(FailureClass::Divergence) {
        return Vec::new();
    }

    vec![applied_step(
        "detect_divergence",
        format!(
            "detected post-sync divergence for {}",
            run.evaluation.target.name
        ),
        "collecting mismatch reports and attempting targeted recovery".to_string(),
    )]
}

fn attempt_target_divergence_recovery(
    config_path: Option<&Path>,
    config: &AppConfig,
    selector: &str,
    target: &crate::model::SyncTargetRecord,
    action: RecoveryAction,
) -> Result<(Vec<ActionStep>, Option<TargetVerifyReport>)> {
    let mut steps = Vec::new();
    let remote_status = probe_remote_service(config);
    let Some(host) = remote_status.selected_host else {
        steps.push(failed_step(
            "select_remote_host_recovery",
            format!(
                "{} could not select a reachable remote host for {}",
                describe_recovery_action(action),
                target.name
            ),
            remote_status.detail,
        ));
        return Ok((steps, None));
    };

    steps.push(applied_step(
        "select_remote_host_recovery",
        format!("selected remote host {} for recovery", host),
        format!("using {} for {}", host, target.remote_path),
    ));

    let mut remote_paused = false;
    if config.coordination.pause_remote_on_sync
        && matches!(remote_status.service_state, ServiceState::Active)
    {
        let pause_step = pause_remote_onedrive(config);
        let pause_failed = pause_step.status == ActionStepStatus::Failed;
        steps.push(pause_step);
        if pause_failed {
            return Ok((steps, None));
        }
        remote_paused = true;

        let materialize_step = materialize_remote_instruction_links(config, &host);
        let materialize_failed = materialize_step.status == ActionStepStatus::Failed;
        steps.push(materialize_step);
        if materialize_failed {
            steps.push(resume_remote_onedrive(config));
            return Ok((steps, None));
        }
    }

    let snapshot_policy = target_snapshot_policy(config, &target.name);
    let mut verification = None;

    for round in 1..=MAX_DIVERGENCE_RECOVERY_ROUNDS {
        let temp_dir = make_temp_workdir(&format!("{}-recovery-round-{round}", target.name))?;
        let remediation_result = execute_target_divergence_recovery(
            config,
            target,
            snapshot_policy,
            &host,
            &temp_dir,
            &mut steps,
        );

        if let Err(error) = fs::remove_dir_all(&temp_dir) {
            steps.push(failed_step(
                "cleanup_recovery_temp_workdir",
                format!("failed to remove {}", temp_dir.display()),
                error.to_string(),
            ));
        }

        if let Err(error) = remediation_result {
            steps.push(failed_step(
                "execute_divergence_recovery",
                format!(
                    "{} failed while repairing divergence for {}",
                    describe_recovery_action(action),
                    target.name
                ),
                error.to_string(),
            ));
            break;
        }

        if steps
            .iter()
            .any(|step| step.status == ActionStepStatus::Failed)
        {
            break;
        }

        let current_verification = verify_target_inner(config_path, selector)?;
        let should_retry = current_verification.outcome == ActionOutcome::Failed
            && current_verification.failure_class == Some(FailureClass::Divergence)
            && round < MAX_DIVERGENCE_RECOVERY_ROUNDS;
        verification = Some(current_verification);

        if should_retry {
            steps.push(applied_step(
                "retry_divergence_recovery",
                format!(
                    "divergence remains after recovery round {} for {}",
                    round, target.name
                ),
                "retrying targeted recovery with a fresh mismatch scan".to_string(),
            ));
            continue;
        }

        break;
    }

    if remote_paused {
        steps.push(resume_remote_onedrive(config));
    }

    if steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Failed)
    {
        return Ok((steps, None));
    }

    Ok((steps, verification))
}

fn execute_target_divergence_recovery(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    snapshot_policy: Option<&crate::config::TargetSnapshot>,
    host: &str,
    temp_dir: &Path,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let rclone_config_path = write_target_rclone_config(config, host, temp_dir)?;
    steps.push(applied_step(
        "write_recovery_rclone_config",
        format!("wrote recovery rclone config for {}", target.name),
        rclone_config_path.display().to_string(),
    ));

    let filter_path = build_filter_file(config, target, temp_dir)?;
    steps.push(applied_step(
        "prepare_recovery_filters",
        format!("prepared recovery filter rules for {}", target.name),
        filter_path.display().to_string(),
    ));

    let remote_path = format!("syncsteward-target:{}", target.remote_path);

    if let Some(snapshot_policy) = snapshot_policy {
        let snapshot_excludes = snapshot_exclusion_patterns(snapshot_policy);
        remediate_rclone_mismatches(
            &rclone_config_path,
            &target.local_path,
            &remote_path,
            Some(&filter_path),
            None,
            &snapshot_excludes,
            temp_dir,
            "non_db",
            &format!("non-database files in {}", target.name),
            steps,
        )?;

        let snapshot_root = temp_dir.join("sqlite-recovery");
        fs::create_dir_all(&snapshot_root)?;
        let mut snapshot_paths = Vec::new();
        for relative_path in &snapshot_policy.sqlite_paths {
            let source_path = target.local_path.join(relative_path);
            if !source_path.exists() {
                steps.push(skipped_step(
                    "sqlite_snapshot_recovery_backup",
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
                    "sqlite_snapshot_recovery_backup",
                    format!(
                        "prepared SQLite recovery snapshot for {}",
                        relative_path.display()
                    ),
                    destination_path.display().to_string(),
                ));
                snapshot_paths.push(relative_path.clone());
            } else {
                steps.push(failed_step(
                    "sqlite_snapshot_recovery_backup",
                    format!(
                        "failed to prepare SQLite recovery snapshot for {}",
                        relative_path.display()
                    ),
                    summarize_command_output(&backup_output),
                ));
            }
        }

        if steps.iter().any(|step| {
            step.id == "sqlite_snapshot_recovery_backup" && step.status == ActionStepStatus::Failed
        }) {
            return Ok(());
        }

        if snapshot_paths.is_empty() {
            steps.push(skipped_step(
                "recover_snapshot_divergence",
                format!(
                    "no SQLite recovery snapshots were available for {}",
                    target.name
                ),
                "all configured snapshot sources were missing locally".to_string(),
            ));
            return Ok(());
        }

        let snapshot_list_path = temp_dir.join("sqlite-recovery-files.txt");
        write_path_list(
            &snapshot_list_path,
            &snapshot_paths
                .iter()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>(),
        )?;
        steps.push(applied_step(
            "sqlite_snapshot_recovery_list",
            format!("prepared snapshot recovery file list for {}", target.name),
            snapshot_list_path.display().to_string(),
        ));

        remediate_rclone_mismatches(
            &rclone_config_path,
            &snapshot_root,
            &remote_path,
            None,
            Some(&snapshot_list_path),
            &[],
            temp_dir,
            "snapshots",
            &format!("SQLite snapshots in {}", target.name),
            steps,
        )?;
        return Ok(());
    }

    remediate_rclone_mismatches(
        &rclone_config_path,
        &target.local_path,
        &remote_path,
        Some(&filter_path),
        None,
        &[],
        temp_dir,
        "target",
        &target.name,
        steps,
    )
}

fn remediate_rclone_mismatches(
    rclone_config_path: &Path,
    source_root: &Path,
    remote_path: &str,
    filter_path: Option<&Path>,
    check_files_from: Option<&Path>,
    extra_excludes: &[String],
    temp_dir: &Path,
    prefix: &str,
    label: &str,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let report = collect_rclone_check_mismatches(
        rclone_config_path,
        source_root,
        remote_path,
        filter_path,
        check_files_from,
        extra_excludes,
        temp_dir,
        prefix,
    )?;

    if report.errors.is_empty() && report.is_clean() {
        steps.push(skipped_step(
            "collect_divergence_report",
            format!("no remaining divergence detected for {}", label),
            "re-check found no mismatched files to rewrite".to_string(),
        ));
        return Ok(());
    }

    if !report.errors.is_empty() {
        steps.push(failed_step(
            "collect_divergence_report",
            format!("failed to gather a clean divergence report for {}", label),
            summarize_mismatch_report(&report),
        ));
        return Ok(());
    }

    steps.push(applied_step(
        "collect_divergence_report",
        format!("collected divergence report for {}", label),
        summarize_mismatch_report(&report),
    ));

    let copy_paths = report.copy_paths();
    if copy_paths.is_empty() {
        steps.push(skipped_step(
            "rewrite_divergent_files",
            format!("no divergent files required a rewrite for {}", label),
            "all mismatches were remote-only extras".to_string(),
        ));
    } else {
        let copy_list_path = temp_dir.join(format!("{prefix}-copy.txt"));
        write_path_list(&copy_list_path, &copy_paths)?;
        let copy_output = run_spawned_command_with_retry(
            || {
                let mut command = Command::new(resolve_program_path("rclone"));
                command
                    .env("RCLONE_CONFIG", rclone_config_path)
                    .arg("copy")
                    .arg(source_root)
                    .arg(remote_path)
                    .arg("--ignore-times")
                    .arg("--files-from")
                    .arg(&copy_list_path)
                    .arg("--skip-links");
                let _ = filter_path;
                let _ = extra_excludes;
                // The structured mismatch report has already been scoped by the original
                // filter file and excludes. rclone does not allow --files-from to be combined
                // with the other filter-style flags for copy/delete remediation commands.
                command
            },
            rclone_retry_attempts(false),
            Duration::from_secs(2),
        );

        if copy_output.success {
            steps.push(applied_step(
                "rewrite_divergent_files",
                format!("rewrote {} divergent files for {}", copy_paths.len(), label),
                summarize_command_output(&copy_output),
            ));
        } else {
            steps.push(failed_step(
                "rewrite_divergent_files",
                format!("failed to rewrite divergent files for {}", label),
                summarize_command_output(&copy_output),
            ));
            return Ok(());
        }
    }

    let delete_paths = report.delete_paths();
    if delete_paths.is_empty() {
        steps.push(skipped_step(
            "delete_remote_extras",
            format!("no remote-only extras required deletion for {}", label),
            "re-check found no destination-only files".to_string(),
        ));
        return Ok(());
    }

    let delete_list_path = temp_dir.join(format!("{prefix}-delete.txt"));
    write_path_list(&delete_list_path, &delete_paths)?;
    let delete_output = run_spawned_command_with_retry(
        || {
            let mut command = Command::new(resolve_program_path("rclone"));
            command
                .env("RCLONE_CONFIG", rclone_config_path)
                .arg("delete")
                .arg(remote_path)
                .arg("--files-from")
                .arg(&delete_list_path)
                .arg("--rmdirs");
            command
        },
        rclone_retry_attempts(false),
        Duration::from_secs(2),
    );

    if delete_output.success {
        steps.push(applied_step(
            "delete_remote_extras",
            format!(
                "deleted {} remote-only files for {}",
                delete_paths.len(),
                label
            ),
            summarize_command_output(&delete_output),
        ));
    } else {
        steps.push(failed_step(
            "delete_remote_extras",
            format!("failed to delete remote-only files for {}", label),
            summarize_command_output(&delete_output),
        ));
    }

    Ok(())
}

fn collect_rclone_check_mismatches(
    rclone_config_path: &Path,
    source_root: &Path,
    remote_path: &str,
    filter_path: Option<&Path>,
    check_files_from: Option<&Path>,
    extra_excludes: &[String],
    temp_dir: &Path,
    prefix: &str,
) -> Result<CheckMismatchReport> {
    let differ_path = temp_dir.join(format!("{prefix}-differ.txt"));
    let missing_dst_path = temp_dir.join(format!("{prefix}-missing-on-dst.txt"));
    let missing_src_path = temp_dir.join(format!("{prefix}-missing-on-src.txt"));
    let error_path = temp_dir.join(format!("{prefix}-error.txt"));

    let _ = run_spawned_command_with_retry(
        || {
            let mut command = Command::new(resolve_program_path("rclone"));
            command
                .env("RCLONE_CONFIG", rclone_config_path)
                .arg("check")
                .arg(source_root)
                .arg(remote_path)
                .arg("--skip-links")
                .arg("--differ")
                .arg(&differ_path)
                .arg("--missing-on-dst")
                .arg(&missing_dst_path)
                .arg("--missing-on-src")
                .arg(&missing_src_path)
                .arg("--error")
                .arg(&error_path)
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
            if let Some(filter_path) = filter_path {
                command.arg("--filter-from").arg(filter_path);
            }
            if let Some(check_files_from) = check_files_from {
                command.arg("--files-from").arg(check_files_from);
            }
            for pattern in extra_excludes {
                command.arg("--exclude").arg(pattern);
            }
            command
        },
        rclone_retry_attempts(false),
        Duration::from_secs(2),
    );

    Ok(CheckMismatchReport {
        differ: read_path_list(&differ_path)?,
        missing_on_dst: read_path_list(&missing_dst_path)?,
        missing_on_src: read_path_list(&missing_src_path)?,
        errors: read_path_list(&error_path)?,
    })
}

fn write_path_list(path: &Path, entries: &[String]) -> Result<()> {
    if entries.is_empty() {
        fs::write(path, "")?;
        return Ok(());
    }

    let mut contents = entries.join("\n");
    contents.push('\n');
    fs::write(path, contents)?;
    Ok(())
}

fn read_path_list(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    Ok(fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn summarize_mismatch_report(report: &CheckMismatchReport) -> String {
    let mut parts = Vec::new();
    if !report.differ.is_empty() {
        parts.push(format!(
            "{} differing ({})",
            report.differ.len(),
            sample_paths(&report.differ)
        ));
    }
    if !report.missing_on_dst.is_empty() {
        parts.push(format!(
            "{} missing on destination ({})",
            report.missing_on_dst.len(),
            sample_paths(&report.missing_on_dst)
        ));
    }
    if !report.missing_on_src.is_empty() {
        parts.push(format!(
            "{} missing on source ({})",
            report.missing_on_src.len(),
            sample_paths(&report.missing_on_src)
        ));
    }
    if !report.errors.is_empty() {
        parts.push(format!(
            "{} report errors ({})",
            report.errors.len(),
            sample_paths(&report.errors)
        ));
    }

    if parts.is_empty() {
        "no mismatches detected".to_string()
    } else {
        parts.join("; ")
    }
}

fn sample_paths(paths: &[String]) -> String {
    let mut sample = paths.iter().take(3).cloned().collect::<Vec<_>>();
    if paths.len() > sample.len() {
        sample.push(format!("+{} more", paths.len() - sample.len()));
    }
    sample.join(", ")
}

pub fn run_cycle(config_path: Option<&Path>, dry_run: bool) -> Result<RunCycleReport> {
    run_cycle_inner(config_path, dry_run, true)
}

fn run_cycle_inner(
    config_path: Option<&Path>,
    dry_run: bool,
    allow_cycle_notification: bool,
) -> Result<RunCycleReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let started_at_unix_ms = now_unix_ms();
    let preflight = evaluate_preflight(
        collect_status(&loaded.config, config_source.clone()),
        PreflightMode::ManagedRun,
    );
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let approved_selectors = loaded.config.runner.approved_targets.clone();
    let active_cycle_guard = RunnerActiveCycleGuard::new(
        &loaded.config.state_path,
        RunnerActiveCycleState {
            dry_run,
            started_at_unix_ms,
            process_id: Some(std::process::id()),
            current_target_selector: None,
            current_target_name: None,
            current_target_started_at_unix_ms: None,
        },
    );

    let mut target_runs = Vec::new();
    let mut skipped_targets = Vec::new();
    let mut coordination_steps = Vec::new();
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
        let remote_status = probe_remote_service(&loaded.config);
        let should_pause_remote = loaded.config.coordination.pause_remote_on_sync
            && matches!(remote_status.service_state, ServiceState::Active);
        let assume_remote_paused = should_pause_remote;

        if should_pause_remote {
            if dry_run {
                coordination_steps.push(skipped_step(
                    "pause_remote_onedrive",
                    "would pause remote OneDrive service for cycle sync".to_string(),
                    remote_status.detail.clone(),
                ));
            } else {
                let pause_step = pause_remote_onedrive(&loaded.config);
                if pause_step.status == ActionStepStatus::Failed {
                    coordination_steps.push(pause_step);
                    drop(cycle_lock);
                    let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
                    let report = RunCycleReport {
                        config_source,
                        dry_run,
                        outcome: ActionOutcome::Failed,
                        summary: "cycle failed while pausing remote OneDrive".to_string(),
                        preflight_ready: preflight.ready,
                        approved_target_count: approved_selectors.len(),
                        coordination_steps,
                        target_runs,
                        skipped_targets,
                        alerts: alert_report.alerts,
                        notification: None,
                    };
                    record_cycle_run(&loaded.config, &report, started_at_unix_ms);
                    return Ok(report);
                }
                coordination_steps.push(pause_step);
                coordination_steps.push(materialize_remote_instruction_links(
                    &loaded.config,
                    remote_status.selected_host.as_deref().unwrap_or(""),
                ));
                if coordination_steps
                    .last()
                    .is_some_and(|step| step.status == ActionStepStatus::Failed)
                {
                    coordination_steps.push(resume_remote_onedrive(&loaded.config));
                    drop(cycle_lock);
                    let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
                    let report = RunCycleReport {
                        config_source,
                        dry_run,
                        outcome: ActionOutcome::Failed,
                        summary: "cycle failed while materializing remote instruction links"
                            .to_string(),
                        preflight_ready: preflight.ready,
                        approved_target_count: approved_selectors.len(),
                        coordination_steps,
                        target_runs,
                        skipped_targets,
                        alerts: alert_report.alerts,
                        notification: None,
                    };
                    record_cycle_run(&loaded.config, &report, started_at_unix_ms);
                    return Ok(report);
                }
            }
        }

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
                    detail:
                        "update runner.approved_targets so every selector matches a current target"
                            .to_string(),
                });
                continue;
            };

            let selector_for_run = target
                .target_id
                .clone()
                .unwrap_or_else(|| target.name.clone());
            active_cycle_guard.update_target(&selector_for_run, &target.name);
            let report = run_target_inner(
                config_path,
                &selector_for_run,
                dry_run,
                true,
                assume_remote_paused,
                TargetExecutionStrategy::Standard,
            )?;
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

        if should_pause_remote && !dry_run {
            let host = remote_status.selected_host.as_deref().unwrap_or("");
            if !host.is_empty() {
                coordination_steps.push(resume_remote_onedrive(&loaded.config));
            }
        }
    }

    drop(cycle_lock);

    let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
    let notification = if allow_cycle_notification && loaded.config.runner.notify_after_cycle {
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
        coordination_steps,
        target_runs,
        skipped_targets,
        alerts: alert_report.alerts,
        notification,
    };
    record_cycle_run(&loaded.config, &report, started_at_unix_ms);
    drop(active_cycle_guard);
    Ok(report)
}

pub fn runner_tick(config_path: Option<&Path>, dry_run: bool) -> Result<RunnerTickReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let mut state = load_state(&loaded.config.state_path)?;
    let stale_cycle_detail =
        stale_runner_active_cycle_detail(state.runner.active_cycle.as_ref(), std::process::id());
    if stale_cycle_detail.is_some() && !dry_run {
        save_runner_active_cycle(&loaded.config.state_path, None)?;
        state.runner.active_cycle = None;
    }
    let stale_target_operation_detail = stale_active_target_operation_detail(
        state.active_target_operation.as_ref(),
        std::process::id(),
    );
    if stale_target_operation_detail.is_some() && !dry_run {
        save_active_target_operation(&loaded.config.state_path, None)?;
        state.active_target_operation = None;
    }
    let now_unix_ms = now_unix_ms();
    let interval_ms = u128::from(loaded.config.runner.cycle_interval_minutes) * 60 * 1000;
    let last_live_cycle_finished_at_unix_ms = state.runner.last_live_cycle_finished_at_unix_ms;
    let (due, next_due_at_unix_ms) = runner_due_status(
        last_live_cycle_finished_at_unix_ms,
        interval_ms,
        now_unix_ms,
    );

    let mut steps = Vec::new();
    if let Some(detail) = stale_cycle_detail {
        steps.push(if dry_run {
            skipped_step(
                "clear_stale_runner_cycle",
                "would clear stale approved cycle state".to_string(),
                detail,
            )
        } else {
            applied_step(
                "clear_stale_runner_cycle",
                "cleared stale approved cycle state".to_string(),
                detail,
            )
        });
    }
    if let Some(detail) = stale_target_operation_detail {
        steps.push(if dry_run {
            skipped_step(
                "clear_stale_target_operation",
                "would clear stale target operation state".to_string(),
                detail,
            )
        } else {
            applied_step(
                "clear_stale_target_operation",
                "cleared stale target operation state".to_string(),
                detail,
            )
        });
    }
    let (outcome, summary, preflight_ready, cycle, alerts, notification, alert_state_update) =
        if due {
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

            if let Some(active_operation) = &state.active_target_operation {
                steps.push(skipped_step(
                    "runner_manual_operation_in_progress",
                    "manual target operation is already running".to_string(),
                    format!(
                        "{} on {} started at {}",
                        describe_target_operation_kind(active_operation.kind),
                        active_operation.target_name,
                        active_operation.started_at_unix_ms
                    ),
                ));
                let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
                let notification = if loaded.config.runner.notify_after_tick {
                    let notification = scheduled_notify_alerts(
                        &loaded.config,
                        &state,
                        &alert_report.alerts,
                        dry_run,
                    )?;
                    steps.push(notification_step(
                        "runner_notify_alerts",
                        &notification.report,
                    ));
                    Some(notification)
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
                    format!(
                        "runner tick observed an in-progress manual target operation ({} active alerts)",
                        alert_report.alerts.len()
                    ),
                    alert_report.preflight_ready,
                    None,
                    alert_report.alerts,
                    notification.as_ref().map(|item| item.report.clone()),
                    notification.and_then(|item| item.updated_state),
                )
            } else if let Some(active_cycle) = &state.runner.active_cycle {
                steps.push(skipped_step(
                    "runner_cycle_in_progress",
                    "approved target cycle is already running".to_string(),
                    match (
                        &active_cycle.current_target_name,
                        &active_cycle.current_target_selector,
                    ) {
                        (Some(name), Some(selector)) => {
                            format!("current target: {name} ({selector})")
                        }
                        (Some(name), None) => format!("current target: {name}"),
                        _ => format!("cycle started at {}", active_cycle.started_at_unix_ms),
                    },
                ));
                let alert_report = evaluate_alerts(&loaded.config, config_source.clone())?;
                let notification = if loaded.config.runner.notify_after_tick {
                    let notification = scheduled_notify_alerts(
                        &loaded.config,
                        &state,
                        &alert_report.alerts,
                        dry_run,
                    )?;
                    steps.push(notification_step(
                        "runner_notify_alerts",
                        &notification.report,
                    ));
                    Some(notification)
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
                    format!(
                        "runner tick observed an in-progress approved cycle ({} active alerts)",
                        alert_report.alerts.len()
                    ),
                    alert_report.preflight_ready,
                    None,
                    alert_report.alerts,
                    notification.as_ref().map(|item| item.report.clone()),
                    notification.and_then(|item| item.updated_state),
                )
            } else {
                let cycle = run_cycle_inner(config_path, dry_run, false)?;
                steps.push(applied_step(
                    "runner_cycle",
                    format!(
                        "executed approved target cycle ({})",
                        describe_action_outcome(cycle.outcome)
                    ),
                    cycle.summary.clone(),
                ));

                let notification =
                    scheduled_notify_alerts(&loaded.config, &state, &cycle.alerts, dry_run)?;
                let notification_step =
                    notification_step("runner_notify_alerts", &notification.report);
                steps.push(notification_step);

                (
                    cycle.outcome,
                    summarize_runner_tick(true, dry_run, cycle.outcome, &cycle.alerts),
                    cycle.preflight_ready,
                    Some(cycle.clone()),
                    cycle.alerts.clone(),
                    Some(notification.report),
                    notification.updated_state,
                )
            }
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
                let notification =
                    scheduled_notify_alerts(&loaded.config, &state, &alert_report.alerts, dry_run)?;
                steps.push(notification_step(
                    "runner_notify_alerts",
                    &notification.report,
                ));
                Some(notification)
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
                notification.as_ref().map(|item| item.report.clone()),
                notification.and_then(|item| item.updated_state),
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
    if !dry_run {
        if let Some(alert_state) = alert_state_update {
            if let Err(error) =
                save_alert_notification_state(&loaded.config.state_path, alert_state)
            {
                eprintln!("syncsteward: failed to record alert notification state: {error}");
            }
        }
    }
    record_runner_tick(&loaded.config, &report);
    Ok(report)
}

fn stale_runner_active_cycle_detail(
    active_cycle: Option<&RunnerActiveCycleState>,
    current_pid: u32,
) -> Option<String> {
    let active_cycle = active_cycle?;
    if let Some(process_id) = active_cycle.process_id {
        if process_id == current_pid || process_is_alive(process_id) {
            return None;
        }
        return Some(format!(
            "recorded runner process {process_id} is no longer active; cycle started at {}",
            active_cycle.started_at_unix_ms
        ));
    }

    if other_runner_tick_process_exists(current_pid) {
        None
    } else {
        Some(format!(
            "cycle started at {} but no other runner-tick process is active",
            active_cycle.started_at_unix_ms
        ))
    }
}

fn stale_active_target_operation_detail(
    active_operation: Option<&ActiveTargetOperationState>,
    current_pid: u32,
) -> Option<String> {
    let active_operation = active_operation?;
    if let Some(process_id) = active_operation.process_id {
        if process_id == current_pid || process_is_alive(process_id) {
            return None;
        }
        return Some(format!(
            "recorded target operation process {process_id} is no longer active; {} on {} started at {}",
            describe_target_operation_kind(active_operation.kind),
            active_operation.target_name,
            active_operation.started_at_unix_ms
        ));
    }

    if other_target_operation_process_exists(current_pid) {
        None
    } else {
        Some(format!(
            "{} on {} started at {} but no target-operation process is active",
            describe_target_operation_kind(active_operation.kind),
            active_operation.target_name,
            active_operation.started_at_unix_ms
        ))
    }
}

fn process_is_alive(pid: u32) -> bool {
    let pid_arg = pid.to_string();
    run_command("ps", ["-p", pid_arg.as_str(), "-o", "pid="]).success
}

fn other_runner_tick_process_exists(current_pid: u32) -> bool {
    let output = run_command("ps", ["-axo", "pid=,command="]);
    if !output.success {
        return false;
    }

    output.stdout.lines().any(|line| {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(pid_text) = parts.next() else {
            return false;
        };
        let Some(command) = parts.next() else {
            return false;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            return false;
        };
        pid != current_pid && command.contains("syncsteward-cli") && command.contains("runner-tick")
    })
}

fn other_target_operation_process_exists(current_pid: u32) -> bool {
    let output = run_command("ps", ["-axo", "pid=,command="]);
    if !output.success {
        return false;
    }

    output.stdout.lines().any(|line| {
        let trimmed = line.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(pid_text) = parts.next() else {
            return false;
        };
        let Some(command) = parts.next() else {
            return false;
        };
        let Ok(pid) = pid_text.trim().parse::<u32>() else {
            return false;
        };
        if pid == current_pid {
            return false;
        }

        command.contains(" run-target ")
            || command.contains(" verify-target ")
            || command.contains(" repair-target ")
            || command.contains(" rebaseline-target ")
    })
}

fn load_runtime_state(config: &AppConfig) -> Result<AppState> {
    let mut state = load_state(&config.state_path)?;
    let current_pid = std::process::id();

    if stale_runner_active_cycle_detail(state.runner.active_cycle.as_ref(), current_pid).is_some() {
        save_runner_active_cycle(&config.state_path, None)?;
        state.runner.active_cycle = None;
    }

    if stale_active_target_operation_detail(state.active_target_operation.as_ref(), current_pid)
        .is_some()
    {
        save_active_target_operation(&config.state_path, None)?;
        state.active_target_operation = None;
    }

    Ok(state)
}

fn describe_target_operation_kind(kind: TargetOperationKind) -> &'static str {
    match kind {
        TargetOperationKind::Run => "run",
        TargetOperationKind::Verify => "verification",
        TargetOperationKind::Repair => "repair",
        TargetOperationKind::Rebaseline => "rebaseline",
    }
}

fn describe_verification_mode(mode: VerificationMode) -> &'static str {
    match mode {
        VerificationMode::Full => "full",
        VerificationMode::SizeAndSample => "size_and_sample",
    }
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
    let executable_path =
        std::env::current_exe().map_err(|error| anyhow!("resolve current executable: {error}"))?;
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

pub fn config_snapshot(config_path: Option<&Path>) -> Result<ConfigSnapshotReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let path = match config_path {
        Some(path) => Some(expand_path(path)),
        None => match &loaded.source {
            crate::config::ConfigSource::Explicit(path)
            | crate::config::ConfigSource::DefaultFile(path) => Some(path.clone()),
            crate::config::ConfigSource::BuiltInDefaults => None,
        },
    };

    Ok(ConfigSnapshotReport {
        config_source,
        path,
        config: loaded.config,
    })
}

pub fn config_schema(config_path: Option<&Path>) -> Result<ConfigSchemaReport> {
    let loaded = load_config(config_path)?;
    let schema = schema_for!(AppConfig);
    Ok(ConfigSchemaReport {
        config_source: loaded.source.description(),
        schema: serde_json::to_value(schema)?,
    })
}

pub fn update_config(
    config_path: Option<&Path>,
    patch: ConfigPatch,
    dry_run: bool,
) -> Result<ConfigUpdateReport> {
    let output_path = config_path
        .map(expand_path)
        .unwrap_or_else(default_config_path);
    let existed = output_path.exists();
    let loaded = if existed {
        load_config(Some(output_path.as_path()))?
    } else {
        crate::config::LoadedConfig {
            config: normalize_app_config(AppConfig::default())?,
            source: crate::config::ConfigSource::BuiltInDefaults,
        }
    };
    let path_display = output_path.display().to_string();

    let original = loaded.config.clone();
    let mut config = original.clone();
    let mut changed_fields = Vec::new();
    apply_config_patch(&mut config, patch, &mut changed_fields);
    let normalized = normalize_app_config(config)?;

    if normalized == original {
        return Ok(ConfigUpdateReport {
            config_source: loaded.source.description(),
            path: output_path,
            backup_path: None,
            dry_run,
            created: false,
            outcome: ActionOutcome::NoOp,
            summary: if dry_run {
                "dry run confirmed config already matches the requested values".to_string()
            } else {
                "config already matches the requested values".to_string()
            },
            changed_fields,
            config: normalized,
        });
    }

    if !dry_run {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let write_result = write_config(&output_path, normalized.clone())?;
        let ConfigWriteResult {
            normalized,
            backup_path,
        } = write_result;
        return Ok(ConfigUpdateReport {
            config_source: loaded.source.description(),
            path: output_path.clone(),
            backup_path,
            dry_run,
            created: !existed && !dry_run,
            outcome: ActionOutcome::Success,
            summary: if dry_run {
                if existed {
                    format!("dry run would update config at {}", path_display)
                } else {
                    format!(
                        "dry run would create config at {} from built-in defaults",
                        path_display
                    )
                }
            } else if existed {
                format!("updated config at {}", path_display)
            } else {
                format!("created config at {}", path_display)
            },
            changed_fields,
            config: normalized,
        });
    }
    Ok(ConfigUpdateReport {
        config_source: loaded.source.description(),
        path: output_path.clone(),
        backup_path: None,
        dry_run,
        created: !existed && !dry_run,
        outcome: ActionOutcome::Success,
        summary: if dry_run {
            if existed {
                format!("dry run would update config at {}", path_display)
            } else {
                format!(
                    "dry run would create config at {} from built-in defaults",
                    path_display
                )
            }
        } else if existed {
            format!("updated config at {}", path_display)
        } else {
            format!("created config at {}", path_display)
        },
        changed_fields,
        config: normalized,
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
    let folder_policy_count = config.policy.folders.len();
    let file_class_policy_count = config.policy.file_classes.len();

    let ConfigWriteResult {
        normalized: _config,
        backup_path,
    } = write_config(&output_path, config)?;

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
        backup_path,
        overwritten: overwrite,
        folder_policy_count,
        file_class_policy_count,
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
            backup_path: None,
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
            backup_path: None,
            assigned_count: 0,
            preserved_count,
            assignments,
        });
    }

    let ConfigWriteResult {
        normalized: _config,
        backup_path,
    } = write_config(&output_path, config)?;

    Ok(EnsureTargetIdsReport {
        outcome: ActionOutcome::Success,
        summary: format!(
            "assigned {} managed target IDs in {}",
            assignments.len(),
            output_path.display()
        ),
        path: output_path,
        backup_path,
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

    let ConfigWriteResult {
        normalized,
        backup_path,
    } = write_config(&output_path, config)?;
    let target = resolve_managed_inventory_target(&normalized, &target_id, &output_path)?;

    Ok(AddManagedTargetReport {
        outcome: ActionOutcome::Success,
        summary: format!("added managed target {}", target.name),
        path: output_path,
        backup_path,
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
            backup_path: None,
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

    let ConfigWriteResult {
        normalized,
        backup_path,
    } = write_config(&output_path, config)?;
    let target = resolve_managed_inventory_target(&normalized, &target_id, &output_path)?;

    Ok(RelocateManagedTargetReport {
        outcome: ActionOutcome::Success,
        summary: format!("relocated managed target {}", target.name),
        path: output_path,
        backup_path,
        selector: selector.to_string(),
        previous_local_path,
        previous_remote_path,
        target,
    })
}

pub fn prune_state(config_path: Option<&Path>, dry_run: bool) -> Result<PruneStateReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
    let state = load_state(&loaded.config.state_path)?;

    let mut keep_keys = std::collections::BTreeSet::new();
    for target in &inventory.targets {
        keep_keys.insert(target_state_key(target));
        keep_keys.insert(target.name.clone());
        keep_keys.insert(target.local_path.display().to_string());
    }

    let removed_keys = state
        .target_runs
        .keys()
        .filter(|key| !keep_keys.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    let remaining_count = state.target_runs.len().saturating_sub(removed_keys.len());

    if removed_keys.is_empty() {
        return Ok(PruneStateReport {
            config_source,
            path: loaded.config.state_path.clone(),
            dry_run,
            outcome: ActionOutcome::NoOp,
            summary: "no stale target-run state needed pruning".to_string(),
            removed_count: 0,
            remaining_count,
            removed_keys,
        });
    }

    if !dry_run {
        let pruned_keys = crate::state::prune_target_runs(&loaded.config.state_path, &keep_keys)?;
        debug_assert_eq!(pruned_keys, removed_keys);
    }

    Ok(PruneStateReport {
        config_source,
        path: loaded.config.state_path.clone(),
        dry_run,
        outcome: ActionOutcome::Success,
        summary: if dry_run {
            format!(
                "dry run would prune {} stale target-run entr{}",
                removed_keys.len(),
                if removed_keys.len() == 1 { "y" } else { "ies" }
            )
        } else {
            format!(
                "pruned {} stale target-run entr{}",
                removed_keys.len(),
                if removed_keys.len() == 1 { "y" } else { "ies" }
            )
        },
        removed_count: removed_keys.len(),
        remaining_count,
        removed_keys,
    })
}

pub fn quarantine_artifacts(
    config_path: Option<&Path>,
    selectors: &[String],
    dry_run: bool,
) -> Result<ArtifactQuarantineReport> {
    let loaded = load_config(config_path)?;
    let config_source = loaded.source.description();
    let roots_scanned = if selectors.is_empty() {
        status_candidate_roots(&loaded.config, &config_source)
    } else {
        let inventory = build_target_inventory(&loaded.config, config_source.clone())?;
        let mut roots = Vec::new();
        for selector in selectors {
            let target = resolve_inventory_target(inventory.targets.clone(), selector)?;
            if !roots.contains(&target.local_path) {
                roots.push(target.local_path);
            }
        }
        roots
    };

    let artifacts = collect_artifact_matches(&roots_scanned);
    let conflict_count = artifacts
        .iter()
        .filter(|artifact| artifact.kind == ArtifactKind::Conflict)
        .count();
    let safe_backup_count = artifacts
        .iter()
        .filter(|artifact| artifact.kind == ArtifactKind::SafeBackup)
        .count();
    let quarantine_root = artifact_quarantine_root(&loaded.config);

    if artifacts.is_empty() {
        return Ok(ArtifactQuarantineReport {
            config_source,
            dry_run,
            outcome: ActionOutcome::NoOp,
            summary: if selectors.is_empty() {
                "no conflict or safeBackup artifacts matched the configured scan roots".to_string()
            } else {
                format!(
                    "no conflict or safeBackup artifacts matched {} selector(s)",
                    selectors.len()
                )
            },
            selectors: selectors.to_vec(),
            roots_scanned,
            quarantine_root,
            manifest_path: None,
            conflict_count,
            safe_backup_count,
            moved_count: 0,
            artifacts: Vec::new(),
        });
    }

    let mut records = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let scope = quarantine_scope_path(&artifact.root);
        let relative = artifact
            .path
            .strip_prefix(&artifact.root)
            .unwrap_or(&artifact.path);
        let destination = quarantine_root.join(&scope).join(relative);
        if !dry_run {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("create artifact quarantine directory {}", parent.display())
                })?;
            }
            fs::rename(&artifact.path, &destination).with_context(|| {
                format!(
                    "move artifact {} to {}",
                    artifact.path.display(),
                    destination.display()
                )
            })?;
        }
        records.push(ArtifactQuarantineRecord {
            kind: artifact.kind,
            source_path: artifact.path,
            quarantine_path: destination,
        });
    }

    let manifest_path = if dry_run {
        None
    } else {
        fs::create_dir_all(&quarantine_root).with_context(|| {
            format!(
                "create artifact quarantine root {}",
                quarantine_root.display()
            )
        })?;
        let path = quarantine_root.join("manifest.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(&records).context("serialize artifact manifest")?,
        )
        .with_context(|| format!("write artifact manifest at {}", path.display()))?;
        Some(path)
    };

    Ok(ArtifactQuarantineReport {
        config_source,
        dry_run,
        outcome: ActionOutcome::Success,
        summary: format!(
            "{} {} artifact(s) into quarantine",
            if dry_run { "would move" } else { "moved" },
            records.len()
        ),
        selectors: selectors.to_vec(),
        roots_scanned,
        quarantine_root,
        manifest_path,
        conflict_count,
        safe_backup_count,
        moved_count: records.len(),
        artifacts: records,
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

struct ConfigWriteResult {
    normalized: AppConfig,
    backup_path: Option<PathBuf>,
}

fn write_config(path: &Path, config: AppConfig) -> Result<ConfigWriteResult> {
    let normalized = normalize_app_config(config)?;
    let encoded = toml::to_string_pretty(&normalized)?;
    let backup_path = if path.exists() {
        Some(backup_config_file(path)?)
    } else {
        None
    };
    write_atomic_file(path, &encoded)?;
    Ok(ConfigWriteResult {
        normalized,
        backup_path,
    })
}

fn backup_config_file(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("config path must have a file name"))?;
    let backup_path = path.with_file_name(format!("{file_name}.bak-{}", Uuid::now_v7()));
    fs::copy(path, &backup_path)?;
    Ok(backup_path)
}

fn write_atomic_file(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow!(
            "config path must have a parent directory: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("config path must have a file name"))?;
    let temp_path = parent.join(format!(".{file_name}.tmp-{}", Uuid::now_v7()));

    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp_path, path)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

fn apply_config_patch(
    config: &mut AppConfig,
    patch: ConfigPatch,
    changed_fields: &mut Vec<String>,
) {
    if let Some(value) = patch.launch_agent_label {
        config.launch_agent_label = value;
        changed_fields.push("launch_agent_label".to_string());
    }
    if let Some(value) = patch.launch_agent_path {
        config.launch_agent_path = value;
        changed_fields.push("launch_agent_path".to_string());
    }
    if let Some(value) = patch.sync_script_path {
        config.sync_script_path = value;
        changed_fields.push("sync_script_path".to_string());
    }
    if let Some(value) = patch.rclone_log_dir {
        config.rclone_log_dir = value;
        changed_fields.push("rclone_log_dir".to_string());
    }
    if let Some(value) = patch.ssh_key_path {
        config.ssh_key_path = value;
        changed_fields.push("ssh_key_path".to_string());
    }
    if let Some(value) = patch.sync_filter_path {
        config.sync_filter_path = value;
        changed_fields.push("sync_filter_path".to_string());
    }
    if let Some(value) = patch.memloft_filter_path {
        config.memloft_filter_path = value;
        changed_fields.push("memloft_filter_path".to_string());
    }
    if let Some(value) = patch.legacy_lock_path {
        config.legacy_lock_path = value;
        changed_fields.push("legacy_lock_path".to_string());
    }
    if let Some(value) = patch.audit_log_path {
        config.audit_log_path = value;
        changed_fields.push("audit_log_path".to_string());
    }
    if let Some(value) = patch.state_path {
        config.state_path = value;
        changed_fields.push("state_path".to_string());
    }
    if let Some(remote) = patch.remote {
        if let Some(value) = remote.ssh_user {
            config.remote.ssh_user = value;
            changed_fields.push("remote.ssh_user".to_string());
        }
        if let Some(value) = remote.preferred_hosts {
            config.remote.preferred_hosts = value;
            changed_fields.push("remote.preferred_hosts".to_string());
        }
        if let Some(value) = remote.onedrive_service {
            config.remote.onedrive_service = value;
            changed_fields.push("remote.onedrive_service".to_string());
        }
        if let Some(value) = remote.rclone_ssh_mode {
            config.remote.rclone_ssh_mode = value;
            changed_fields.push("remote.rclone_ssh_mode".to_string());
        }
        if let Some(value) = remote.onedrive_service_scope {
            config.remote.onedrive_service_scope = value;
            changed_fields.push("remote.onedrive_service_scope".to_string());
        }
        if let Some(value) = remote.sync_root {
            config.remote_sync_root = value;
            changed_fields.push("remote.sync_root".to_string());
        }
        if let Some(value) = remote.sudo_password_op_reference {
            config.remote.sudo_password_op_reference = Some(value);
            changed_fields.push("remote.sudo_password_op_reference".to_string());
        }
        if let Some(value) = remote.sudo_password_keychain_service {
            config.remote.sudo_password_keychain_service = Some(value);
            changed_fields.push("remote.sudo_password_keychain_service".to_string());
        }
        if let Some(value) = remote.sudo_password_keychain_account {
            config.remote.sudo_password_keychain_account = Some(value);
            changed_fields.push("remote.sudo_password_keychain_account".to_string());
        }
    }
    if let Some(scan) = patch.scan {
        if let Some(value) = scan.roots {
            config.scan.roots = value;
            changed_fields.push("scan.roots".to_string());
        }
        if let Some(value) = scan.max_examples {
            config.scan.max_examples = value;
            changed_fields.push("scan.max_examples".to_string());
        }
    }
    if let Some(value) = patch.managed_targets {
        config.managed_targets = value;
        changed_fields.push("managed_targets".to_string());
    }
    if let Some(coordination) = patch.coordination {
        if let Some(value) = coordination.pause_remote_on_sync {
            config.coordination.pause_remote_on_sync = value;
            changed_fields.push("coordination.pause_remote_on_sync".to_string());
        }
        if let Some(value) = coordination.materialize_instruction_links {
            config.coordination.materialize_instruction_links = value;
            changed_fields.push("coordination.materialize_instruction_links".to_string());
        }
        if let Some(value) = coordination.instruction_file_names {
            config.coordination.instruction_file_names = value;
            changed_fields.push("coordination.instruction_file_names".to_string());
        }
    }
    if let Some(value) = patch.alerts {
        config.alerts = value;
        changed_fields.push("alerts".to_string());
    }
    if let Some(runner) = patch.runner {
        if let Some(value) = runner.approved_targets {
            config.runner.approved_targets = value;
            changed_fields.push("runner.approved_targets".to_string());
        }
        if let Some(value) = runner.cycle_interval_minutes {
            config.runner.cycle_interval_minutes = value;
            changed_fields.push("runner.cycle_interval_minutes".to_string());
        }
        if let Some(value) = runner.notify_after_cycle {
            config.runner.notify_after_cycle = value;
            changed_fields.push("runner.notify_after_cycle".to_string());
        }
        if let Some(value) = runner.notify_after_tick {
            config.runner.notify_after_tick = value;
            changed_fields.push("runner.notify_after_tick".to_string());
        }
        if let Some(launch_agent) = runner.launch_agent {
            if let Some(value) = launch_agent.label {
                config.runner.launch_agent.label = value;
                changed_fields.push("runner.launch_agent.label".to_string());
            }
            if let Some(value) = launch_agent.plist_path {
                config.runner.launch_agent.plist_path = value;
                changed_fields.push("runner.launch_agent.plist_path".to_string());
            }
            if let Some(value) = launch_agent.tick_interval_minutes {
                config.runner.launch_agent.tick_interval_minutes = value;
                changed_fields.push("runner.launch_agent.tick_interval_minutes".to_string());
            }
            if let Some(value) = launch_agent.stdout_path {
                config.runner.launch_agent.stdout_path = value;
                changed_fields.push("runner.launch_agent.stdout_path".to_string());
            }
            if let Some(value) = launch_agent.stderr_path {
                config.runner.launch_agent.stderr_path = value;
                changed_fields.push("runner.launch_agent.stderr_path".to_string());
            }
            if let Some(value) = launch_agent.run_at_load {
                config.runner.launch_agent.run_at_load = value;
                changed_fields.push("runner.launch_agent.run_at_load".to_string());
            }
        }
    }
    if let Some(value) = patch.policy {
        config.policy = value;
        changed_fields.push("policy".to_string());
    }
    if let Some(verification) = patch.verification {
        if let Some(value) = verification.default_mode {
            config.verification.default_mode = value;
            changed_fields.push("verification.default_mode".to_string());
        }
        if let Some(value) = verification.sample_file_count {
            config.verification.sample_file_count = value;
            changed_fields.push("verification.sample_file_count".to_string());
        }
        if let Some(value) = verification.full_after_hours {
            config.verification.full_after_hours = value;
            changed_fields.push("verification.full_after_hours".to_string());
        }
    }
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
    let artifact_roots = status_candidate_roots(config, &config_source);
    collect_status_with_artifact_roots(config, config_source, artifact_roots)
}

fn collect_target_status(
    config: &AppConfig,
    config_source: String,
    artifact_roots: &[PathBuf],
) -> StatusReport {
    collect_status_with_artifact_roots(config, config_source, artifact_roots.to_vec())
}

fn collect_status_with_artifact_roots(
    config: &AppConfig,
    config_source: String,
    artifact_roots: Vec<PathBuf>,
) -> StatusReport {
    let state = load_runtime_state(config).ok();
    let acknowledged_log = state
        .as_ref()
        .and_then(|item| item.acknowledged_log.clone());
    let launch_agent =
        probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
    let runner_agent = probe_launch_agent(
        &config.runner.launch_agent.label,
        Some(&config.runner.launch_agent.plist_path),
    );
    let remote = probe_remote_service(config);
    let artifacts = scan_artifacts(config, &artifact_roots);
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
        active_target_operation: state
            .as_ref()
            .and_then(|item| item.active_target_operation.as_ref())
            .map(map_active_target_operation_summary),
        remote,
        artifacts,
        acknowledged_log,
        latest_log,
    }
}

fn status_candidate_roots(config: &AppConfig, config_source: &str) -> Vec<PathBuf> {
    let mut candidate_roots = config.scan.roots.clone();
    let inventory_targets = build_target_inventory(config, config_source.to_string())
        .map(|inventory| inventory.targets)
        .unwrap_or_else(|_| {
            config
                .managed_targets
                .iter()
                .map(|target| crate::model::SyncTargetRecord {
                    target_id: target.target_id.clone(),
                    name: target.name.clone(),
                    local_path: target.local_path.clone(),
                    remote_path: target.remote_path.clone(),
                    legacy_mode: crate::model::LegacySyncMode::Managed,
                    recommended_mode: target.mode,
                    configured_mode: Some(target.mode),
                    rationale: target.rationale.clone().unwrap_or_else(|| {
                        "Managed target defined explicitly in SyncSteward config.".to_string()
                    }),
                })
                .collect::<Vec<_>>()
        });
    for target in inventory_targets {
        if !candidate_roots.contains(&target.local_path) {
            candidate_roots.push(target.local_path);
        }
    }
    candidate_roots
}

fn map_active_target_operation_summary(
    operation: &ActiveTargetOperationState,
) -> ActiveTargetOperationSummary {
    ActiveTargetOperationSummary {
        kind: operation.kind,
        dry_run: operation.dry_run,
        started_at_unix_ms: operation.started_at_unix_ms,
        process_id: operation.process_id,
        selector: operation.selector.clone(),
        target_name: operation.target_name.clone(),
        target_id: operation.target_id.clone(),
        local_path: operation.local_path.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreflightMode {
    Strict,
    ManagedRun,
}

fn evaluate_preflight(status: StatusReport, mode: PreflightMode) -> PreflightReport {
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
        ServiceState::Active
            if matches!(mode, PreflightMode::ManagedRun) && status.remote.coordination_enabled =>
        {
            warn_check(
                "remote_onedrive_paused",
                "remote OneDrive service is active but SyncSteward can pause it".to_string(),
                status.remote.detail.clone(),
            )
        }
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
        ServiceState::Failed => warn_check(
            "remote_onedrive_paused",
            "remote OneDrive service is failed but not actively running".to_string(),
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

fn target_is_runner_approved(
    approved_selectors: &[String],
    target: &crate::model::SyncTargetRecord,
) -> bool {
    approved_selectors.iter().any(|selector| {
        let selector_path = expand_path(Path::new(selector));
        target_matches_selector(target, selector, &selector_path)
    })
}

fn evaluate_alerts(config: &AppConfig, config_source: String) -> Result<AlertReport> {
    let status = collect_status(config, config_source.clone());
    let preflight = evaluate_preflight(status, PreflightMode::ManagedRun);
    let inventory = build_target_inventory(config, config_source.clone())?;
    let state = load_runtime_state(config)?;
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

        let run_state = lookup_target_run_state(&state, &evaluation.target);
        let runner_approved =
            target_is_runner_approved(&config.runner.approved_targets, &evaluation.target);

        if !runner_approved {
            let Some(run_state) = run_state else {
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
                if run_state.consecutive_failure_count >= 3 {
                    alerts.push(AlertRecord {
                        id: format!("target_{}_chronic_failure", evaluation.target.name),
                        severity: AlertSeverity::Critical,
                        summary: format!(
                            "{} has {} consecutive failed live runs",
                            evaluation.target.name, run_state.consecutive_failure_count
                        ),
                        detail: format!(
                            "latest outcome {:?} recorded at unix_ms {}",
                            run_state.outcome, run_state.finished_at_unix_ms
                        ),
                        target_name: Some(evaluation.target.name.clone()),
                    });
                }
            }
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
            if let Some(run_state) = run_state {
                if run_state.outcome != ActionOutcome::Success
                    && run_state.consecutive_failure_count >= 3
                {
                    alerts.push(AlertRecord {
                        id: format!("target_{}_chronic_failure", evaluation.target.name),
                        severity: AlertSeverity::Critical,
                        summary: format!(
                            "{} has {} consecutive failed live runs",
                            evaluation.target.name, run_state.consecutive_failure_count
                        ),
                        detail: format!(
                            "latest outcome {:?} recorded at unix_ms {}",
                            run_state.outcome, run_state.finished_at_unix_ms
                        ),
                        target_name: Some(evaluation.target.name.clone()),
                    });
                }
            }
            continue;
        }

        let Some(run_state) = run_state else {
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
            if run_state.consecutive_failure_count >= 3 {
                alerts.push(AlertRecord {
                    id: format!("target_{}_chronic_failure", evaluation.target.name),
                    severity: AlertSeverity::Critical,
                    summary: format!(
                        "{} has {} consecutive failed live runs",
                        evaluation.target.name, run_state.consecutive_failure_count
                    ),
                    detail: format!(
                        "latest outcome {:?} recorded at unix_ms {}",
                        run_state.outcome, run_state.finished_at_unix_ms
                    ),
                    target_name: Some(evaluation.target.name.clone()),
                });
            }
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

        if run_state.consecutive_failure_count >= 3 {
            alerts.push(AlertRecord {
                id: format!("target_{}_chronic_failure", evaluation.target.name),
                severity: AlertSeverity::Critical,
                summary: format!(
                    "{} has {} consecutive failed live runs",
                    evaluation.target.name, run_state.consecutive_failure_count
                ),
                detail: format!(
                    "latest outcome {:?} recorded at unix_ms {}",
                    run_state.outcome, run_state.finished_at_unix_ms
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
        repeat_notification_after_minutes: config.alerts.repeat_notification_after_minutes,
        alerts,
    })
}

fn build_runner_overview(
    config: &AppConfig,
    runner_agent: &LaunchAgentStatus,
    state: &AppState,
    now_unix_ms: u128,
) -> RunnerOverview {
    let interval_ms = u128::from(config.runner.cycle_interval_minutes) * 60 * 1000;
    let (due, next_due_at_unix_ms) = runner_due_status(
        state.runner.last_live_cycle_finished_at_unix_ms,
        interval_ms,
        now_unix_ms,
    );

    RunnerOverview {
        agent: runner_agent.clone(),
        cycle_interval_minutes: config.runner.cycle_interval_minutes,
        tick_interval_minutes: config.runner.launch_agent.tick_interval_minutes,
        due,
        last_live_cycle_finished_at_unix_ms: state.runner.last_live_cycle_finished_at_unix_ms,
        next_due_at_unix_ms,
        active_cycle: state
            .runner
            .active_cycle
            .as_ref()
            .map(|cycle| RunnerActiveCycleSummary {
                dry_run: cycle.dry_run,
                started_at_unix_ms: cycle.started_at_unix_ms,
                current_target_selector: cycle.current_target_selector.clone(),
                current_target_name: cycle.current_target_name.clone(),
                current_target_started_at_unix_ms: cycle.current_target_started_at_unix_ms,
            }),
        last_cycle: state
            .runner
            .last_cycle
            .as_ref()
            .map(|cycle| RunnerCycleSummary {
                dry_run: cycle.dry_run,
                started_at_unix_ms: cycle.started_at_unix_ms,
                finished_at_unix_ms: cycle.finished_at_unix_ms,
                outcome: cycle.outcome,
                approved_target_count: cycle.approved_target_count,
                active_alert_count: cycle.active_alert_count,
                summary: cycle.summary.clone(),
            }),
        last_tick: state
            .runner
            .last_tick
            .as_ref()
            .map(|tick| RunnerTickSummary {
                dry_run: tick.dry_run,
                finished_at_unix_ms: tick.finished_at_unix_ms,
                due: tick.due,
                outcome: tick.outcome,
                next_due_at_unix_ms: tick.next_due_at_unix_ms,
                summary: tick.summary.clone(),
            }),
    }
}

struct RunnerActiveCycleGuard {
    state_path: PathBuf,
}

impl RunnerActiveCycleGuard {
    fn new(state_path: &Path, cycle: RunnerActiveCycleState) -> Self {
        let state_path = state_path.to_path_buf();
        if let Err(error) = save_runner_active_cycle(&state_path, Some(cycle)) {
            eprintln!("syncsteward: failed to record active runner cycle: {error}");
        }
        Self { state_path }
    }

    fn update_target(&self, selector: &str, target_name: &str) {
        if let Ok(mut state) = load_state(&self.state_path) {
            if let Some(active) = state.runner.active_cycle.as_mut() {
                active.current_target_selector = Some(selector.to_string());
                active.current_target_name = Some(target_name.to_string());
                active.current_target_started_at_unix_ms = Some(now_unix_ms());
                if let Err(error) =
                    save_runner_active_cycle(&self.state_path, state.runner.active_cycle)
                {
                    eprintln!("syncsteward: failed to update active runner cycle: {error}");
                }
            }
        }
    }
}

impl Drop for RunnerActiveCycleGuard {
    fn drop(&mut self) {
        if let Err(error) = save_runner_active_cycle(&self.state_path, None) {
            eprintln!("syncsteward: failed to clear active runner cycle: {error}");
        }
    }
}

struct ActiveTargetOperationGuard {
    state_path: PathBuf,
}

impl ActiveTargetOperationGuard {
    fn for_selector(
        config_path: Option<&Path>,
        selector: &str,
        kind: TargetOperationKind,
        dry_run: bool,
    ) -> Result<Self> {
        let loaded = load_config(config_path)?;
        let inventory = build_target_inventory(&loaded.config, loaded.source.description())?;
        let target = resolve_inventory_target(inventory.targets, selector)?;
        Ok(Self::new(
            &loaded.config.state_path,
            ActiveTargetOperationState {
                kind,
                dry_run,
                started_at_unix_ms: now_unix_ms(),
                process_id: Some(std::process::id()),
                selector: selector.to_string(),
                target_name: target.name,
                target_id: target.target_id,
                local_path: target.local_path,
            },
        ))
    }

    fn new(state_path: &Path, operation: ActiveTargetOperationState) -> Self {
        let state_path = state_path.to_path_buf();
        if let Err(error) = save_active_target_operation(&state_path, Some(operation)) {
            eprintln!("syncsteward: failed to record active target operation: {error}");
        }
        Self { state_path }
    }
}

impl Drop for ActiveTargetOperationGuard {
    fn drop(&mut self) {
        if let Err(error) = save_active_target_operation(&self.state_path, None) {
            eprintln!("syncsteward: failed to clear active target operation: {error}");
        }
    }
}

fn build_target_health_overview(
    evaluations: &[TargetEvaluation],
    approved_targets: &[ApprovedTargetOverview],
    state: &AppState,
    chronic_failure_target_count: usize,
) -> TargetHealthOverview {
    let ready_target_count = evaluations
        .iter()
        .filter(|evaluation| evaluation.ready)
        .count();
    let live_success_target_count = evaluations
        .iter()
        .filter(|evaluation| {
            lookup_target_run_state(state, &evaluation.target)
                .and_then(|run_state| run_state.last_success_at_unix_ms)
                .is_some()
        })
        .count();
    let verified_target_count = evaluations
        .iter()
        .filter(|evaluation| {
            lookup_target_run_state(state, &evaluation.target)
                .and_then(|run_state| run_state.last_verified_at_unix_ms)
                .is_some()
        })
        .count();

    TargetHealthOverview {
        total_target_count: evaluations.len(),
        managed_target_count: evaluations
            .iter()
            .filter(|evaluation| {
                matches!(
                    evaluation.target.legacy_mode,
                    crate::model::LegacySyncMode::Managed
                )
            })
            .count(),
        approved_target_count: approved_targets.len(),
        resolved_approved_target_count: approved_targets
            .iter()
            .filter(|target| target.resolved)
            .count(),
        ready_target_count,
        blocked_target_count: evaluations.len().saturating_sub(ready_target_count),
        ready_approved_target_count: approved_targets
            .iter()
            .filter(|target| {
                target
                    .evaluation
                    .as_ref()
                    .map(|evaluation| evaluation.ready)
                    .unwrap_or(false)
            })
            .count(),
        live_success_target_count,
        verified_target_count,
        chronic_failure_target_count,
    }
}

fn build_chronic_failure_overview(
    approved_selectors: &[String],
    evaluations: &[TargetEvaluation],
    state: &AppState,
) -> Vec<crate::model::ChronicFailureOverview> {
    let mut failures = evaluations
        .iter()
        .filter_map(|evaluation| {
            if !target_is_runner_approved(approved_selectors, &evaluation.target) {
                return None;
            }
            let run_state = lookup_target_run_state(state, &evaluation.target)?;
            let count = run_state.consecutive_failure_count;
            if count < 3 {
                return None;
            }

            Some(crate::model::ChronicFailureOverview {
                target_name: run_state.target_name.clone(),
                target_id: run_state.target_id.clone(),
                local_path: run_state.local_path.clone(),
                consecutive_failure_count: count,
                outcome: run_state.outcome,
                last_failure_class: run_state.last_failure_class,
                summary: format!(
                    "{} has failed {} consecutive live runs",
                    run_state.target_name, count
                ),
            })
        })
        .collect::<Vec<_>>();

    failures.sort_by(|left, right| {
        right
            .consecutive_failure_count
            .cmp(&left.consecutive_failure_count)
            .then_with(|| left.target_name.cmp(&right.target_name))
    });

    failures
}

fn build_approved_target_overview(
    approved_selectors: &[String],
    evaluations: &[TargetEvaluation],
    state: &AppState,
) -> Vec<ApprovedTargetOverview> {
    approved_selectors
        .iter()
        .map(|selector| {
            let selector_path = expand_path(Path::new(selector));
            let evaluation = evaluations
                .iter()
                .find(|evaluation| {
                    target_matches_selector(&evaluation.target, selector, &selector_path)
                })
                .cloned();
            let last_run = evaluation
                .as_ref()
                .and_then(|evaluation| lookup_target_run_state(state, &evaluation.target))
                .map(map_recent_target_run_summary);

            match evaluation {
                Some(evaluation) => {
                    let detail = if evaluation.ready {
                        "approved target is ready".to_string()
                    } else {
                        format!("{} blocker(s) still active", evaluation.blockers.len())
                    };
                    ApprovedTargetOverview {
                        selector: selector.clone(),
                        resolved: true,
                        detail,
                        evaluation: Some(evaluation),
                        last_run,
                    }
                }
                None => ApprovedTargetOverview {
                    selector: selector.clone(),
                    resolved: false,
                    detail:
                        "update runner.approved_targets so every selector matches a current target"
                            .to_string(),
                    evaluation: None,
                    last_run: None,
                },
            }
        })
        .collect()
}

fn build_recent_target_run_summaries(state: &AppState) -> Vec<RecentTargetRunSummary> {
    let mut runs = state
        .target_runs
        .values()
        .map(map_recent_target_run_summary)
        .collect::<Vec<_>>();
    runs.sort_by(|left, right| right.finished_at_unix_ms.cmp(&left.finished_at_unix_ms));
    runs
}

fn map_recent_target_run_summary(run_state: &TargetRunState) -> RecentTargetRunSummary {
    RecentTargetRunSummary {
        target_name: run_state.target_name.clone(),
        target_id: run_state.target_id.clone(),
        local_path: run_state.local_path.clone(),
        effective_mode: run_state.effective_mode,
        outcome: run_state.outcome,
        finished_at_unix_ms: run_state.finished_at_unix_ms,
        last_success_at_unix_ms: run_state.last_success_at_unix_ms,
        last_verified_at_unix_ms: run_state.last_verified_at_unix_ms,
        last_full_verified_at_unix_ms: run_state.last_full_verified_at_unix_ms,
        last_verification_mode: run_state.last_verification_mode,
        last_failure_class: run_state.last_failure_class,
        last_repair_at_unix_ms: run_state.last_repair_at_unix_ms,
        last_rebaseline_at_unix_ms: run_state.last_rebaseline_at_unix_ms,
        summary: run_state.summary.clone(),
    }
}

struct LegacyLockGuard {
    path: PathBuf,
    _file: fs::File,
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

    loop {
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                write!(file, "{}", std::process::id())?;
                file.sync_all()?;
                *slot = Some(LegacyLockGuard {
                    path: lock_path.clone(),
                    _file: file,
                });

                return Ok(applied_step(
                    "legacy_lock",
                    format!("acquired legacy sync lock {}", lock_path.display()),
                    "single-target execution is now protected from concurrent legacy runs"
                        .to_string(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
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
                } else {
                    let lock_is_recent = fs::metadata(lock_path)
                        .ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .map(|age| age <= Duration::from_secs(10))
                        .unwrap_or(true);
                    if lock_is_recent {
                        return Ok(blocked_step(
                            "legacy_lock",
                            format!(
                                "legacy sync lock is still active at {}",
                                lock_path.display()
                            ),
                            "lock owner is still being recorded".to_string(),
                        ));
                    }
                }

                match fs::remove_file(lock_path) {
                    Ok(()) => continue,
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {
                        continue;
                    }
                    Err(remove_error) => {
                        return Err(anyhow!(
                            "remove stale legacy lock {}: {remove_error}",
                            lock_path.display()
                        ));
                    }
                }
            }
            Err(error) => {
                return Err(anyhow!(
                    "acquire legacy lock {}: {error}",
                    lock_path.display()
                ));
            }
        }
    }
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

fn execute_rebaseline_purge_target(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    host: &str,
    dry_run: bool,
    temp_dir: &Path,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let rclone_config_path = write_target_rclone_config(config, host, temp_dir)?;
    steps.push(applied_step(
        "write_rebaseline_rclone_config",
        format!("wrote rebaseline rclone config for {}", target.name),
        rclone_config_path.display().to_string(),
    ));

    let empty_dir = temp_dir.join("rebaseline-empty");
    fs::create_dir_all(&empty_dir)?;
    let remote_path = format!("syncsteward-target:{}", target.remote_path);
    let output = run_rclone_command_with_host_retry(
        config,
        host,
        temp_dir,
        rclone_retry_attempts(dry_run),
        Duration::from_secs(2),
        |rclone_config_path| {
            let mut command = Command::new(resolve_program_path("rclone"));
            command
                .env("RCLONE_CONFIG", &rclone_config_path)
                .arg("sync")
                .arg(&empty_dir)
                .arg(&remote_path);
            if dry_run {
                command.arg("--dry-run");
            }
            command
        },
    )?;

    if output.success {
        steps.push(applied_step(
            "rebaseline_remote_target",
            format!(
                "{} rebuilt remote baseline for {}",
                if dry_run { "dry run" } else { "rebaseline" },
                target.name
            ),
            summarize_command_output(&output),
        ));
    } else {
        steps.push(failed_step(
            "rebaseline_remote_target",
            format!(
                "{} failed while clearing remote target for {}",
                if dry_run { "dry run" } else { "rebaseline" },
                target.name
            ),
            summarize_command_output(&output),
        ));
    }

    Ok(())
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
    let output = run_rclone_command_with_host_retry(
        config,
        host,
        temp_dir,
        rclone_retry_attempts(dry_run),
        Duration::from_secs(2),
        |rclone_config_path| {
            let mut command = Command::new(resolve_program_path("rclone"));
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
            command
        },
    )?;

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
    let snapshot_excludes = snapshot_exclusion_patterns(snapshot_policy);
    let non_db_output = run_rclone_command_with_host_retry(
        config,
        host,
        temp_dir,
        rclone_retry_attempts(dry_run),
        Duration::from_secs(2),
        |rclone_config_path| {
            let mut command = Command::new(resolve_program_path("rclone"));
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
            for pattern in &snapshot_excludes {
                command.arg("--exclude").arg(pattern);
            }
            if dry_run {
                command.arg("--dry-run");
            }
            command
        },
    )?;
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
        let upload_output = run_rclone_command_with_host_retry(
            config,
            host,
            temp_dir,
            rclone_retry_attempts(dry_run),
            Duration::from_secs(2),
            |rclone_config_path| {
                let mut upload = Command::new(resolve_program_path("rclone"));
                upload
                    .env("RCLONE_CONFIG", &rclone_config_path)
                    .arg("copyto")
                    .arg(&snapshot_path)
                    .arg(&remote_file);
                if dry_run {
                    upload.arg("--dry-run");
                }
                upload
            },
        )?;
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

fn execute_target_verification(
    config: &AppConfig,
    target: &crate::model::SyncTargetRecord,
    snapshot_policy: Option<&crate::config::TargetSnapshot>,
    verification_mode: VerificationMode,
    host: &str,
    temp_dir: &Path,
    steps: &mut Vec<ActionStep>,
) -> Result<()> {
    let rclone_config_path = write_target_rclone_config(config, host, temp_dir)?;
    steps.push(applied_step(
        "write_verify_rclone_config",
        format!("wrote verification rclone config for {}", target.name),
        rclone_config_path.display().to_string(),
    ));

    let filter_path = build_filter_file(config, target, temp_dir)?;
    steps.push(applied_step(
        "prepare_verify_filters",
        format!("prepared verification filter rules for {}", target.name),
        filter_path.display().to_string(),
    ));

    let remote_path = format!("syncsteward-target:{}", target.remote_path);
    if let Some(snapshot_policy) = snapshot_policy {
        let snapshot_excludes = snapshot_exclusion_patterns(snapshot_policy);
        if verification_mode == VerificationMode::SizeAndSample {
            let non_db_output = run_rclone_command_with_host_retry(
                config,
                host,
                temp_dir,
                rclone_retry_attempts(false),
                Duration::from_secs(2),
                |rclone_config_path| {
                    let mut command = Command::new(resolve_program_path("rclone"));
                    command
                        .env("RCLONE_CONFIG", &rclone_config_path)
                        .arg("check")
                        .arg(&target.local_path)
                        .arg(&remote_path)
                        .arg("--size-only")
                        .arg("--filter-from")
                        .arg(&filter_path)
                        .arg("--skip-links")
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
                    for pattern in &snapshot_excludes {
                        command.arg("--exclude").arg(pattern);
                    }
                    command
                },
            )?;
            if non_db_output.success {
                steps.push(applied_step(
                    "rclone_check_non_db_size",
                    format!("verified non-database file sizes for {}", target.name),
                    summarize_command_output(&non_db_output),
                ));
            } else {
                steps.push(failed_step(
                    "rclone_check_non_db_size",
                    format!(
                        "size verification failed for non-database files in {}",
                        target.name
                    ),
                    summarize_command_output(&non_db_output),
                ));
                return Ok(());
            }

            let non_db_candidates = list_filtered_local_files(
                &target.local_path,
                Some(&filter_path),
                &snapshot_excludes,
            )?;
            verify_sampled_hashes(
                config,
                host,
                &target.local_path,
                &target.remote_path,
                &non_db_candidates,
                config.verification.sample_file_count,
                steps,
                "verify_sampled_hashes_non_db",
                &target.name,
                "non-database files",
            )?;

            let snapshot_root = temp_dir.join("sqlite-verify");
            fs::create_dir_all(&snapshot_root)?;
            let mut snapshot_paths = Vec::new();
            for relative_path in &snapshot_policy.sqlite_paths {
                let source_path = target.local_path.join(relative_path);
                if !source_path.exists() {
                    steps.push(skipped_step(
                        "sqlite_snapshot_verify_backup",
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
                        "sqlite_snapshot_verify_backup",
                        format!(
                            "prepared SQLite verification snapshot for {}",
                            relative_path.display()
                        ),
                        destination_path.display().to_string(),
                    ));
                    snapshot_paths.push(relative_path.clone());
                } else {
                    steps.push(failed_step(
                        "sqlite_snapshot_verify_backup",
                        format!(
                            "failed to prepare SQLite verification snapshot for {}",
                            relative_path.display()
                        ),
                        summarize_command_output(&backup_output),
                    ));
                }
            }

            if steps.iter().any(|step| {
                step.id == "sqlite_snapshot_verify_backup"
                    && step.status == ActionStepStatus::Failed
            }) {
                return Ok(());
            }

            if snapshot_paths.is_empty() {
                steps.push(skipped_step(
                    "rclone_check_snapshots_size",
                    format!(
                        "no SQLite snapshots needed verification for {}",
                        target.name
                    ),
                    "all configured snapshot sources were missing locally".to_string(),
                ));
                return Ok(());
            }

            let snapshot_list_path = temp_dir.join("sqlite-verify-files.txt");
            let snapshot_list = snapshot_paths
                .iter()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(&snapshot_list_path, snapshot_list)?;
            steps.push(applied_step(
                "sqlite_snapshot_verify_list",
                format!("prepared snapshot verification list for {}", target.name),
                snapshot_list_path.display().to_string(),
            ));

            let snapshot_output = run_rclone_command_with_host_retry(
                config,
                host,
                temp_dir,
                rclone_retry_attempts(false),
                Duration::from_secs(2),
                |rclone_config_path| {
                    let mut command = Command::new(resolve_program_path("rclone"));
                    command
                        .env("RCLONE_CONFIG", &rclone_config_path)
                        .arg("check")
                        .arg(&snapshot_root)
                        .arg(&remote_path)
                        .arg("--size-only")
                        .arg("--files-from")
                        .arg(&snapshot_list_path);
                    command
                },
            )?;
            if snapshot_output.success {
                steps.push(applied_step(
                    "rclone_check_snapshots_size",
                    format!("verified SQLite snapshot sizes for {}", target.name),
                    summarize_command_output(&snapshot_output),
                ));
            } else {
                steps.push(failed_step(
                    "rclone_check_snapshots_size",
                    format!(
                        "size verification failed for SQLite snapshots in {}",
                        target.name
                    ),
                    summarize_command_output(&snapshot_output),
                ));
                return Ok(());
            }

            let snapshot_candidates = snapshot_paths
                .iter()
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .collect::<Vec<_>>();
            verify_sampled_hashes(
                config,
                host,
                &snapshot_root,
                &target.remote_path,
                &snapshot_candidates,
                config.verification.sample_file_count,
                steps,
                "verify_sampled_hashes_snapshots",
                &target.name,
                "SQLite snapshots",
            )?;
            return Ok(());
        }

        let non_db_output = run_rclone_command_with_host_retry(
            config,
            host,
            temp_dir,
            rclone_retry_attempts(false),
            Duration::from_secs(2),
            |rclone_config_path| {
                let mut command = Command::new(resolve_program_path("rclone"));
                command
                    .env("RCLONE_CONFIG", &rclone_config_path)
                    .arg("check")
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
                for pattern in &snapshot_excludes {
                    command.arg("--exclude").arg(pattern);
                }
                command
            },
        )?;
        if non_db_output.success {
            steps.push(applied_step(
                "rclone_check_non_db",
                format!("verified non-database files for {}", target.name),
                summarize_command_output(&non_db_output),
            ));
        } else {
            steps.push(failed_step(
                "rclone_check_non_db",
                format!(
                    "verification failed for non-database files in {}",
                    target.name
                ),
                summarize_command_output(&non_db_output),
            ));
            return Ok(());
        }

        let snapshot_root = temp_dir.join("sqlite-verify");
        fs::create_dir_all(&snapshot_root)?;
        let mut snapshot_paths = Vec::new();
        for relative_path in &snapshot_policy.sqlite_paths {
            let source_path = target.local_path.join(relative_path);
            if !source_path.exists() {
                steps.push(skipped_step(
                    "sqlite_snapshot_verify_backup",
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
                    "sqlite_snapshot_verify_backup",
                    format!(
                        "prepared SQLite verification snapshot for {}",
                        relative_path.display()
                    ),
                    destination_path.display().to_string(),
                ));
                snapshot_paths.push(relative_path.clone());
            } else {
                steps.push(failed_step(
                    "sqlite_snapshot_verify_backup",
                    format!(
                        "failed to prepare SQLite verification snapshot for {}",
                        relative_path.display()
                    ),
                    summarize_command_output(&backup_output),
                ));
            }
        }

        if steps.iter().any(|step| {
            step.id == "sqlite_snapshot_verify_backup" && step.status == ActionStepStatus::Failed
        }) {
            return Ok(());
        }

        if snapshot_paths.is_empty() {
            steps.push(skipped_step(
                "rclone_check_snapshots",
                format!(
                    "no SQLite snapshots needed verification for {}",
                    target.name
                ),
                "all configured snapshot sources were missing locally".to_string(),
            ));
            return Ok(());
        }

        let snapshot_list_path = temp_dir.join("sqlite-verify-files.txt");
        let snapshot_list = snapshot_paths
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&snapshot_list_path, snapshot_list)?;
        steps.push(applied_step(
            "sqlite_snapshot_verify_list",
            format!("prepared snapshot verification list for {}", target.name),
            snapshot_list_path.display().to_string(),
        ));

        let snapshot_output = run_rclone_command_with_host_retry(
            config,
            host,
            temp_dir,
            rclone_retry_attempts(false),
            Duration::from_secs(2),
            |rclone_config_path| {
                let mut command = Command::new(resolve_program_path("rclone"));
                command
                    .env("RCLONE_CONFIG", &rclone_config_path)
                    .arg("check")
                    .arg(&snapshot_root)
                    .arg(&remote_path)
                    .arg("--files-from")
                    .arg(&snapshot_list_path);
                command
            },
        )?;
        if snapshot_output.success {
            steps.push(applied_step(
                "rclone_check_snapshots",
                format!("verified SQLite snapshots for {}", target.name),
                summarize_command_output(&snapshot_output),
            ));
        } else {
            steps.push(failed_step(
                "rclone_check_snapshots",
                format!(
                    "verification failed for SQLite snapshots in {}",
                    target.name
                ),
                summarize_command_output(&snapshot_output),
            ));
        }
        return Ok(());
    }

    if verification_mode == VerificationMode::SizeAndSample {
        let output = run_rclone_command_with_host_retry(
            config,
            host,
            temp_dir,
            rclone_retry_attempts(false),
            Duration::from_secs(2),
            |rclone_config_path| {
                let mut command = Command::new(resolve_program_path("rclone"));
                command
                    .env("RCLONE_CONFIG", &rclone_config_path)
                    .arg("check")
                    .arg(&target.local_path)
                    .arg(&remote_path)
                    .arg("--size-only")
                    .arg("--filter-from")
                    .arg(&filter_path)
                    .arg("--skip-links")
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
                command
            },
        )?;
        if output.success {
            steps.push(applied_step(
                "rclone_check_size",
                format!("verified remote file sizes for {}", target.name),
                summarize_command_output(&output),
            ));
        } else {
            steps.push(failed_step(
                "rclone_check_size",
                format!("size verification failed for {}", target.name),
                summarize_command_output(&output),
            ));
            return Ok(());
        }

        let candidates = list_filtered_local_files(&target.local_path, Some(&filter_path), &[])?;
        verify_sampled_hashes(
            config,
            host,
            &target.local_path,
            &target.remote_path,
            &candidates,
            config.verification.sample_file_count,
            steps,
            "verify_sampled_hashes",
            &target.name,
            "files",
        )?;
        return Ok(());
    }

    let output = run_rclone_command_with_host_retry(
        config,
        host,
        temp_dir,
        rclone_retry_attempts(false),
        Duration::from_secs(2),
        |rclone_config_path| {
            let mut command = Command::new(resolve_program_path("rclone"));
            command
                .env("RCLONE_CONFIG", &rclone_config_path)
                .arg("check")
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
            command
        },
    )?;
    if output.success {
        steps.push(applied_step(
            "rclone_check",
            format!("verified remote contents for {}", target.name),
            summarize_command_output(&output),
        ));
    } else {
        steps.push(failed_step(
            "rclone_check",
            format!("verification failed for {}", target.name),
            summarize_command_output(&output),
        ));
    }
    Ok(())
}

fn list_filtered_local_files(
    source_root: &Path,
    filter_path: Option<&Path>,
    extra_excludes: &[String],
) -> Result<Vec<String>> {
    let mut command = Command::new(resolve_program_path("rclone"));
    command
        .arg("lsf")
        .arg(source_root)
        .arg("--files-only")
        .arg("-R")
        .arg("--skip-links");
    if let Some(filter_path) = filter_path {
        command.arg("--filter-from").arg(filter_path);
    }
    command
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
    for pattern in extra_excludes {
        command.arg("--exclude").arg(pattern);
    }

    let output = run_spawned_command(command);
    if !output.success {
        bail!(
            "list verification candidates under {}: {}",
            source_root.display(),
            summarize_command_output(&output)
        );
    }

    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches('/').to_string())
        .collect())
}

fn select_sample_paths(paths: &[String], sample_limit: usize) -> Vec<String> {
    if sample_limit == 0 || paths.is_empty() {
        return Vec::new();
    }
    if paths.len() <= sample_limit {
        return paths.to_vec();
    }

    let max_index = paths.len() - 1;
    let divisor = sample_limit.saturating_sub(1);
    let mut indexes = BTreeSet::new();
    for position in 0..sample_limit {
        let index = if divisor == 0 {
            max_index / 2
        } else {
            position * max_index / divisor
        };
        indexes.insert(index);
    }

    indexes
        .into_iter()
        .map(|index| paths[index].clone())
        .collect()
}

fn verify_sampled_hashes(
    config: &AppConfig,
    host: &str,
    local_root: &Path,
    target_remote_path: &str,
    candidates: &[String],
    sample_limit: usize,
    steps: &mut Vec<ActionStep>,
    step_id: &str,
    target_name: &str,
    sample_label: &str,
) -> Result<()> {
    let sample_paths = select_sample_paths(candidates, sample_limit);
    if sample_paths.is_empty() {
        steps.push(skipped_step(
            step_id,
            format!("no {sample_label} needed sampled hash verification for {target_name}"),
            "candidate file list was empty after filters and exclusions".to_string(),
        ));
        return Ok(());
    }

    let mut mismatches = Vec::new();
    for relative_path in &sample_paths {
        let local_path = local_root.join(relative_path);
        let local_hash = compute_local_md5_hex(&local_path)
            .map_err(|error| anyhow!("hash local {}: {error}", local_path.display()))?;
        let remote_hash = compute_remote_md5_hex(config, host, target_remote_path, relative_path)
            .map_err(|error| anyhow!("hash remote {}: {error}", relative_path))?;
        if local_hash != remote_hash {
            mismatches.push(format!(
                "{relative_path}: hash differ (local {local_hash}, remote {remote_hash})"
            ));
        }
    }

    if mismatches.is_empty() {
        steps.push(applied_step(
            step_id,
            format!(
                "verified {} sampled {} hashes for {}",
                sample_paths.len(),
                sample_label,
                target_name
            ),
            sample_paths.join("\n"),
        ));
    } else {
        steps.push(failed_step(
            step_id,
            format!("sampled hash verification failed for {}", target_name),
            mismatches.join("\n"),
        ));
    }

    Ok(())
}

fn compute_local_md5_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Md5::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn compute_remote_md5_hex(
    config: &AppConfig,
    host: &str,
    target_remote_path: &str,
    relative_path: &str,
) -> Result<String> {
    let remote_root = remote_target_root_for_shell(config, target_remote_path);
    let remote_path = join_remote_shell_path(&remote_root, relative_path);
    let script = format!(
        "md5sum -- {} | awk '{{print $1}}'",
        shell_single_quote(&remote_path)
    );
    let output = run_remote_command(
        config,
        host,
        &format!("bash -lc {}", shell_single_quote(&script)),
    );
    if !output.success {
        bail!("{}", summarize_command_output(&output));
    }

    let hash = output.stdout.split_whitespace().next().unwrap_or_default();
    if hash.is_empty() {
        bail!("remote md5 command returned no hash for {}", remote_path);
    }
    Ok(hash.to_string())
}

fn snapshot_exclusion_patterns(snapshot_policy: &crate::config::TargetSnapshot) -> Vec<String> {
    let mut patterns = std::collections::BTreeSet::new();
    for relative_path in &snapshot_policy.sqlite_paths {
        let normalized = relative_path.to_string_lossy().replace('\\', "/");
        if normalized.is_empty() {
            continue;
        }
        patterns.insert(normalized.clone());
        patterns.insert(format!("{normalized}-journal"));
        patterns.insert(format!("{normalized}-wal"));
        patterns.insert(format!("{normalized}-shm"));
    }
    patterns.into_iter().collect()
}

fn write_target_rclone_config(config: &AppConfig, host: &str, temp_dir: &Path) -> Result<PathBuf> {
    let path = temp_dir.join("rclone.conf");
    write_target_rclone_config_at_path(config, host, &path)?;
    Ok(path)
}

fn write_target_rclone_config_at_path(config: &AppConfig, host: &str, path: &Path) -> Result<()> {
    let contents = if config.remote.rclone_ssh_mode.uses_external_ssh() {
        format!(
            "[syncsteward-target]\n\
type = sftp\n\
ssh = {ssh_command}\n\
shell_type = unix\n\
md5sum_command = md5sum\n\
sha1sum_command = sha1sum\n",
            ssh_command = render_rclone_external_ssh_command(config, host),
        )
    } else {
        format!(
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
        )
    };
    fs::write(path, contents)?;
    Ok(())
}

fn render_rclone_external_ssh_command(config: &AppConfig, host: &str) -> String {
    let remote = format!("{}@{host}", config.remote.ssh_user);
    [
        "/usr/bin/ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "ConnectTimeout=5".to_string(),
        "-o".to_string(),
        "ControlMaster=auto".to_string(),
        "-o".to_string(),
        "ControlPersist=600".to_string(),
        "-o".to_string(),
        "ControlPath=/tmp/syncsteward-%r@%h-%p".to_string(),
        "-o".to_string(),
        "StreamLocalBindUnlink=yes".to_string(),
        "-i".to_string(),
        config.ssh_key_path.display().to_string(),
        "-p".to_string(),
        "22".to_string(),
        remote,
    ]
    .into_iter()
    .map(quote_rclone_space_separated_arg)
    .collect::<Vec<_>>()
    .join(" ")
}

fn quote_rclone_space_separated_arg(value: String) -> String {
    if value.contains([' ', '\t', '"']) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value
    }
}

fn rclone_attempt_hosts(config: &AppConfig, primary_host: &str, attempts: usize) -> Vec<String> {
    let attempts = attempts.max(1);
    let mut ordered_hosts = vec![primary_host.to_string()];
    for host in &config.remote.preferred_hosts {
        if host != primary_host && !ordered_hosts.iter().any(|existing| existing == host) {
            ordered_hosts.push(host.clone());
        }
    }
    if ordered_hosts.is_empty() {
        ordered_hosts.push(primary_host.to_string());
    }

    (0..attempts)
        .map(|index| ordered_hosts[index % ordered_hosts.len()].clone())
        .collect()
}

fn run_rclone_command_with_host_retry<F>(
    config: &AppConfig,
    primary_host: &str,
    temp_dir: &Path,
    max_attempts: usize,
    retry_delay: Duration,
    mut build_command: F,
) -> Result<CommandOutput>
where
    F: FnMut(&Path) -> Command,
{
    let attempt_hosts = rclone_attempt_hosts(config, primary_host, max_attempts.max(1));
    let mut retry_notes = Vec::new();
    let mut last_output = None;

    for (index, host) in attempt_hosts.iter().enumerate() {
        let attempt_number = index + 1;
        let rclone_config_path = temp_dir.join(format!("rclone-attempt-{attempt_number}.conf"));
        write_target_rclone_config_at_path(config, host, &rclone_config_path)?;

        let mut output = run_spawned_command(build_command(&rclone_config_path));
        output.attempts = attempt_number;
        if output.success {
            if !retry_notes.is_empty() {
                let notes = retry_notes.join("\n");
                output.stderr = if output.stderr.trim().is_empty() {
                    notes
                } else {
                    format!("{notes}\n{}", output.stderr.trim())
                };
            }
            return Ok(output);
        }

        let retryable = command_failure_is_retryable(&output);
        retry_notes.push(format!(
            "attempt {attempt_number} [{host}]: {}",
            output.trim_or(&output.stdout)
        ));
        last_output = Some(output);
        if retryable && attempt_number < attempt_hosts.len() {
            std::thread::sleep(retry_delay);
        } else {
            break;
        }
    }

    let mut output = last_output.unwrap_or(CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: "command failed without output".to_string(),
        attempts: 1,
    });
    if output.attempts == 0 {
        output.attempts = 1;
    }
    if retry_notes.len() > 1 {
        let notes = retry_notes.join("\n");
        output.stderr = if output.stderr.trim().is_empty() {
            notes
        } else {
            format!("{notes}\n{}", output.stderr.trim())
        };
    }
    Ok(output)
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
    let preflight = evaluate_preflight(
        collect_status(config, config_source.to_string()),
        PreflightMode::Strict,
    );
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
    let launch_agent =
        probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
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
    let launch_agent =
        probe_launch_agent(&config.launch_agent_label, Some(&config.launch_agent_path));
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

    if matches!(
        config.remote.onedrive_service_scope,
        crate::config::RemoteServiceScope::User
    ) {
        let stop = run_remote_systemctl(config, host, "stop");
        if stop.success {
            return applied_step(
                "pause_remote_onedrive",
                format!("paused {} on {}", config.remote.onedrive_service, host),
                stop.trim_or(&stop.stdout).to_string(),
            );
        }
        let terminate = terminate_remote_onedrive_processes(config, host);
        if terminate.success {
            return applied_step(
                "pause_remote_onedrive",
                format!(
                    "paused {} on {} after user-service fallback",
                    config.remote.onedrive_service, host
                ),
                terminate.trim_or(&terminate.stdout).to_string(),
            );
        }
        return failed_step(
            "pause_remote_onedrive",
            format!(
                "failed to pause {} on {}",
                config.remote.onedrive_service, host
            ),
            format!(
                "user service stop: {}; process fallback: {}",
                stop.trim_or(&stop.stdout),
                terminate.trim_or(&terminate.stdout)
            ),
        );
    }

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
    if matches!(
        config.remote.onedrive_service_scope,
        crate::config::RemoteServiceScope::User
    ) {
        let start = start_remote_onedrive_user_service(config, host);
        if start.success {
            return applied_step(
                "resume_remote_onedrive",
                format!("resumed {} on {}", config.remote.onedrive_service, host),
                start.trim_or(&start.stdout).to_string(),
            );
        }
        return failed_step(
            "resume_remote_onedrive",
            format!(
                "failed to resume {} on {}",
                config.remote.onedrive_service, host
            ),
            start.trim_or(&start.stdout).to_string(),
        );
    }

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
    let state_key = target_state_key(&report.evaluation.target);
    let existing_state = load_state(&config.state_path).ok();
    let existing_run = existing_state
        .as_ref()
        .and_then(|state| state.target_runs.get(&state_key))
        .or_else(|| {
            existing_state
                .as_ref()
                .and_then(|state| state.target_runs.get(&report.evaluation.target.name))
        });
    let consecutive_failure_count = match report.outcome {
        ActionOutcome::Success | ActionOutcome::NoOp => 0,
        ActionOutcome::Failed => existing_run
            .map(|run| run.consecutive_failure_count.saturating_add(1))
            .unwrap_or(1),
        ActionOutcome::Blocked => existing_run
            .map(|run| run.consecutive_failure_count)
            .unwrap_or(0),
    };

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
        last_verified_at_unix_ms: report.verified_at_unix_ms,
        last_full_verified_at_unix_ms: if report.outcome == ActionOutcome::Success
            && report.verification_mode == Some(VerificationMode::Full)
        {
            report.verified_at_unix_ms
        } else {
            effective_last_full_verified_at(existing_run)
        },
        last_verification_mode: report.verification_mode,
        last_failure_class: match report.outcome {
            ActionOutcome::Success | ActionOutcome::NoOp => None,
            ActionOutcome::Failed => report.failure_class,
            ActionOutcome::Blocked => existing_run.and_then(|run| run.last_failure_class),
        },
        last_repair_at_unix_ms: existing_run.and_then(|run| run.last_repair_at_unix_ms),
        last_rebaseline_at_unix_ms: existing_run.and_then(|run| run.last_rebaseline_at_unix_ms),
        consecutive_failure_count,
        summary: report.summary.clone(),
    };

    if let Err(error) = save_target_run(&config.state_path, &state_key, state) {
        eprintln!("syncsteward: failed to record target run state: {error}");
    }
}

fn record_target_verification(config: &AppConfig, report: &TargetVerifyReport) {
    if let Err(error) = append_target_verify_audit(&config.audit_log_path, report) {
        eprintln!("syncsteward: failed to append target verification audit: {error}");
    }
    let state_key = target_state_key(&report.evaluation.target);
    let existing_state = load_state(&config.state_path).ok();
    let existing_run = existing_state
        .as_ref()
        .and_then(|state| state.target_runs.get(&state_key))
        .or_else(|| {
            existing_state
                .as_ref()
                .and_then(|state| state.target_runs.get(&report.evaluation.target.name))
        });

    let finished_at_unix_ms = report.verified_at_unix_ms.unwrap_or_else(now_unix_ms);
    let consecutive_failure_count = match report.outcome {
        ActionOutcome::Success | ActionOutcome::NoOp => 0,
        ActionOutcome::Failed => existing_run
            .map(|run| run.consecutive_failure_count.saturating_add(1))
            .unwrap_or(1),
        ActionOutcome::Blocked => existing_run
            .map(|run| run.consecutive_failure_count)
            .unwrap_or(0),
    };

    let state = TargetRunState {
        target_name: report.evaluation.target.name.clone(),
        target_id: report.evaluation.target.target_id.clone(),
        local_path: report.evaluation.target.local_path.clone(),
        effective_mode: report.evaluation.effective_mode,
        outcome: report.outcome,
        dry_run: false,
        finished_at_unix_ms,
        last_success_at_unix_ms: if report.outcome == ActionOutcome::Success {
            existing_run
                .and_then(|run| run.last_success_at_unix_ms)
                .or(Some(finished_at_unix_ms))
        } else {
            None
        },
        last_verified_at_unix_ms: report.verified_at_unix_ms,
        last_full_verified_at_unix_ms: if report.outcome == ActionOutcome::Success
            && report.verification_mode == VerificationMode::Full
        {
            report.verified_at_unix_ms
        } else {
            effective_last_full_verified_at(existing_run)
        },
        last_verification_mode: Some(report.verification_mode),
        last_failure_class: match report.outcome {
            ActionOutcome::Success | ActionOutcome::NoOp => None,
            ActionOutcome::Failed => report.failure_class,
            ActionOutcome::Blocked => existing_run.and_then(|run| run.last_failure_class),
        },
        last_repair_at_unix_ms: existing_run.and_then(|run| run.last_repair_at_unix_ms),
        last_rebaseline_at_unix_ms: existing_run.and_then(|run| run.last_rebaseline_at_unix_ms),
        consecutive_failure_count,
        summary: report.summary.clone(),
    };

    if let Err(error) = save_target_run(&config.state_path, &state_key, state) {
        eprintln!("syncsteward: failed to record target verification state: {error}");
    }
}

fn record_target_recovery(config: &AppConfig, report: &TargetRecoveryReport) {
    if let Err(error) = append_target_recovery_audit(&config.audit_log_path, report) {
        eprintln!("syncsteward: failed to append target recovery audit: {error}");
    }

    if report.dry_run || report.outcome != ActionOutcome::Success {
        return;
    }

    let Some(run) = &report.run else {
        return;
    };

    let state_key = target_state_key(&run.evaluation.target);
    let Ok(existing_state) = load_state(&config.state_path) else {
        return;
    };
    let Some(existing_run) = existing_state
        .target_runs
        .get(&state_key)
        .or_else(|| existing_state.target_runs.get(&run.evaluation.target.name))
        .cloned()
    else {
        return;
    };

    let recovered_at_unix_ms = report
        .verification
        .as_ref()
        .and_then(|verify| verify.verified_at_unix_ms)
        .unwrap_or_else(now_unix_ms);

    let mut updated = existing_run;
    match report.action {
        RecoveryAction::Repair => updated.last_repair_at_unix_ms = Some(recovered_at_unix_ms),
        RecoveryAction::Rebaseline => {
            updated.last_rebaseline_at_unix_ms = Some(recovered_at_unix_ms)
        }
    }

    if let Err(error) = save_target_run(&config.state_path, &state_key, updated) {
        eprintln!("syncsteward: failed to record target recovery state: {error}");
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

    if let Err(error) = save_runner_cycle(
        &config.state_path,
        state,
        last_live_cycle_finished_at_unix_ms,
    ) {
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
        verification_mode: Option<VerificationMode>,
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
        verification_mode: report.verification_mode,
        effective_mode: report.evaluation.effective_mode,
        blocker_ids,
        step_ids,
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn append_target_verify_audit(path: &Path, report: &TargetVerifyReport) -> Result<()> {
    #[derive(Serialize)]
    struct TargetVerifyAuditRecord<'a> {
        timestamp_unix_ms: u128,
        kind: &'static str,
        selector: &'a str,
        target_name: &'a str,
        outcome: ActionOutcome,
        summary: &'a str,
        verified_at_unix_ms: Option<u128>,
        verification_mode: VerificationMode,
        failure_class: Option<FailureClass>,
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

    let record = TargetVerifyAuditRecord {
        timestamp_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        kind: "target_verify",
        selector: &report.selector,
        target_name: &report.evaluation.target.name,
        outcome: report.outcome,
        summary: &report.summary,
        verified_at_unix_ms: report.verified_at_unix_ms,
        verification_mode: report.verification_mode,
        failure_class: report.failure_class,
        effective_mode: report.evaluation.effective_mode,
        blocker_ids,
        step_ids,
    };

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&record)?)?;
    Ok(())
}

fn append_target_recovery_audit(path: &Path, report: &TargetRecoveryReport) -> Result<()> {
    #[derive(Serialize)]
    struct TargetRecoveryAuditRecord<'a> {
        timestamp_unix_ms: u128,
        kind: &'static str,
        selector: &'a str,
        target_name: &'a str,
        action: RecoveryAction,
        dry_run: bool,
        confirmation_required: bool,
        confirmed: bool,
        outcome: ActionOutcome,
        summary: &'a str,
        recovery_step_ids: Vec<&'a str>,
        run_outcome: Option<ActionOutcome>,
        verification_outcome: Option<ActionOutcome>,
        verified_at_unix_ms: Option<u128>,
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let record = TargetRecoveryAuditRecord {
        timestamp_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        kind: "target_recovery",
        selector: &report.selector,
        target_name: &report.target_name,
        action: report.action,
        dry_run: report.dry_run,
        confirmation_required: report.confirmation_required,
        confirmed: report.confirmed,
        outcome: report.outcome,
        summary: &report.summary,
        recovery_step_ids: report
            .recovery_steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<Vec<_>>(),
        run_outcome: report.run.as_ref().map(|run| run.outcome),
        verification_outcome: report.verification.as_ref().map(|verify| verify.outcome),
        verified_at_unix_ms: report
            .verification
            .as_ref()
            .and_then(|verify| verify.verified_at_unix_ms),
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
    let home_dir = crate::config::current_home_dir();
    let login_name = runner_login_name(&home_dir);
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
            "  <key>EnvironmentVariables</key>\n",
            "  <dict>\n",
            "    <key>PATH</key>\n",
            "    <string>{path_env}</string>\n",
            "    <key>HOME</key>\n",
            "    <string>{home_dir}</string>\n",
            "    <key>USER</key>\n",
            "    <string>{login_name}</string>\n",
            "    <key>LOGNAME</key>\n",
            "    <string>{login_name}</string>\n",
            "  </dict>\n",
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
        path_env = plist_xml_escape(&runner_path_environment()),
        home_dir = plist_xml_escape(&home_dir.to_string_lossy()),
        login_name = plist_xml_escape(&login_name),
        run_at_load = if agent.run_at_load { "true" } else { "false" },
        interval_seconds = interval_seconds,
        stdout_path = plist_xml_escape(&agent.stdout_path.to_string_lossy()),
        stderr_path = plist_xml_escape(&agent.stderr_path.to_string_lossy()),
    )
}

fn runner_login_name(home_dir: &Path) -> String {
    std::env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("LOGNAME")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            home_dir
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "syncsteward".to_string())
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
            service_name: config.remote.onedrive_service.clone(),
            service_scope: config.remote.onedrive_service_scope,
            coordination_enabled: config.coordination.pause_remote_on_sync,
            service_state,
            detail,
        };
    }

    RemoteStatus {
        selected_host: None,
        reachable: false,
        service_name: config.remote.onedrive_service.clone(),
        service_scope: config.remote.onedrive_service_scope,
        coordination_enabled: config.coordination.pause_remote_on_sync,
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

    if matches!(
        config.remote.onedrive_service_scope,
        crate::config::RemoteServiceScope::User
    ) {
        return primary;
    }

    let (sudo_password, secret_detail) = resolve_remote_sudo_password(config);
    if let Some(password) = sudo_password {
        let secret_output = run_remote_systemctl_with_password(config, host, action, &password);
        if secret_output.success {
            return secret_output;
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
            let mut parts = vec![format!("sudo path: {}", primary.trim_or(&primary.stdout))];
            parts.push(format!(
                "secret path: {}",
                secret_output.trim_or(&secret_output.stdout)
            ));
            parts.push(format!(
                "direct path: {}",
                fallback.trim_or(&fallback.stdout)
            ));
            if let Some(detail) = secret_detail {
                parts.push(format!("secret source: {detail}"));
            }
            CommandOutput {
                success: false,
                stdout: String::new(),
                stderr: parts.join("; "),
                attempts: 1,
            }
        }
    } else {
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
            let mut parts = vec![format!("sudo path: {}", primary.trim_or(&primary.stdout))];
            if let Some(detail) = secret_detail {
                parts.push(format!("secret source: {detail}"));
            }
            parts.push(format!(
                "direct path: {}",
                fallback.trim_or(&fallback.stdout)
            ));
            CommandOutput {
                success: false,
                stdout: String::new(),
                stderr: parts.join("; "),
                attempts: 1,
            }
        }
    }
}

fn run_remote_systemctl_raw(config: &AppConfig, host: &str, action: &str) -> CommandOutput {
    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let command = match config.remote.onedrive_service_scope {
        crate::config::RemoteServiceScope::System => {
            format!(
                "sudo -n systemctl {} {}",
                action, config.remote.onedrive_service
            )
        }
        crate::config::RemoteServiceScope::User => {
            format!(
                "systemctl --user {} {}",
                action, config.remote.onedrive_service
            )
        }
    };
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
    if matches!(
        config.remote.onedrive_service_scope,
        crate::config::RemoteServiceScope::User
    ) {
        return primary;
    }
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

fn run_remote_command(config: &AppConfig, host: &str, command: &str) -> CommandOutput {
    let remote = format!("{}@{}", config.remote.ssh_user, host);
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
            command,
        ],
    )
}

fn terminate_remote_onedrive_processes(config: &AppConfig, host: &str) -> CommandOutput {
    let terminate = run_remote_command(config, host, "pkill -TERM -x onedrive || true");
    for _ in 0..10 {
        let processes =
            run_remote_command(config, host, "pgrep -a -u \"$USER\" -x onedrive || true");
        if processes.stdout.trim().is_empty() {
            return CommandOutput {
                success: true,
                stdout: if terminate.stdout.trim().is_empty() && terminate.stderr.trim().is_empty()
                {
                    "terminated onedrive processes".to_string()
                } else {
                    terminate.trim_or(&terminate.stdout).to_string()
                },
                stderr: String::new(),
                attempts: 1,
            };
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    let processes = run_remote_command(config, host, "pgrep -a -u \"$USER\" -x onedrive || true");
    CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: if processes.stdout.trim().is_empty() {
            "onedrive processes did not exit before timeout".to_string()
        } else {
            format!(
                "onedrive processes were still running after timeout: {}",
                processes.stdout.trim()
            )
        },
        attempts: 1,
    }
}

fn start_remote_onedrive_user_service(config: &AppConfig, host: &str) -> CommandOutput {
    let start = run_remote_command(
        config,
        host,
        &format!(
            "systemctl --user reset-failed {} >/dev/null 2>&1 || true; systemctl --user start --no-block {}",
            config.remote.onedrive_service, config.remote.onedrive_service
        ),
    );
    if !start.success {
        return start;
    }

    for _ in 0..30 {
        let processes =
            run_remote_command(config, host, "pgrep -a -u \"$USER\" -x onedrive || true");
        if !processes.stdout.trim().is_empty() {
            return CommandOutput {
                success: true,
                stdout: if start.stdout.trim().is_empty() && start.stderr.trim().is_empty() {
                    "started onedrive user service".to_string()
                } else {
                    start.trim_or(&start.stdout).to_string()
                },
                stderr: String::new(),
                attempts: 1,
            };
        }
        std::thread::sleep(Duration::from_secs(1));
    }

    CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: "onedrive user service did not report a running process before timeout".to_string(),
        attempts: 1,
    }
}

fn resolve_remote_sudo_password(config: &AppConfig) -> (Option<String>, Option<String>) {
    if let Some(service) = &config.remote.sudo_password_keychain_service {
        if let Some(secret) = read_keychain_secret(
            service,
            config.remote.sudo_password_keychain_account.as_deref(),
        ) {
            return (
                Some(secret),
                Some(format!(
                    "Keychain service {}{}",
                    service,
                    config
                        .remote
                        .sudo_password_keychain_account
                        .as_deref()
                        .map(|account| format!(" account {}", account))
                        .unwrap_or_default()
                )),
            );
        }
    }

    if let Some(op_reference) = &config.remote.sudo_password_op_reference {
        if let Some(secret) = read_1password_secret(op_reference) {
            return (
                Some(secret),
                Some(format!("1Password reference {}", op_reference)),
            );
        }
    }

    let detail = if let Some(service) = &config.remote.sudo_password_keychain_service {
        Some(format!(
            "Keychain service {}{} was unavailable",
            service,
            config
                .remote
                .sudo_password_keychain_account
                .as_deref()
                .map(|account| format!(" account {}", account))
                .unwrap_or_default()
        ))
    } else if let Some(op_reference) = &config.remote.sudo_password_op_reference {
        Some(format!(
            "1Password reference {} was unavailable",
            op_reference
        ))
    } else {
        None
    };

    (None, detail)
}

fn read_1password_secret(op_reference: &str) -> Option<String> {
    let shell_command = format!(
        "op signin --account my.1password.com >/dev/null 2>&1; op read {} 2>/dev/null",
        shell_single_quote(op_reference)
    );
    let output = run_command("zsh", ["-lc", shell_command.as_str()]);
    if output.success {
        let value = output.stdout.trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    } else {
        None
    }
}

fn read_keychain_secret(service: &str, account: Option<&str>) -> Option<String> {
    let mut command = Command::new(resolve_program_path("security"));
    command
        .arg("find-generic-password")
        .arg("-w")
        .arg("-s")
        .arg(service);
    if let Some(account) = account {
        command.arg("-a").arg(account);
    }
    let output = run_spawned_command(command);
    if output.success {
        let value = output.stdout.trim().to_string();
        if value.is_empty() { None } else { Some(value) }
    } else {
        None
    }
}

fn run_remote_systemctl_with_password(
    config: &AppConfig,
    host: &str,
    action: &str,
    password: &str,
) -> CommandOutput {
    let remote = format!("{}@{}", config.remote.ssh_user, host);
    let remote_command = format!(
        "sudo -S -p '' systemctl {} {}",
        action, config.remote.onedrive_service
    );
    let mut command = Command::new(resolve_program_path("ssh"));
    command
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg("-i")
        .arg(&config.ssh_key_path)
        .arg(remote)
        .arg(remote_command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match command.spawn() {
        Ok(mut child) => {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = stdin.write_all(password.as_bytes());
                let _ = stdin.write_all(b"\n");
            }
            match child.wait_with_output() {
                Ok(output) => CommandOutput {
                    success: output.status.success(),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    attempts: 1,
                },
                Err(error) => CommandOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: error.to_string(),
                    attempts: 1,
                },
            }
        }
        Err(error) => CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
            attempts: 1,
        },
    }
}

fn materialize_remote_instruction_links(config: &AppConfig, host: &str) -> ActionStep {
    if !config.coordination.materialize_instruction_links {
        return skipped_step(
            "materialize_remote_instruction_links",
            "instruction-link materialization is disabled".to_string(),
            "remote instruction-file symlink normalization is not enabled in config".to_string(),
        );
    }

    let root = remote_path_for_shell(&config.remote_sync_root, &config.remote.ssh_user);
    let names = config
        .coordination
        .instruction_file_names
        .iter()
        .map(|name| shell_single_quote(name))
        .collect::<Vec<_>>()
        .join(" ");

    let command = format!(
        r#"bash -lc '
set -eu
root={}

materialize_link() {{
  link="$1"
  dir=$(dirname "$link")
  if [ ! -e "$dir/AGENTS.md" ]; then
    search_dir="$dir"
    while [ "$search_dir" != "/" ]; do
      candidate="$search_dir/AGENTS.md"
      if [ -f "$candidate" ]; then
        cp "$candidate" "$dir/AGENTS.md"
        break
      fi
      search_dir=$(dirname "$search_dir")
    done
  fi
  if [ -L "$link" ]; then
    tmp="$link.syncsteward.$$"
    if cp -L "$link" "$tmp"; then
      mv -f "$tmp" "$link"
      printf "materialized %s\n" "$link"
    else
      printf "could not materialize %s\n" "$link"
    fi
  fi
}}

for name in {}; do
  while IFS= read -r -d "" link; do
    materialize_link "$link"
  done < <(find "$root" -type l -name "$name" -print0)
done
'"#,
        shell_single_quote(&root),
        names
    );
    let output = run_remote_command(config, host, &command);
    if output.success {
        applied_step(
            "materialize_remote_instruction_links",
            format!(
                "materialized instruction links under {}",
                config.remote_sync_root
            ),
            output.trim_or(&output.stdout).to_string(),
        )
    } else {
        failed_step(
            "materialize_remote_instruction_links",
            format!(
                "failed to normalize instruction links under {} on {}",
                config.remote_sync_root, host
            ),
            output.trim_or(&output.stdout).to_string(),
        )
    }
}

fn remote_target_root_for_shell(config: &AppConfig, target_remote_path: &str) -> String {
    let root = remote_path_for_shell(&config.remote_sync_root, &config.remote.ssh_user);
    let trimmed = target_remote_path.trim_matches('/');
    if trimmed.is_empty() {
        return root;
    }

    let root_name = Path::new(&root)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if !root_name.is_empty() {
        if trimmed == root_name {
            return root;
        }
        if let Some(suffix) = trimmed.strip_prefix(&format!("{root_name}/")) {
            return join_remote_shell_path(&root, suffix);
        }
    }

    join_remote_shell_path(&root, trimmed)
}

fn join_remote_shell_path(base: &str, relative: &str) -> String {
    if relative.starts_with('/') {
        return relative.to_string();
    }
    let base = base.trim_end_matches('/');
    let relative = relative.trim_matches('/');
    if relative.is_empty() {
        base.to_string()
    } else if base.is_empty() {
        format!("/{relative}")
    } else {
        format!("{base}/{relative}")
    }
}

fn remote_path_for_shell(path: &str, remote_user: &str) -> String {
    let trimmed = path.trim();
    if let Some(suffix) = trimmed.strip_prefix("~/") {
        return format!("/home/{}/{}", remote_user, suffix);
    }
    if trimmed == "~" {
        return format!("/home/{}", remote_user);
    }
    trimmed.to_string()
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn scan_artifacts(config: &AppConfig, candidate_roots: &[PathBuf]) -> ArtifactReport {
    let mut roots_scanned = Vec::new();
    let mut conflict_examples = Vec::new();
    let mut safe_backup_examples = Vec::new();
    let mut conflict_count = 0usize;
    let mut safe_backup_count = 0usize;

    for root in candidate_roots {
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

#[derive(Debug, Clone)]
struct ArtifactMatch {
    kind: ArtifactKind,
    root: PathBuf,
    path: PathBuf,
}

fn collect_artifact_matches(candidate_roots: &[PathBuf]) -> Vec<ArtifactMatch> {
    let mut matches = Vec::new();

    for root in candidate_roots {
        if !root.exists() {
            continue;
        }

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
                matches.push(ArtifactMatch {
                    kind: ArtifactKind::Conflict,
                    root: root.clone(),
                    path: entry.path().to_path_buf(),
                });
            }
            if name.contains("victorystore-safeBackup") {
                matches.push(ArtifactMatch {
                    kind: ArtifactKind::SafeBackup,
                    root: root.clone(),
                    path: entry.path().to_path_buf(),
                });
            }
        }
    }

    matches
}

fn artifact_quarantine_root(config: &AppConfig) -> PathBuf {
    let base = config
        .state_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("artifact-quarantine")
        .join(now_unix_ms().to_string())
}

fn quarantine_scope_path(root: &Path) -> PathBuf {
    let home = crate::config::current_home_dir();
    if let Ok(relative) = root.strip_prefix(&home) {
        return relative.to_path_buf();
    }

    let sanitized = root
        .to_string_lossy()
        .replace(':', "")
        .replace('/', "__")
        .replace('\\', "__");
    PathBuf::from(sanitized)
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

fn contains_blocking_steps(steps: &[ActionStep]) -> bool {
    steps
        .iter()
        .any(|step| step.status == ActionStepStatus::Blocked)
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
        ActionOutcome::Success => {
            if !dry_run && !verification_steps_from_steps(steps).is_empty() {
                format!("{mode} and verification succeeded for {target_name}")
            } else {
                format!("{mode} succeeded for {target_name}")
            }
        }
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

fn summarize_target_verify(
    target_name: &str,
    outcome: ActionOutcome,
    steps: &[ActionStep],
) -> String {
    match outcome {
        ActionOutcome::Success => format!("verification succeeded for {target_name}"),
        ActionOutcome::NoOp => format!("verification made no changes for {target_name}"),
        ActionOutcome::Blocked => format!("verification blocked for {target_name}"),
        ActionOutcome::Failed => {
            let failed = steps
                .iter()
                .filter(|step| step.status == ActionStepStatus::Failed)
                .count();
            format!("verification failed for {target_name} ({failed} failed steps)")
        }
    }
}

fn summarize_target_recovery(
    action: RecoveryAction,
    outcome: ActionOutcome,
    run: &TargetRunReport,
    verification: Option<&TargetVerifyReport>,
) -> String {
    let action_name = describe_recovery_action(action);
    match outcome {
        ActionOutcome::Success => {
            if run.outcome == ActionOutcome::Failed {
                format!(
                    "{action_name} recovered and verified {}",
                    run.evaluation.target.name
                )
            } else if verification.is_some_and(|report| report.outcome == ActionOutcome::Success) {
                format!(
                    "{action_name} succeeded and verified {}",
                    run.evaluation.target.name
                )
            } else if run.dry_run {
                format!(
                    "{action_name} dry run succeeded for {}",
                    run.evaluation.target.name
                )
            } else {
                format!("{action_name} succeeded for {}", run.evaluation.target.name)
            }
        }
        ActionOutcome::NoOp => format!(
            "{action_name} made no changes for {}",
            run.evaluation.target.name
        ),
        ActionOutcome::Blocked => {
            format!("{action_name} blocked for {}", run.evaluation.target.name)
        }
        ActionOutcome::Failed => {
            if verification.is_some_and(|report| report.outcome == ActionOutcome::Success) {
                format!(
                    "{action_name} recovered verification for {} but the recovery flow still failed",
                    run.evaluation.target.name
                )
            } else {
                format!("{action_name} failed for {}", run.evaluation.target.name)
            }
        }
    }
}

fn describe_recovery_action(action: RecoveryAction) -> &'static str {
    match action {
        RecoveryAction::Repair => "repair",
        RecoveryAction::Rebaseline => "rebaseline",
    }
}

fn verification_steps_from_steps(steps: &[ActionStep]) -> Vec<ActionStep> {
    steps
        .iter()
        .filter(|step| is_verification_step(step))
        .cloned()
        .collect()
}

fn verification_report_from_run(report: &TargetRunReport) -> Option<TargetVerifyReport> {
    let steps = verification_steps_from_steps(&report.steps);
    if steps.is_empty() {
        return None;
    }

    let outcome = summarize_run_outcome(&steps);
    Some(TargetVerifyReport {
        config_source: report.config_source.clone(),
        selector: report.selector.clone(),
        outcome,
        summary: summarize_target_verify(&report.evaluation.target.name, outcome, &steps),
        preflight_ready: report.preflight_ready,
        evaluation: report.evaluation.clone(),
        verified_at_unix_ms: if outcome == ActionOutcome::Success {
            report.verified_at_unix_ms
        } else {
            None
        },
        verification_mode: report.verification_mode.unwrap_or(VerificationMode::Full),
        failure_class: classify_failure_from_steps(outcome, &steps),
        steps,
    })
}

fn is_verification_step(step: &ActionStep) -> bool {
    step.id.starts_with("write_verify_")
        || step.id.starts_with("prepare_verify_")
        || step.id == "select_verification_mode"
        || step.id == "rclone_check"
        || step.id == "rclone_check_size"
        || step.id == "rclone_check_non_db"
        || step.id == "rclone_check_non_db_size"
        || step.id == "rclone_check_snapshots"
        || step.id == "rclone_check_snapshots_size"
        || step.id == "sqlite_snapshot_verify_backup"
        || step.id == "sqlite_snapshot_verify_list"
        || step.id == "verify_sampled_hashes"
        || step.id == "verify_sampled_hashes_non_db"
        || step.id == "verify_sampled_hashes_snapshots"
        || step.id == "verify_target"
}

fn classify_failure_from_steps(
    outcome: ActionOutcome,
    steps: &[ActionStep],
) -> Option<FailureClass> {
    if outcome != ActionOutcome::Failed {
        return None;
    }

    let failed_steps = steps
        .iter()
        .filter(|step| step.status == ActionStepStatus::Failed)
        .collect::<Vec<_>>();
    if failed_steps.is_empty() {
        return Some(FailureClass::Unknown);
    }

    let combined = failed_steps
        .iter()
        .flat_map(|step| [step.summary.as_str(), step.detail.as_str()])
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();

    if combined.contains("permission denied")
        || combined.contains("access denied")
        || combined.contains("auth")
        || combined.contains("publickey")
        || combined.contains("unauthorized")
    {
        return Some(FailureClass::Auth);
    }

    if combined.contains("connection refused")
        || combined.contains("timed out")
        || combined.contains("timeout")
        || combined.contains("no route to host")
        || combined.contains("network is unreachable")
        || combined.contains("connection reset")
        || combined.contains("dial tcp")
    {
        return Some(FailureClass::Transport);
    }

    if failed_steps.iter().any(|step| {
        matches!(
            step.id.as_str(),
            "sqlite_snapshot_backup"
                | "sqlite_snapshot_upload"
                | "sqlite_snapshot_verify_backup"
                | "rclone_check_snapshots"
        )
    }) || combined.contains("sqlite")
    {
        return Some(FailureClass::Snapshot);
    }

    if combined.contains("missing on")
        || combined.contains("files differ")
        || combined.contains("sizes differ")
        || combined.contains("hash differ")
        || combined.contains("checks differ")
        || combined.contains("differ")
        || combined.contains("corrupt")
    {
        return Some(FailureClass::Divergence);
    }

    if combined.contains("no such file or directory")
        || combined.contains("not found")
        || combined.contains("missing")
    {
        return Some(FailureClass::Path);
    }

    Some(FailureClass::Unknown)
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

fn alert_signature(alerts: &[AlertRecord]) -> String {
    let mut parts = alerts
        .iter()
        .map(|alert| {
            format!(
                "{}|{:?}|{}|{}|{}",
                alert.id,
                alert.severity,
                alert.target_name.as_deref().unwrap_or(""),
                alert.summary,
                alert.detail
            )
        })
        .collect::<Vec<_>>();
    parts.sort();
    parts.join("\u{1f}")
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
    let mut summary = if source.is_empty() {
        "command completed without output".to_string()
    } else {
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
    };
    if output.attempts > 1 {
        let prefix = if output.success {
            format!("command succeeded after {} attempts", output.attempts)
        } else {
            format!("command failed after {} attempts", output.attempts)
        };
        summary = format!("{prefix}\n{summary}");
    }
    summary
}

fn command_failure_is_retryable(output: &CommandOutput) -> bool {
    let combined = format!("{}\n{}", output.stdout, output.stderr).to_lowercase();

    if combined.contains("files differ")
        || combined.contains("sizes differ")
        || combined.contains("hash differ")
        || combined.contains("checks differ")
        || combined.contains("missing on")
        || combined.contains("corrupt")
    {
        return false;
    }

    if combined.contains("permission denied")
        || combined.contains("access denied")
        || combined.contains("publickey")
        || combined.contains("unauthorized")
        || combined.contains("authentication failed")
        || combined.contains("no such file or directory")
        || combined.contains("not found")
        || combined.contains("directory not found")
    {
        return false;
    }

    combined.contains("connection refused")
        || combined.contains("timed out")
        || combined.contains("timeout")
        || combined.contains("no route to host")
        || combined.contains("network is unreachable")
        || combined.contains("connection reset")
        || combined.contains("dial tcp")
        || combined.contains("temporarily unavailable")
        || combined.contains("broken pipe")
        || combined.contains("server closed idle connection")
        || combined.contains("unexpected eof")
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

fn runner_path_environment() -> String {
    let mut segments = vec![
        "/usr/local/bin".to_string(),
        "/opt/homebrew/bin".to_string(),
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];

    if let Some(path) = std::env::var_os("PATH") {
        for segment in std::env::split_paths(&path) {
            let value = crate::config::expand_path(&segment)
                .to_string_lossy()
                .to_string();
            if !value.is_empty() && !segments.iter().any(|existing| existing == &value) {
                segments.push(value);
            }
        }
    }

    segments.join(":")
}

fn resolve_program_path(program: &str) -> PathBuf {
    let candidate = PathBuf::from(program);
    if candidate.components().count() > 1 {
        return candidate;
    }

    let env_paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();

    let mut search_paths = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];
    for path in env_paths {
        let expanded = crate::config::expand_path(&path);
        if !search_paths.iter().any(|existing| existing == &expanded) {
            search_paths.push(expanded);
        }
    }

    for directory in search_paths {
        let resolved = directory.join(program);
        if resolved.exists() {
            return resolved;
        }
    }

    candidate
}

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
    attempts: usize,
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
    match Command::new(resolve_program_path(program))
        .args(args)
        .output()
    {
        Ok(output) => CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            attempts: 1,
        },
        Err(error) => CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
            attempts: 1,
        },
    }
}

fn run_spawned_command(mut command: Command) -> CommandOutput {
    match command.output() {
        Ok(output) => CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            attempts: 1,
        },
        Err(error) => CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: error.to_string(),
            attempts: 1,
        },
    }
}

fn rclone_retry_attempts(dry_run: bool) -> usize {
    if dry_run { 1 } else { 3 }
}

fn run_spawned_command_with_retry<F>(
    mut make_command: F,
    max_attempts: usize,
    retry_delay: Duration,
) -> CommandOutput
where
    F: FnMut() -> Command,
{
    let attempts = max_attempts.max(1);
    let mut retry_notes = Vec::new();
    let mut last_output = None;

    for attempt in 1..=attempts {
        let mut output = run_spawned_command(make_command());
        output.attempts = attempt;
        if output.success {
            if !retry_notes.is_empty() {
                let note = retry_notes.join("\n");
                output.stderr = if output.stderr.trim().is_empty() {
                    note
                } else {
                    format!("{note}\n{}", output.stderr.trim())
                };
            }
            return output;
        }

        let retryable = command_failure_is_retryable(&output);
        retry_notes.push(format!(
            "attempt {attempt}: {}",
            output.trim_or(&output.stdout)
        ));
        last_output = Some(output);
        if retryable && attempt < attempts {
            std::thread::sleep(retry_delay);
        } else {
            break;
        }
    }

    let mut output = last_output.unwrap_or(CommandOutput {
        success: false,
        stdout: String::new(),
        stderr: "command failed without output".to_string(),
        attempts: 1,
    });
    if output.attempts == 0 {
        output.attempts = 1;
    }
    if retry_notes.len() > 1 {
        let notes = retry_notes.join("\n");
        output.stderr = if output.stderr.trim().is_empty() {
            notes
        } else {
            format!("{notes}\n{}", output.stderr.trim())
        };
    }
    output
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
        ActionOutcome, ActionStepStatus, ActiveTargetOperationGuard, PreflightMode,
        add_managed_target, alert_signature, analyze_log_contents,
        build_recent_target_run_summaries, collect_target_status, ensure_target_ids,
        evaluate_preflight, prune_state, record_target_run, relocate_managed_target,
        run_spawned_command_with_retry, runner_due_status, scheduled_notify_alerts,
        stale_runner_active_cycle_detail, summarize_command_output, summarize_outcome,
        target_state_key, update_config, write_target_rclone_config_at_path,
    };
    use crate::config::{
        AppConfig, ManagedTarget, PolicyConfig, PolicyMode, RcloneSshMode, VerificationMode,
        load_config,
    };
    use crate::model::{
        AcknowledgedLogSummary, ActionStep, AlertRecord, AlertSeverity, ArtifactReport,
        CheckStatus, ConfigPatch, FailureClass, LaunchAgentStatus, LegacySyncMode, PolicySummary,
        RemoteConfigPatch, RemoteStatus, RunnerConfigPatch, ServiceState, StatusReport,
        SyncTargetRecord, TargetEvaluation, TargetOperationKind, TargetRunReport,
    };
    use crate::state::{
        AlertNotificationState, AppState, RunnerActiveCycleState, TargetRunState, load_state,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;
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
        let report = evaluate_preflight(
            StatusReport {
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
                active_target_operation: None,
                remote: RemoteStatus {
                    selected_host: Some("127.0.0.1".to_string()),
                    reachable: true,
                    service_name: "onedrive.service".to_string(),
                    service_scope: crate::config::RemoteServiceScope::User,
                    coordination_enabled: true,
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
            },
            PreflightMode::ManagedRun,
        );

        assert!(report.ready);
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "latest_log_clean")
            .expect("latest_log_clean");
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[test]
    fn managed_run_preflight_warns_when_remote_service_is_active_and_coordinated() {
        let report = evaluate_preflight(
            StatusReport {
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
                active_target_operation: None,
                remote: RemoteStatus {
                    selected_host: Some("127.0.0.1".to_string()),
                    reachable: true,
                    service_name: "onedrive.service".to_string(),
                    service_scope: crate::config::RemoteServiceScope::User,
                    coordination_enabled: true,
                    service_state: ServiceState::Active,
                    detail: "onedrive.service returned active".to_string(),
                },
                artifacts: ArtifactReport {
                    roots_scanned: Vec::new(),
                    conflict_count: 0,
                    conflict_examples: Vec::new(),
                    safe_backup_count: 0,
                    safe_backup_examples: Vec::new(),
                },
                acknowledged_log: None,
                latest_log: None,
            },
            PreflightMode::ManagedRun,
        );

        assert!(report.ready);
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "remote_onedrive_paused")
            .expect("remote_onedrive_paused");
        assert_eq!(check.status, CheckStatus::Warn);
    }

    #[test]
    fn strict_preflight_warns_when_remote_service_is_failed() {
        let report = evaluate_preflight(
            StatusReport {
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
                active_target_operation: None,
                remote: RemoteStatus {
                    selected_host: Some("127.0.0.1".to_string()),
                    reachable: true,
                    service_name: "onedrive.service".to_string(),
                    service_scope: crate::config::RemoteServiceScope::User,
                    coordination_enabled: true,
                    service_state: ServiceState::Failed,
                    detail: "onedrive.service returned failed".to_string(),
                },
                artifacts: ArtifactReport {
                    roots_scanned: Vec::new(),
                    conflict_count: 0,
                    conflict_examples: Vec::new(),
                    safe_backup_count: 0,
                    safe_backup_examples: Vec::new(),
                },
                acknowledged_log: None,
                latest_log: None,
            },
            PreflightMode::Strict,
        );

        assert!(report.ready);
        let check = report
            .checks
            .iter()
            .find(|check| check.id == "remote_onedrive_paused")
            .expect("remote_onedrive_paused");
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
    fn snapshot_exclusion_patterns_only_cover_listed_sqlite_paths() {
        let snapshot = crate::config::TargetSnapshot {
            target: "Books".to_string(),
            sqlite_paths: vec![
                PathBuf::from("e-Books/librona.db"),
                PathBuf::from("e-Books/.docs/workspace.db"),
            ],
            rationale: None,
        };

        let patterns = super::snapshot_exclusion_patterns(&snapshot);

        assert!(patterns.contains(&"e-Books/librona.db".to_string()));
        assert!(patterns.contains(&"e-Books/librona.db-wal".to_string()));
        assert!(patterns.contains(&"e-Books/librona.db-shm".to_string()));
        assert!(patterns.contains(&"e-Books/librona.db-journal".to_string()));
        assert!(patterns.contains(&"e-Books/.docs/workspace.db".to_string()));
        assert_eq!(patterns.len(), 8);
        assert!(!patterns.contains(&"*.db".to_string()));
        assert!(!patterns.contains(&"*.sqlite".to_string()));
    }

    #[test]
    fn recent_target_runs_are_sorted_newest_first() {
        let mut state = AppState::default();
        state.target_runs.insert(
            "older".to_string(),
            TargetRunState {
                target_name: "Older".to_string(),
                target_id: Some("older".to_string()),
                local_path: PathBuf::from("/tmp/older"),
                effective_mode: PolicyMode::BackupOnly,
                outcome: ActionOutcome::Success,
                dry_run: false,
                finished_at_unix_ms: 100,
                last_success_at_unix_ms: Some(100),
                last_verified_at_unix_ms: None,
                last_full_verified_at_unix_ms: None,
                last_verification_mode: None,
                last_failure_class: None,
                last_repair_at_unix_ms: None,
                last_rebaseline_at_unix_ms: None,
                consecutive_failure_count: 0,
                summary: "older summary".to_string(),
            },
        );
        state.target_runs.insert(
            "newer".to_string(),
            TargetRunState {
                target_name: "Newer".to_string(),
                target_id: Some("newer".to_string()),
                local_path: PathBuf::from("/tmp/newer"),
                effective_mode: PolicyMode::BackupOnly,
                outcome: ActionOutcome::NoOp,
                dry_run: false,
                finished_at_unix_ms: 200,
                last_success_at_unix_ms: Some(150),
                last_verified_at_unix_ms: None,
                last_full_verified_at_unix_ms: None,
                last_verification_mode: None,
                last_failure_class: None,
                last_repair_at_unix_ms: None,
                last_rebaseline_at_unix_ms: None,
                consecutive_failure_count: 0,
                summary: "newer summary".to_string(),
            },
        );

        let runs = build_recent_target_run_summaries(&state);

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].target_name, "Newer");
        assert_eq!(runs[1].target_name, "Older");
    }

    #[test]
    fn runner_overview_exposes_active_cycle_state() {
        let config = AppConfig::default();
        let runner_agent = LaunchAgentStatus {
            label: "com.example.runner".to_string(),
            plist_path: Some(PathBuf::from("/tmp/com.example.runner.plist")),
            installed: true,
            loaded: true,
            running: true,
            detail: "running".to_string(),
        };
        let mut state = AppState::default();
        state.runner.active_cycle = Some(RunnerActiveCycleState {
            dry_run: false,
            started_at_unix_ms: 100,
            process_id: Some(42),
            current_target_selector: Some("Books".to_string()),
            current_target_name: Some("Books".to_string()),
            current_target_started_at_unix_ms: Some(125),
        });

        let overview = super::build_runner_overview(&config, &runner_agent, &state, 200);

        let active_cycle = overview.active_cycle.expect("active cycle");
        assert_eq!(active_cycle.started_at_unix_ms, 100);
        assert_eq!(
            active_cycle.current_target_selector.as_deref(),
            Some("Books")
        );
        assert_eq!(active_cycle.current_target_name.as_deref(), Some("Books"));
        assert_eq!(active_cycle.current_target_started_at_unix_ms, Some(125));
    }

    #[test]
    fn stale_runner_active_cycle_detects_missing_recorded_process() {
        let detail = stale_runner_active_cycle_detail(
            Some(&RunnerActiveCycleState {
                dry_run: false,
                started_at_unix_ms: 100,
                process_id: Some(999_999),
                current_target_selector: None,
                current_target_name: None,
                current_target_started_at_unix_ms: None,
            }),
            std::process::id(),
        );

        assert!(detail.is_some());
        assert!(
            detail
                .expect("stale cycle detail")
                .contains("recorded runner process 999999 is no longer active")
        );
    }

    #[test]
    fn stale_active_target_operation_detects_missing_recorded_process() {
        let detail = super::stale_active_target_operation_detail(
            Some(&crate::state::ActiveTargetOperationState {
                kind: TargetOperationKind::Run,
                dry_run: false,
                started_at_unix_ms: 100,
                process_id: Some(999_999),
                selector: "Books".to_string(),
                target_name: "Books".to_string(),
                target_id: None,
                local_path: PathBuf::from("/tmp/books"),
            }),
            std::process::id(),
        );

        assert!(detail.is_some());
        assert!(
            detail
                .expect("stale target detail")
                .contains("recorded target operation process 999999 is no longer active")
        );
    }

    #[test]
    fn runner_launch_agent_plist_includes_home_and_login_environment() {
        let plist = super::render_runner_launch_agent_plist(
            &crate::config::RunnerLaunchAgentConfig::default(),
            Path::new("/tmp/syncsteward-cli"),
            Path::new("/tmp/config.toml"),
        );

        assert!(plist.contains("<key>HOME</key>"));
        assert!(plist.contains("<key>USER</key>"));
        assert!(plist.contains("<key>LOGNAME</key>"));
    }

    #[test]
    fn active_target_operation_guard_records_and_clears_state() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let state_path = temp_root.join("state.json");
        fs::create_dir_all(&temp_root).expect("create temp root");

        {
            let _guard = ActiveTargetOperationGuard::new(
                &state_path,
                crate::state::ActiveTargetOperationState {
                    kind: TargetOperationKind::Rebaseline,
                    dry_run: false,
                    started_at_unix_ms: 100,
                    process_id: Some(42),
                    selector: "Business".to_string(),
                    target_name: "Business".to_string(),
                    target_id: Some("business".to_string()),
                    local_path: PathBuf::from("/tmp/business"),
                },
            );

            let state = load_state(&state_path).expect("load active target state");
            let operation = state
                .active_target_operation
                .expect("active target operation recorded");
            assert_eq!(operation.kind, TargetOperationKind::Rebaseline);
            assert_eq!(operation.process_id, Some(42));
            assert_eq!(operation.selector, "Business");
            assert_eq!(operation.target_name, "Business");
        }

        let state = load_state(&state_path).expect("load cleared target state");
        assert!(state.active_target_operation.is_none());
    }

    #[test]
    fn load_runtime_state_clears_stale_active_target_operation() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let state_path = temp_root.join("state.json");
        fs::create_dir_all(&temp_root).expect("create temp root");
        crate::state::save_active_target_operation(
            &state_path,
            Some(crate::state::ActiveTargetOperationState {
                kind: TargetOperationKind::Verify,
                dry_run: false,
                started_at_unix_ms: 100,
                process_id: Some(999_999),
                selector: "Books".to_string(),
                target_name: "Books".to_string(),
                target_id: None,
                local_path: PathBuf::from("/tmp/books"),
            }),
        )
        .expect("save stale active target operation");

        let mut config = AppConfig::default();
        config.state_path = state_path.clone();

        let state = super::load_runtime_state(&config).expect("load runtime state");
        assert!(state.active_target_operation.is_none());

        let recorded = crate::state::load_state(&state_path).expect("reload state");
        assert!(recorded.active_target_operation.is_none());
    }

    #[test]
    fn prune_state_removes_stale_target_runs_without_touching_current_entries() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let temp_path = temp_root.join("config.toml");
        let state_path = temp_root.join("state.json");
        let target_path = temp_root.join("Notes/Personal");
        fs::create_dir_all(&target_path).expect("create target path");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.sync_script_path = temp_root.join("cloud-sync.sh");
        config.state_path = state_path.clone();
        config.runner.approved_targets = vec!["managed-1".to_string()];
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: target_path.clone(),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        }];
        fs::write(
            &temp_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let mut state = AppState::default();
        state.target_runs.insert(
            "managed-1".to_string(),
            TargetRunState {
                target_name: "Notes/Personal".to_string(),
                target_id: Some("managed-1".to_string()),
                local_path: target_path.clone(),
                effective_mode: PolicyMode::BackupOnly,
                outcome: ActionOutcome::Success,
                dry_run: false,
                finished_at_unix_ms: 100,
                last_success_at_unix_ms: Some(100),
                last_verified_at_unix_ms: None,
                last_full_verified_at_unix_ms: None,
                last_verification_mode: None,
                last_failure_class: None,
                last_repair_at_unix_ms: None,
                last_rebaseline_at_unix_ms: None,
                consecutive_failure_count: 0,
                summary: "current".to_string(),
            },
        );
        state.target_runs.insert(
            "obsolete".to_string(),
            TargetRunState {
                target_name: "Old".to_string(),
                target_id: Some("obsolete".to_string()),
                local_path: temp_root.join("Old"),
                effective_mode: PolicyMode::BackupOnly,
                outcome: ActionOutcome::Failed,
                dry_run: false,
                finished_at_unix_ms: 50,
                last_success_at_unix_ms: None,
                last_verified_at_unix_ms: None,
                last_full_verified_at_unix_ms: None,
                last_verification_mode: None,
                last_failure_class: None,
                last_repair_at_unix_ms: None,
                last_rebaseline_at_unix_ms: None,
                consecutive_failure_count: 4,
                summary: "stale".to_string(),
            },
        );
        fs::write(
            &state_path,
            serde_json::to_string_pretty(&state).expect("serialize state"),
        )
        .expect("write state");

        let dry_run_report = prune_state(Some(temp_path.as_path()), true).expect("dry-run prune");
        assert!(dry_run_report.dry_run);
        assert_eq!(dry_run_report.removed_count, 1);
        assert!(
            fs::read_to_string(&state_path)
                .expect("read state")
                .contains("\"obsolete\"")
        );

        let live_report = prune_state(Some(temp_path.as_path()), false).expect("live prune");
        assert_eq!(live_report.outcome, ActionOutcome::Success);
        assert_eq!(live_report.removed_count, 1);

        let pruned_state = crate::state::load_state(&state_path).expect("load pruned state");
        assert_eq!(pruned_state.target_runs.len(), 1);
        assert!(pruned_state.target_runs.contains_key("managed-1"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn chronic_failure_alerts_are_visible_in_alert_evaluation() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let temp_path = temp_root.join("config.toml");
        let state_path = temp_root.join("state.json");
        let target_path = temp_root.join("Notes/Personal");
        fs::create_dir_all(&target_path).expect("create target path");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.sync_script_path = temp_root.join("cloud-sync.sh");
        config.state_path = state_path.clone();
        config.runner.approved_targets = vec!["managed-1".to_string()];
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: target_path.clone(),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        }];
        fs::write(
            &temp_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let mut state = AppState::default();
        state.target_runs.insert(
            "managed-1".to_string(),
            TargetRunState {
                target_name: "Notes/Personal".to_string(),
                target_id: Some("managed-1".to_string()),
                local_path: target_path.clone(),
                effective_mode: PolicyMode::BackupOnly,
                outcome: ActionOutcome::Failed,
                dry_run: false,
                finished_at_unix_ms: 100,
                last_success_at_unix_ms: None,
                last_verified_at_unix_ms: None,
                last_full_verified_at_unix_ms: None,
                last_verification_mode: None,
                last_failure_class: None,
                last_repair_at_unix_ms: None,
                last_rebaseline_at_unix_ms: None,
                consecutive_failure_count: 3,
                summary: "failed three times".to_string(),
            },
        );
        fs::write(
            &state_path,
            serde_json::to_string_pretty(&state).expect("serialize state"),
        )
        .expect("write state");

        let report = super::evaluate_alerts(&config, "test".to_string()).expect("alerts");
        assert!(
            report
                .alerts
                .iter()
                .any(|alert| alert.id == "target_Notes/Personal_chronic_failure")
        );

        let overview = super::build_chronic_failure_overview(
            &config.runner.approved_targets,
            &[super::evaluate_target(
                &super::evaluate_preflight(
                    super::collect_status(&config, "test".to_string()),
                    PreflightMode::ManagedRun,
                ),
                crate::model::SyncTargetRecord {
                    target_id: Some("managed-1".to_string()),
                    name: "Notes/Personal".to_string(),
                    local_path: target_path.clone(),
                    remote_path: "OneDrive/Notes/Personal".to_string(),
                    legacy_mode: crate::model::LegacySyncMode::Managed,
                    recommended_mode: PolicyMode::BackupOnly,
                    configured_mode: Some(PolicyMode::BackupOnly),
                    rationale: "test".to_string(),
                },
            )],
            &crate::state::load_state(&state_path).expect("load state"),
        );
        assert_eq!(overview.len(), 1);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn record_target_run_resets_failure_count_after_success() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let state_path = temp_root.join("state.json");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.state_path = state_path.clone();

        let target = SyncTargetRecord {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: temp_root.join("Notes/Personal"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            legacy_mode: LegacySyncMode::Managed,
            recommended_mode: PolicyMode::BackupOnly,
            configured_mode: Some(PolicyMode::BackupOnly),
            rationale: "test".to_string(),
        };
        let evaluation = TargetEvaluation {
            target: target.clone(),
            effective_mode: PolicyMode::BackupOnly,
            ready: true,
            blockers: Vec::new(),
        };

        let failure_report = TargetRunReport {
            config_source: "test".to_string(),
            selector: "Notes/Personal".to_string(),
            dry_run: false,
            outcome: ActionOutcome::Failed,
            summary: "run failed".to_string(),
            preflight_ready: true,
            evaluation: evaluation.clone(),
            verified_at_unix_ms: None,
            verification_mode: Some(VerificationMode::SizeAndSample),
            failure_class: Some(FailureClass::Unknown),
            steps: vec![ActionStep {
                id: "step-1".to_string(),
                status: ActionStepStatus::Failed,
                summary: "failed".to_string(),
                detail: "simulated failure".to_string(),
            }],
        };
        record_target_run(&config, &failure_report);
        let recorded = crate::state::load_state(&state_path).expect("load failure state");
        let state = recorded
            .target_runs
            .get(&target_state_key(&target))
            .expect("failure target run");
        assert_eq!(state.consecutive_failure_count, 1);
        assert!(state.last_success_at_unix_ms.is_none());

        let success_report = TargetRunReport {
            outcome: ActionOutcome::Success,
            summary: "run succeeded".to_string(),
            steps: vec![ActionStep {
                id: "step-1".to_string(),
                status: ActionStepStatus::Applied,
                summary: "ok".to_string(),
                detail: "simulated success".to_string(),
            }],
            ..failure_report
        };
        record_target_run(&config, &success_report);
        let recorded = crate::state::load_state(&state_path).expect("load success state");
        let state = recorded
            .target_runs
            .get(&target_state_key(&target))
            .expect("success target run");
        assert_eq!(state.consecutive_failure_count, 0);
        assert!(state.last_success_at_unix_ms.is_some());

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn record_target_run_preserves_failure_metadata_when_blocked() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let state_path = temp_root.join("state.json");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.state_path = state_path.clone();

        let target = SyncTargetRecord {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: temp_root.join("Notes/Personal"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            legacy_mode: LegacySyncMode::Managed,
            recommended_mode: PolicyMode::BackupOnly,
            configured_mode: Some(PolicyMode::BackupOnly),
            rationale: "test".to_string(),
        };
        let evaluation = TargetEvaluation {
            target: target.clone(),
            effective_mode: PolicyMode::BackupOnly,
            ready: true,
            blockers: Vec::new(),
        };

        let initial_state = TargetRunState {
            target_name: target.name.clone(),
            target_id: target.target_id.clone(),
            local_path: target.local_path.clone(),
            effective_mode: PolicyMode::BackupOnly,
            outcome: ActionOutcome::Failed,
            dry_run: false,
            finished_at_unix_ms: 10,
            last_success_at_unix_ms: None,
            last_verified_at_unix_ms: None,
            last_full_verified_at_unix_ms: None,
            last_verification_mode: None,
            last_failure_class: Some(FailureClass::Transport),
            last_repair_at_unix_ms: None,
            last_rebaseline_at_unix_ms: None,
            consecutive_failure_count: 2,
            summary: "failed".to_string(),
        };
        crate::state::save_target_run(&state_path, &target_state_key(&target), initial_state)
            .expect("seed target state");

        let blocked_report = TargetRunReport {
            config_source: "test".to_string(),
            selector: "Notes/Personal".to_string(),
            dry_run: false,
            outcome: ActionOutcome::Blocked,
            summary: "run blocked".to_string(),
            preflight_ready: false,
            evaluation,
            verified_at_unix_ms: None,
            verification_mode: None,
            failure_class: None,
            steps: vec![ActionStep {
                id: "preflight_gate".to_string(),
                status: ActionStepStatus::Blocked,
                summary: "blocked".to_string(),
                detail: "preflight failure".to_string(),
            }],
        };
        record_target_run(&config, &blocked_report);

        let recorded = crate::state::load_state(&state_path).expect("load blocked state");
        let state = recorded
            .target_runs
            .get(&target_state_key(&target))
            .expect("blocked target run");
        assert_eq!(state.consecutive_failure_count, 2);
        assert_eq!(state.last_failure_class, Some(FailureClass::Transport));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn verification_report_from_run_extracts_verification_steps() {
        let target = SyncTargetRecord {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: PathBuf::from("/tmp/notes"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            legacy_mode: LegacySyncMode::Managed,
            recommended_mode: PolicyMode::BackupOnly,
            configured_mode: Some(PolicyMode::BackupOnly),
            rationale: "test".to_string(),
        };
        let evaluation = TargetEvaluation {
            target,
            effective_mode: PolicyMode::BackupOnly,
            ready: true,
            blockers: Vec::new(),
        };
        let run = TargetRunReport {
            config_source: "test".to_string(),
            selector: "managed-1".to_string(),
            dry_run: false,
            outcome: ActionOutcome::Success,
            summary: "run and verification succeeded".to_string(),
            preflight_ready: true,
            evaluation,
            verified_at_unix_ms: Some(1234),
            verification_mode: Some(VerificationMode::Full),
            failure_class: None,
            steps: vec![
                ActionStep {
                    id: "rclone_sync".to_string(),
                    status: ActionStepStatus::Applied,
                    summary: "sync ok".to_string(),
                    detail: "sync ok".to_string(),
                },
                ActionStep {
                    id: "rclone_check".to_string(),
                    status: ActionStepStatus::Applied,
                    summary: "verify ok".to_string(),
                    detail: "verify ok".to_string(),
                },
            ],
        };

        let verification = super::verification_report_from_run(&run).expect("verification");
        assert_eq!(verification.outcome, ActionOutcome::Success);
        assert_eq!(verification.verified_at_unix_ms, Some(1234));
        assert_eq!(verification.steps.len(), 1);
        assert_eq!(verification.steps[0].id, "rclone_check");
    }

    #[test]
    fn classify_failure_from_steps_detects_divergence() {
        let steps = vec![ActionStep {
            id: "rclone_check".to_string(),
            status: ActionStepStatus::Failed,
            summary: "verification failed".to_string(),
            detail: "files differ between source and destination".to_string(),
        }];

        let class = super::classify_failure_from_steps(ActionOutcome::Failed, &steps);
        assert_eq!(class, Some(FailureClass::Divergence));
    }

    #[test]
    fn check_mismatch_report_deduplicates_copy_and_delete_paths() {
        let report = super::CheckMismatchReport {
            differ: vec!["a".to_string(), "b".to_string()],
            missing_on_dst: vec!["b".to_string(), "c".to_string()],
            missing_on_src: vec!["d".to_string(), "d".to_string()],
            errors: Vec::new(),
        };

        assert_eq!(
            report.copy_paths(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(report.delete_paths(), vec!["d".to_string()]);
        assert!(!report.is_clean());
    }

    #[test]
    fn summarize_target_recovery_reports_recovered_verification() {
        let run = TargetRunReport {
            config_source: "test".to_string(),
            selector: "Business".to_string(),
            dry_run: false,
            outcome: ActionOutcome::Failed,
            summary: "run failed for Business".to_string(),
            preflight_ready: true,
            evaluation: TargetEvaluation {
                target: crate::model::SyncTargetRecord {
                    target_id: None,
                    name: "Business".to_string(),
                    local_path: PathBuf::from("/tmp/Business"),
                    remote_path: "OneDrive/Business".to_string(),
                    legacy_mode: crate::model::LegacySyncMode::Bisync,
                    recommended_mode: PolicyMode::BackupOnly,
                    configured_mode: Some(PolicyMode::BackupOnly),
                    rationale: "test".to_string(),
                },
                effective_mode: PolicyMode::BackupOnly,
                ready: true,
                blockers: Vec::new(),
            },
            verified_at_unix_ms: None,
            verification_mode: None,
            failure_class: Some(FailureClass::Divergence),
            steps: Vec::new(),
        };

        let summary = super::summarize_target_recovery(
            super::RecoveryAction::Repair,
            ActionOutcome::Success,
            &run,
            None,
        );

        assert_eq!(summary, "repair recovered and verified Business");
    }

    #[test]
    fn read_and_write_path_list_round_trip() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let list_path = temp_root.join("paths.txt");
        let entries = vec![
            "one/file.txt".to_string(),
            "two/file.txt".to_string(),
            "three/file.txt".to_string(),
        ];

        super::write_path_list(&list_path, &entries).expect("write path list");
        let round_trip = super::read_path_list(&list_path).expect("read path list");

        assert_eq!(round_trip, entries);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn contains_blocking_steps_ignores_applied_only_sequences() {
        let applied_only = vec![
            ActionStep {
                id: "legacy_lock".to_string(),
                status: ActionStepStatus::Applied,
                summary: "acquired".to_string(),
                detail: "ok".to_string(),
            },
            ActionStep {
                id: "select_remote_host".to_string(),
                status: ActionStepStatus::Applied,
                summary: "selected".to_string(),
                detail: "ok".to_string(),
            },
        ];
        let blocked = vec![ActionStep {
            id: "preflight_gate".to_string(),
            status: ActionStepStatus::Blocked,
            summary: "blocked".to_string(),
            detail: "blocked".to_string(),
        }];

        assert!(!super::contains_blocking_steps(&applied_only));
        assert!(super::contains_blocking_steps(&blocked));
    }

    #[test]
    fn acquire_legacy_lock_blocks_reentry_while_guard_is_held() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.legacy_lock_path = temp_root.join("cloud-sync.lock");

        let mut first = None;
        let mut second = None;

        let acquired = super::acquire_legacy_lock(&config, &mut first).expect("acquire first");
        let blocked = super::acquire_legacy_lock(&config, &mut second).expect("acquire second");

        assert_eq!(acquired.status, ActionStepStatus::Applied);
        assert_eq!(blocked.status, ActionStepStatus::Blocked);
        assert!(blocked.detail.contains(&std::process::id().to_string()));

        drop(first);
        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn acquire_legacy_lock_replaces_stale_pid_file() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.legacy_lock_path = temp_root.join("cloud-sync.lock");
        fs::write(&config.legacy_lock_path, "999999").expect("write stale lock");

        let mut guard = None;
        let acquired = super::acquire_legacy_lock(&config, &mut guard).expect("acquire lock");

        assert_eq!(acquired.status, ActionStepStatus::Applied);
        assert_eq!(
            fs::read_to_string(&config.legacy_lock_path).expect("read active lock"),
            std::process::id().to_string()
        );

        drop(guard);
        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn rebaseline_requires_explicit_confirmation() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let config_path = temp_root.join("config.toml");
        let state_path = temp_root.join("state.json");
        let audit_path = temp_root.join("audit.jsonl");
        let target_path = temp_root.join("Notes/Personal");

        fs::create_dir_all(&target_path).expect("create target path");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.state_path = state_path;
        config.audit_log_path = audit_path;
        config.legacy_lock_path = temp_root.join("cloud-sync.lock");
        config.remote.preferred_hosts = vec!["127.0.0.1".to_string()];
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: target_path,
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        }];
        fs::write(
            &config_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let report =
            super::rebaseline_target(Some(config_path.as_path()), "managed-1", false, false)
                .expect("rebaseline report");

        assert_eq!(report.outcome, ActionOutcome::Blocked);
        assert!(report.confirmation_required);
        assert!(!report.confirmed);
        assert!(report.run.is_none());
        assert!(report.verification.is_none());
        assert_eq!(report.recovery_steps.len(), 1);
        assert_eq!(report.recovery_steps[0].id, "confirm_rebaseline");
        assert_eq!(report.recovery_steps[0].status, ActionStepStatus::Blocked);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn rebaseline_dry_run_skips_confirmation_gate() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let config_path = temp_root.join("config.toml");
        let state_path = temp_root.join("state.json");
        let audit_path = temp_root.join("audit.jsonl");
        let target_path = temp_root.join("Notes/Personal");

        fs::create_dir_all(&target_path).expect("create target path");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.state_path = state_path;
        config.audit_log_path = audit_path;
        config.legacy_lock_path = temp_root.join("cloud-sync.lock");
        config.remote.preferred_hosts = vec!["127.0.0.1".to_string()];
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: target_path,
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        }];
        fs::write(
            &config_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let report =
            super::rebaseline_target(Some(config_path.as_path()), "managed-1", true, false)
                .expect("rebaseline dry-run report");

        assert_eq!(report.outcome, ActionOutcome::Failed);
        assert!(!report.confirmation_required);
        assert!(report.confirmed);
        assert!(report.run.is_some());
        assert_eq!(report.recovery_steps.len(), 1);
        assert_eq!(report.recovery_steps[0].id, "confirm_rebaseline");
        assert_eq!(report.recovery_steps[0].status, ActionStepStatus::Skipped);

        let _ = fs::remove_dir_all(temp_root);
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
        assert!(report.backup_path.is_some());
        assert!(report.backup_path.as_ref().expect("backup path").exists());

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
        assert!(report.backup_path.is_some());
        assert!(report.backup_path.as_ref().expect("backup path").exists());

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
        assert!(report.backup_path.is_some());
        assert!(report.backup_path.as_ref().expect("backup path").exists());

        let loaded = load_config(Some(temp_path.as_path())).expect("load config");
        assert_eq!(
            loaded.config.managed_targets[0].target_id.as_deref(),
            Some("target-123")
        );
        assert_eq!(loaded.config.managed_targets[0].local_path, relocated_path);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn config_update_dry_run_does_not_mutate_file() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let temp_path = temp_root.join("config.toml");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.launch_agent_label = "com.example.sync".to_string();
        config.sync_script_path = PathBuf::from("~/bin/cloud-sync.sh");
        let original = toml::to_string_pretty(&config).expect("serialize config");
        fs::write(&temp_path, &original).expect("write config");

        let report = update_config(
            Some(temp_path.as_path()),
            ConfigPatch {
                launch_agent_label: Some("com.example.updated".to_string()),
                ..ConfigPatch::default()
            },
            true,
        )
        .expect("dry-run config update");

        assert!(report.dry_run);
        assert_eq!(report.outcome, ActionOutcome::Success);
        assert_eq!(report.config.launch_agent_label, "com.example.updated");
        assert_eq!(
            fs::read_to_string(&temp_path).expect("read config"),
            original
        );

        let loaded = load_config(Some(temp_path.as_path())).expect("reload config");
        assert_eq!(loaded.config.launch_agent_label, "com.example.sync");

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn config_update_applies_patch_and_persists() {
        let temp_root = std::env::temp_dir().join(format!("syncsteward-test-{}", Uuid::now_v7()));
        let temp_path = temp_root.join("config.toml");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.runner.cycle_interval_minutes = 60;
        let original = toml::to_string_pretty(&config).expect("serialize config");
        fs::write(&temp_path, &original).expect("write config");

        let report = update_config(
            Some(temp_path.as_path()),
            ConfigPatch {
                runner: Some(RunnerConfigPatch {
                    cycle_interval_minutes: Some(30),
                    notify_after_cycle: Some(false),
                    ..RunnerConfigPatch::default()
                }),
                remote: Some(RemoteConfigPatch {
                    ssh_user: Some("deaton".to_string()),
                    ..RemoteConfigPatch::default()
                }),
                ..ConfigPatch::default()
            },
            false,
        )
        .expect("update config");

        assert_eq!(report.outcome, ActionOutcome::Success);
        assert!(!report.dry_run);
        assert_eq!(
            report.changed_fields,
            vec![
                "remote.ssh_user",
                "runner.cycle_interval_minutes",
                "runner.notify_after_cycle",
            ]
        );
        assert!(report.backup_path.is_some());
        assert!(report.backup_path.as_ref().expect("backup path").exists());
        assert_eq!(
            fs::read_to_string(report.backup_path.as_ref().expect("backup path"))
                .expect("read backup"),
            original
        );

        let loaded = load_config(Some(temp_path.as_path())).expect("reload config");
        assert_eq!(loaded.config.runner.cycle_interval_minutes, 30);
        assert!(!loaded.config.runner.notify_after_cycle);
        assert_eq!(loaded.config.remote.ssh_user, "deaton");

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn runner_due_status_detects_when_cycle_is_due() {
        let (due_without_history, next_due_without_history) = runner_due_status(None, 60_000, 123);
        assert!(due_without_history);
        assert_eq!(next_due_without_history, None);

        let (due_too_soon, next_due_too_soon) = runner_due_status(Some(1_000), 60_000, 30_000);
        assert!(!due_too_soon);
        assert_eq!(next_due_too_soon, Some(61_000));

        let (due_after_interval, next_due_after_interval) =
            runner_due_status(Some(1_000), 60_000, 61_000);
        assert!(due_after_interval);
        assert_eq!(next_due_after_interval, Some(61_000));
    }

    #[test]
    fn resolve_program_path_finds_common_system_binary() {
        let resolved = super::resolve_program_path("sh");
        assert!(resolved.exists());
        assert_eq!(
            resolved.file_name().and_then(|value| value.to_str()),
            Some("sh")
        );
    }

    #[test]
    fn runner_path_environment_includes_usr_local_bin() {
        let path = super::runner_path_environment();
        assert!(path.split(':').any(|segment| segment == "/usr/local/bin"));
        assert!(path.split(':').any(|segment| segment == "/usr/bin"));
    }

    #[test]
    fn rclone_attempt_hosts_rotate_across_preferred_hosts() {
        let mut config = AppConfig::default();
        config.remote.preferred_hosts =
            vec!["192.168.77.135".to_string(), "192.168.195.155".to_string()];

        let hosts = super::rclone_attempt_hosts(&config, "192.168.77.135", 4);

        assert_eq!(
            hosts,
            vec![
                "192.168.77.135".to_string(),
                "192.168.195.155".to_string(),
                "192.168.77.135".to_string(),
                "192.168.195.155".to_string()
            ]
        );
    }

    #[test]
    fn write_target_rclone_config_uses_external_ssh_command_in_auto_mode() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-rclone-config-{}-{}",
            std::process::id(),
            super::now_unix_ms()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let path = temp_root.join("rclone.conf");
        let config = AppConfig::default();

        write_target_rclone_config_at_path(&config, "192.168.77.135", &path)
            .expect("write rclone config");

        let contents = fs::read_to_string(&path).expect("read rclone config");
        if cfg!(target_os = "macos") {
            assert!(contents.contains("ssh = /usr/bin/ssh"));
            assert!(contents.contains("ControlMaster=auto"));
            assert!(contents.contains("ControlPath=/tmp/syncsteward-%r@%h-%p"));
            assert!(contents.contains("john@192.168.77.135"));
            assert!(!contents.contains("key_file ="));
        } else {
            assert!(contents.contains("key_file ="));
        }

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn write_target_rclone_config_supports_internal_ssh_mode() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-rclone-config-internal-{}-{}",
            std::process::id(),
            super::now_unix_ms()
        ));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let path = temp_root.join("rclone.conf");
        let mut config = AppConfig::default();
        config.remote.rclone_ssh_mode = RcloneSshMode::Internal;

        write_target_rclone_config_at_path(&config, "192.168.77.135", &path)
            .expect("write rclone config");

        let contents = fs::read_to_string(&path).expect("read rclone config");
        assert!(contents.contains("host = 192.168.77.135"));
        assert!(contents.contains("key_file ="));
        assert!(!contents.contains("ssh = /usr/bin/ssh"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn determine_verification_mode_uses_size_and_sample_without_full_history() {
        let config = AppConfig::default();
        let (mode, detail) = super::determine_verification_mode(
            &config,
            super::VerificationTrigger::ScheduledRun,
            None,
        );
        assert_eq!(mode, VerificationMode::SizeAndSample);
        assert!(detail.contains("no prior full verification"));
    }

    #[test]
    fn determine_verification_mode_uses_size_and_sample_with_recent_full_verification() {
        let config = AppConfig::default();
        let recent = super::now_unix_ms().saturating_sub(60 * 60 * 1000);
        let run_state = TargetRunState {
            target_name: "Books".to_string(),
            target_id: Some("books".to_string()),
            local_path: PathBuf::from("/tmp/Books"),
            effective_mode: PolicyMode::BackupOnly,
            outcome: ActionOutcome::Success,
            dry_run: false,
            finished_at_unix_ms: recent,
            last_success_at_unix_ms: Some(recent),
            last_verified_at_unix_ms: Some(recent),
            last_full_verified_at_unix_ms: Some(recent),
            last_verification_mode: Some(VerificationMode::Full),
            last_failure_class: None,
            last_repair_at_unix_ms: None,
            last_rebaseline_at_unix_ms: None,
            consecutive_failure_count: 0,
            summary: "verified".to_string(),
        };

        let (mode, detail) = super::determine_verification_mode(
            &config,
            super::VerificationTrigger::ScheduledRun,
            Some(&run_state),
        );
        assert_eq!(mode, VerificationMode::SizeAndSample);
        assert!(detail.contains("size-and-sample"));
    }

    #[test]
    fn select_sample_paths_spreads_across_candidate_list() {
        let candidates = (0..10)
            .map(|index| format!("file-{index}.txt"))
            .collect::<Vec<_>>();
        let samples = super::select_sample_paths(&candidates, 4);
        assert_eq!(
            samples,
            vec![
                "file-0.txt".to_string(),
                "file-3.txt".to_string(),
                "file-6.txt".to_string(),
                "file-9.txt".to_string(),
            ]
        );
    }

    #[test]
    fn collect_target_status_scans_only_requested_roots() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-target-status-{}-{}",
            std::process::id(),
            super::now_unix_ms()
        ));
        let root_one = temp_root.join("one");
        let root_two = temp_root.join("two");
        fs::create_dir_all(&root_one).expect("create root one");
        fs::create_dir_all(&root_two).expect("create root two");
        fs::write(root_one.join("alpha.conflict.md"), "alpha").expect("write root one conflict");
        fs::write(root_two.join("beta.conflict.md"), "beta").expect("write root two conflict");

        let mut config = AppConfig::default();
        config.scan.roots = vec![root_one.clone(), root_two.clone()];
        let report =
            collect_target_status(&config, "test".to_string(), std::slice::from_ref(&root_one));

        assert_eq!(report.artifacts.roots_scanned, vec![root_one.clone()]);
        assert_eq!(report.artifacts.conflict_count, 1);
        assert_eq!(
            report.artifacts.conflict_examples,
            vec![root_one.join("alpha.conflict.md")]
        );

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn collect_status_scans_legacy_inventory_roots_for_artifacts() {
        let temp_name = format!("syncsteward-legacy-artifacts-{}", Uuid::now_v7());
        let home_root = crate::config::current_home_dir().join(&temp_name);
        let script_root =
            std::env::temp_dir().join(format!("syncsteward-legacy-script-{}", Uuid::now_v7()));
        let script_path = script_root.join("cloud-sync.sh");
        fs::create_dir_all(&home_root).expect("create home target root");
        fs::create_dir_all(&script_root).expect("create script root");
        fs::write(home_root.join("alpha.conflict.md"), "alpha").expect("write conflict");
        fs::write(
            &script_path,
            format!("BISYNC_FOLDERS=(\n    \"{temp_name}\"\n)\n\nBACKUP_FOLDERS=(\n    \".memloft:.memloft\"\n)\n"),
        )
        .expect("write legacy script");

        let mut config = AppConfig::default();
        config.scan.roots = Vec::new();
        config.managed_targets = Vec::new();
        config.sync_script_path = script_path;

        let report = super::collect_status(&config, "test".to_string());

        assert!(report.artifacts.roots_scanned.contains(&home_root));
        assert_eq!(report.artifacts.conflict_count, 1);
        assert_eq!(
            report.artifacts.conflict_examples,
            vec![home_root.join("alpha.conflict.md")]
        );

        let _ = fs::remove_dir_all(&script_root);
        let _ = fs::remove_dir_all(&home_root);
    }

    #[test]
    fn quarantine_artifacts_moves_matches_into_quarantine_root() {
        let temp_root =
            std::env::temp_dir().join(format!("syncsteward-quarantine-{}", Uuid::now_v7()));
        let config_path = temp_root.join("config.toml");
        let state_path = temp_root.join("state.json");
        let target_root = temp_root.join("Music");
        fs::create_dir_all(&target_root).expect("create target root");
        let conflict = target_root.join("alpha.conflict.md");
        let safe_backup = target_root.join("beta-victorystore-safeBackup-0001.db");
        fs::write(&conflict, "alpha").expect("write conflict");
        fs::write(&safe_backup, "beta").expect("write safe backup");

        let mut config = AppConfig::default();
        config.state_path = state_path;
        config.sync_script_path = temp_root.join("missing-cloud-sync.sh");
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("music".to_string()),
            name: "Music".to_string(),
            local_path: target_root.clone(),
            remote_path: "OneDrive/Music".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        }];
        fs::create_dir_all(&temp_root).expect("create temp root");
        fs::write(
            &config_path,
            toml::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config");

        let selectors = vec!["Music".to_string()];
        let report = super::quarantine_artifacts(Some(&config_path), &selectors, false)
            .expect("quarantine artifacts");

        assert_eq!(report.outcome, ActionOutcome::Success);
        assert_eq!(report.moved_count, 2);
        assert!(report.manifest_path.is_some());
        assert!(!conflict.exists());
        assert!(!safe_backup.exists());
        for artifact in &report.artifacts {
            assert!(artifact.quarantine_path.exists());
        }

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn run_spawned_command_with_retry_recovers_after_transient_failure() {
        let marker = std::env::temp_dir().join(format!(
            "syncsteward-retry-{}-{}",
            std::process::id(),
            super::now_unix_ms()
        ));
        let output = run_spawned_command_with_retry(
            || {
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg(
                        "if [ -f \"$1\" ]; then echo ok; exit 0; else touch \"$1\"; echo dial tcp 192.168.1.10:22: connect: connection refused >&2; exit 1; fi",
                    )
                    .arg("sh")
                    .arg(&marker);
                command
            },
            2,
            Duration::from_millis(1),
        );
        let _ = fs::remove_file(&marker);

        assert!(output.success);
        assert_eq!(output.attempts, 2);
        assert!(summarize_command_output(&output).contains("succeeded after 2 attempts"));
    }

    #[test]
    fn run_spawned_command_with_retry_reports_persistent_failure() {
        let output = run_spawned_command_with_retry(
            || {
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg("echo dial tcp 192.168.1.10:22: connect: connection refused >&2; exit 1");
                command
            },
            2,
            Duration::from_millis(1),
        );

        assert!(!output.success);
        assert_eq!(output.attempts, 2);
        assert!(summarize_command_output(&output).contains("failed after 2 attempts"));
    }

    #[test]
    fn run_spawned_command_with_retry_stops_on_non_retryable_failure() {
        let output = run_spawned_command_with_retry(
            || {
                let mut command = Command::new("sh");
                command
                    .arg("-c")
                    .arg("echo files differ between source and destination >&2; exit 1");
                command
            },
            3,
            Duration::from_millis(1),
        );

        assert!(!output.success);
        assert_eq!(output.attempts, 1);
        assert!(!summarize_command_output(&output).contains("failed after 3 attempts"));
    }

    #[test]
    fn alert_signature_is_stable_across_order() {
        let a = AlertRecord {
            id: "a".to_string(),
            severity: AlertSeverity::Warn,
            summary: "A".to_string(),
            detail: "detail-a".to_string(),
            target_name: Some("one".to_string()),
        };
        let b = AlertRecord {
            id: "b".to_string(),
            severity: AlertSeverity::Critical,
            summary: "B".to_string(),
            detail: "detail-b".to_string(),
            target_name: Some("two".to_string()),
        };

        assert_eq!(
            alert_signature(&[a.clone(), b.clone()]),
            alert_signature(&[b, a])
        );
    }

    #[test]
    fn scheduled_notifications_suppress_unchanged_alerts_inside_repeat_window() {
        let mut config = AppConfig::default();
        config.alerts.repeat_notification_after_minutes = 240;

        let alerts = vec![AlertRecord {
            id: "target_notes_blocked".to_string(),
            severity: AlertSeverity::Warn,
            summary: "Notes cannot run yet".to_string(),
            detail: "policy_hold".to_string(),
            target_name: Some("Notes".to_string()),
        }];
        let signature = alert_signature(&alerts);
        let state = AppState {
            alert_notifications: AlertNotificationState {
                active_signature: Some(signature.clone()),
                active_since_unix_ms: Some(1),
                last_notified_signature: Some(signature),
                last_notified_at_unix_ms: Some(super::now_unix_ms()),
                repeat_count: 1,
            },
            ..AppState::default()
        };

        let decision =
            scheduled_notify_alerts(&config, &state, &alerts, true).expect("scheduled decision");

        assert_eq!(decision.report.outcome, ActionOutcome::NoOp);
        assert!(
            decision
                .report
                .summary
                .contains("suppressed repeat notification")
        );
        assert!(decision.updated_state.is_none());
    }

    #[test]
    fn scheduled_notifications_clear_state_when_alerts_recover() {
        let mut config = AppConfig::default();
        config.alerts.enable_macos_notifications = false;
        config.alerts.recovery_notifications = false;

        let state = AppState {
            alert_notifications: AlertNotificationState {
                active_signature: Some("active".to_string()),
                active_since_unix_ms: Some(1),
                last_notified_signature: Some("active".to_string()),
                last_notified_at_unix_ms: Some(2),
                repeat_count: 3,
            },
            ..AppState::default()
        };

        let decision =
            scheduled_notify_alerts(&config, &state, &[], false).expect("recovery decision");

        assert_eq!(decision.report.outcome, ActionOutcome::NoOp);
        assert_eq!(
            decision.report.summary,
            "alerts cleared without recovery notification"
        );
        assert_eq!(
            decision
                .updated_state
                .expect("updated state")
                .active_signature,
            None
        );
    }
}
