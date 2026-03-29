use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use syncsteward_core::{
    ActionOutcome, ActionStepStatus, ActionTarget, AlertReport, AlertSeverity, CheckStatus,
    ConfigScaffoldReport, ControlReport, LogAcknowledgeReport, NotifyAlertsReport, PolicyMode,
    PreflightReport, StatusReport, SyncTargetInventoryReport, TargetCheckReport,
    TargetCheckSetReport, TargetRunReport, acknowledge_latest_log, alerts, check_target,
    check_targets, notify_alerts, pause, preflight, resume, run_target, scaffold_config, status,
    targets,
};

#[derive(Debug, Parser)]
#[command(
    name = "syncsteward",
    about = "Safety-first sync health and preflight control"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Read the current sync health snapshot.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Run guarded preflight checks before re-enabling sync.
    Preflight {
        #[arg(long)]
        json: bool,
    },
    /// Read the legacy sync targets from the current cloud-sync script and show the safer recommended policy for each one.
    Targets {
        #[arg(long)]
        json: bool,
    },
    /// Explain readiness and blockers for every configured sync target.
    CheckTargets {
        #[arg(long)]
        json: bool,
    },
    /// Explain readiness and blockers for one configured sync target.
    CheckTarget {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Evaluate active alerts from preflight and target run history.
    Alerts {
        #[arg(long)]
        json: bool,
    },
    /// Send a local notification for current alerts.
    NotifyAlerts {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Run one approved target with preflight and policy gating.
    RunTarget {
        target: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Acknowledge the latest rclone log as historical baseline state.
    AcknowledgeLatestLog {
        #[arg(long)]
        json: bool,
    },
    /// Write a SyncSteward config scaffold from the current target inventory.
    ScaffoldConfig {
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Pause the local launch agent, remote OneDrive service, or both.
    Pause {
        #[arg(long, value_enum, default_value_t = TargetArg::All)]
        target: TargetArg,
        #[arg(long)]
        json: bool,
    },
    /// Resume the local launch agent, remote OneDrive service, or both.
    /// Resume stays blocked until preflight passes.
    Resume {
        #[arg(long, value_enum, default_value_t = TargetArg::All)]
        target: TargetArg,
        #[arg(long)]
        json: bool,
    },
    /// Run the MCP server.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// Serve MCP over stdio.
    Stdio,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum TargetArg {
    Local,
    Remote,
    All,
}

fn main() -> Result<(), String> {
    std::process::exit(run());
}

fn run() -> i32 {
    let cli = Cli::parse();
    match cli.command {
        Command::Status { json } => {
            let report = match status(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize status as JSON");
                }
            } else {
                print_status(&report);
            }
            0
        }
        Command::Preflight { json } => {
            let report = match preflight(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize preflight as JSON");
                }
            } else {
                print_preflight(&report);
            }
            0
        }
        Command::Targets { json } => {
            let report = match targets(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize target inventory as JSON");
                }
            } else {
                print_targets(&report);
            }
            0
        }
        Command::CheckTargets { json } => {
            let report = match check_targets(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize target readiness report as JSON");
                }
            } else {
                print_check_targets(&report);
            }
            if report.preflight_ready
                && report.evaluations.iter().all(|evaluation| evaluation.ready)
            {
                0
            } else {
                2
            }
        }
        Command::CheckTarget { target, json } => {
            let report = match check_target(cli.config.as_deref(), &target) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize target readiness report as JSON");
                }
            } else {
                print_check_target(&report);
            }
            if report.preflight_ready && report.evaluation.ready {
                0
            } else {
                2
            }
        }
        Command::Alerts { json } => {
            let report = match alerts(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize alert report as JSON");
                }
            } else {
                print_alerts(&report);
            }
            if report.alerts.is_empty() { 0 } else { 2 }
        }
        Command::NotifyAlerts { dry_run, json } => {
            let report = match notify_alerts(cli.config.as_deref(), dry_run) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize notify-alerts report as JSON");
                }
            } else {
                print_notify_alerts(&report);
            }
            action_exit_code(report.outcome)
        }
        Command::RunTarget {
            target,
            dry_run,
            json,
        } => {
            let report = match run_target(cli.config.as_deref(), &target, dry_run) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize target run report as JSON");
                }
            } else {
                print_run_target(&report);
            }
            action_exit_code(report.outcome)
        }
        Command::AcknowledgeLatestLog { json } => {
            let report = match acknowledge_latest_log(cli.config.as_deref()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize log acknowledgement report as JSON");
                }
            } else {
                print_acknowledged_log(&report);
            }
            action_exit_code(report.outcome)
        }
        Command::ScaffoldConfig { force, json } => {
            let report = match scaffold_config(cli.config.as_deref(), force) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize config scaffold report as JSON");
                }
            } else {
                print_config_scaffold(&report);
            }
            action_exit_code(report.outcome)
        }
        Command::Pause { target, json } => {
            let report = match pause(cli.config.as_deref(), target.into()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize pause report as JSON");
                }
            } else {
                print_control(&report);
            }
            control_exit_code(&report)
        }
        Command::Resume { target, json } => {
            let report = match resume(cli.config.as_deref(), target.into()) {
                Ok(report) => report,
                Err(error) => return fatal_error(&error.to_string()),
            };
            if json {
                if print_json(&report).is_err() {
                    return fatal_error("failed to serialize resume report as JSON");
                }
            } else {
                print_control(&report);
            }
            control_exit_code(&report)
        }
        Command::Mcp {
            command: McpCommand::Stdio,
        } => match syncsteward_mcp::serve_stdio_blocking(cli.config) {
            Ok(()) => 0,
            Err(error) => fatal_error(&error),
        },
    }
}

impl From<TargetArg> for ActionTarget {
    fn from(value: TargetArg) -> Self {
        match value {
            TargetArg::Local => ActionTarget::Local,
            TargetArg::Remote => ActionTarget::Remote,
            TargetArg::All => ActionTarget::All,
        }
    }
}

fn print_status(report: &StatusReport) {
    println!("SyncSteward Status");
    println!("Config source: {}", report.config_source);
    println!(
        "Policies: {} folder overrides, {} file-class defaults",
        report.policy.folder_policies.len(),
        report.policy.file_class_policies.len()
    );
    for entry in &report.policy.file_class_policies {
        println!(
            "  {:?}: {} [{}]",
            entry.class,
            describe_policy_mode(entry.mode),
            entry.patterns.join(", ")
        );
    }
    println!(
        "Local launch agent: {} ({})",
        if report.launch_agent.loaded {
            if report.launch_agent.running {
                "loaded and running"
            } else {
                "loaded"
            }
        } else {
            "not loaded"
        },
        report.launch_agent.label
    );
    println!("  {}", report.launch_agent.detail);

    let remote_host = report
        .remote
        .selected_host
        .as_deref()
        .unwrap_or("none reachable");
    println!(
        "Remote OneDrive: {:?} (host: {})",
        report.remote.service_state, remote_host
    );
    println!("  {}", report.remote.detail);

    println!(
        "Artifacts: {} conflict, {} safeBackup",
        report.artifacts.conflict_count, report.artifacts.safe_backup_count
    );
    print_path_samples("Conflict samples", &report.artifacts.conflict_examples);
    print_path_samples("safeBackup samples", &report.artifacts.safe_backup_examples);
    if let Some(acknowledged) = &report.acknowledged_log {
        println!(
            "Acknowledged log baseline: {} ({} errors, {} out-of-sync, {} warnings)",
            acknowledged.path.display(),
            acknowledged.error_count,
            acknowledged.out_of_sync_count,
            acknowledged.warning_count
        );
    } else {
        println!("Acknowledged log baseline: none");
    }

    if let Some(log) = &report.latest_log {
        println!(
            "Latest log: {} ({} warnings, {} errors, {} out-of-sync)",
            log.path.display(),
            log.warning_count,
            log.error_count,
            log.out_of_sync_count
        );
        if let Some(line) = &log.last_started_line {
            println!("  Last start: {}", line);
        }
        if let Some(line) = &log.last_completed_line {
            println!("  Last completion: {}", line);
        }
        if !log.issue_examples.is_empty() {
            println!("  Issue examples:");
            for line in &log.issue_examples {
                println!("    {}", line);
            }
        }
    } else {
        println!("Latest log: none found");
    }
}

fn print_preflight(report: &PreflightReport) {
    println!(
        "SyncSteward Preflight: {}",
        if report.ready { "READY" } else { "BLOCKED" }
    );
    for check in &report.checks {
        let badge = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        };
        println!("[{}] {}: {}", badge, check.id, check.summary);
        println!("  {}", check.detail);
    }

    println!();
    print_status(&report.status);
}

fn print_control(report: &ControlReport) {
    println!(
        "SyncSteward {}: {:?}",
        format!("{:?}", report.action).to_lowercase(),
        report.outcome
    );
    println!("{}", report.summary);
    for step in &report.steps {
        println!(
            "[{}] {}: {}",
            describe_step_status(step.status),
            step.id,
            step.summary
        );
        println!("  {}", step.detail);
    }
    if let Some(preflight) = &report.preflight {
        println!();
        print_preflight(preflight);
    } else {
        println!();
        print_status(&report.status);
    }
}

fn print_targets(report: &SyncTargetInventoryReport) {
    println!("SyncSteward Target Inventory");
    println!("Config source: {}", report.config_source);
    println!("Script path: {}", report.script_path.display());
    for target in &report.targets {
        println!(
            "- {} [{} -> {}]",
            target.name,
            describe_legacy_mode(target.legacy_mode),
            describe_policy_mode(target.recommended_mode),
        );
        println!("  local: {}", target.local_path.display());
        println!("  remote: {}", target.remote_path);
        if let Some(mode) = target.configured_mode {
            println!("  configured override: {}", describe_policy_mode(mode));
        }
        println!("  why: {}", target.rationale);
    }
}

fn print_check_targets(report: &TargetCheckSetReport) {
    println!(
        "SyncSteward Target Readiness: {}",
        if report.preflight_ready {
            "PRECHECK OK"
        } else {
            "PRECHECK BLOCKED"
        }
    );
    println!("Config source: {}", report.config_source);
    for evaluation in &report.evaluations {
        print_target_evaluation(evaluation);
    }
}

fn print_check_target(report: &TargetCheckReport) {
    println!(
        "SyncSteward Target Readiness: {}",
        if report.preflight_ready {
            "PRECHECK OK"
        } else {
            "PRECHECK BLOCKED"
        }
    );
    println!("Config source: {}", report.config_source);
    println!("Selector: {}", report.selector);
    print_target_evaluation(&report.evaluation);
}

fn print_target_evaluation(evaluation: &syncsteward_core::TargetEvaluation) {
    println!(
        "- {} [{}]",
        evaluation.target.name,
        if evaluation.ready { "ready" } else { "blocked" }
    );
    println!("  local: {}", evaluation.target.local_path.display());
    println!("  remote: {}", evaluation.target.remote_path);
    println!(
        "  legacy: {}, effective mode: {}",
        describe_legacy_mode(evaluation.target.legacy_mode),
        describe_policy_mode(evaluation.effective_mode)
    );
    if let Some(mode) = evaluation.target.configured_mode {
        println!("  configured override: {}", describe_policy_mode(mode));
    }
    if evaluation.blockers.is_empty() {
        println!("  blockers: none");
    } else {
        println!("  blockers:");
        for blocker in &evaluation.blockers {
            println!("    {}: {}", blocker.id, blocker.summary);
            println!("      {}", blocker.detail);
        }
    }
}

fn print_run_target(report: &TargetRunReport) {
    println!("SyncSteward Target Run: {:?}", report.outcome);
    println!("{}", report.summary);
    println!("Config source: {}", report.config_source);
    println!("Selector: {}", report.selector);
    println!("Dry run: {}", if report.dry_run { "yes" } else { "no" });
    println!(
        "Preflight ready: {}",
        if report.preflight_ready { "yes" } else { "no" }
    );
    print_target_evaluation(&report.evaluation);
    println!("  execution steps:");
    for step in &report.steps {
        println!(
            "    [{}] {}: {}",
            describe_step_status(step.status),
            step.id,
            step.summary
        );
        println!("      {}", step.detail);
    }
}

fn print_alerts(report: &AlertReport) {
    println!(
        "SyncSteward Alerts: {}",
        if report.alerts.is_empty() {
            "CLEAR"
        } else {
            "ACTIVE"
        }
    );
    println!("Config source: {}", report.config_source);
    println!(
        "Preflight ready: {}",
        if report.preflight_ready { "yes" } else { "no" }
    );
    println!(
        "Stale success threshold: {} hours",
        report.stale_success_after_hours
    );
    if report.alerts.is_empty() {
        println!("No active alerts.");
        return;
    }
    for alert in &report.alerts {
        println!(
            "- [{}] {}",
            describe_alert_severity(alert.severity),
            alert.summary
        );
        if let Some(target_name) = &alert.target_name {
            println!("  target: {}", target_name);
        }
        println!("  {}", alert.detail);
    }
}

fn print_notify_alerts(report: &NotifyAlertsReport) {
    println!("SyncSteward Notify Alerts: {:?}", report.outcome);
    println!("{}", report.summary);
    println!("Dry run: {}", if report.dry_run { "yes" } else { "no" });
    println!("Alert count: {}", report.alerts.len());
    for step in &report.steps {
        println!(
            "[{}] {}: {}",
            describe_step_status(step.status),
            step.id,
            step.summary
        );
        println!("  {}", step.detail);
    }
}

fn print_acknowledged_log(report: &LogAcknowledgeReport) {
    println!("SyncSteward Acknowledge Latest Log: {:?}", report.outcome);
    println!("{}", report.summary);
    println!("State path: {}", report.state_path.display());
    if let Some(latest) = &report.latest_log {
        println!("Latest log: {}", latest.path.display());
    }
    if let Some(acknowledged) = &report.acknowledged_log {
        println!(
            "Acknowledged: {} ({} errors, {} out-of-sync, {} warnings)",
            acknowledged.path.display(),
            acknowledged.error_count,
            acknowledged.out_of_sync_count,
            acknowledged.warning_count
        );
    }
}

fn print_config_scaffold(report: &ConfigScaffoldReport) {
    println!("SyncSteward Config Scaffold: {:?}", report.outcome);
    println!("{}", report.summary);
    println!("Path: {}", report.path.display());
    println!(
        "Policies: {} folder overrides, {} file-class defaults",
        report.folder_policy_count, report.file_class_policy_count
    );
}

fn print_path_samples(title: &str, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    println!("{}:", title);
    for path in paths {
        println!("  {}", path.display());
    }
}

fn describe_policy_mode(mode: PolicyMode) -> &'static str {
    match mode {
        PolicyMode::TwoWayCurated => "two-way curated",
        PolicyMode::BackupOnly => "backup only",
        PolicyMode::Excluded => "excluded",
        PolicyMode::Hold => "hold",
    }
}

fn describe_legacy_mode(mode: syncsteward_core::LegacySyncMode) -> &'static str {
    match mode {
        syncsteward_core::LegacySyncMode::Bisync => "legacy bisync",
        syncsteward_core::LegacySyncMode::BackupOneWay => "legacy backup",
    }
}

fn describe_step_status(status: ActionStepStatus) -> &'static str {
    match status {
        ActionStepStatus::Applied => "APPLIED",
        ActionStepStatus::Skipped => "SKIPPED",
        ActionStepStatus::Blocked => "BLOCKED",
        ActionStepStatus::Failed => "FAILED",
    }
}

fn describe_alert_severity(severity: AlertSeverity) -> &'static str {
    match severity {
        AlertSeverity::Info => "INFO",
        AlertSeverity::Warn => "WARN",
        AlertSeverity::Critical => "CRITICAL",
    }
}

fn control_exit_code(report: &ControlReport) -> i32 {
    match report.outcome {
        ActionOutcome::Success | ActionOutcome::NoOp => 0,
        ActionOutcome::Blocked => 2,
        ActionOutcome::Failed => 1,
    }
}

fn action_exit_code(outcome: ActionOutcome) -> i32 {
    match outcome {
        ActionOutcome::Success | ActionOutcome::NoOp => 0,
        ActionOutcome::Blocked => 2,
        ActionOutcome::Failed => 1,
    }
}

fn fatal_error(message: &str) -> i32 {
    eprintln!("{message}");
    1
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<(), serde_json::Error> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
