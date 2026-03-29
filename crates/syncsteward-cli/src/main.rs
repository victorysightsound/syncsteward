use clap::{Parser, Subcommand};
use std::path::PathBuf;
use syncsteward_core::{CheckStatus, PreflightReport, StatusReport, preflight, status};

#[derive(Debug, Parser)]
#[command(name = "syncsteward", about = "Safety-first sync health and preflight control")]
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

fn main() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Command::Status { json } => {
            let report = status(cli.config.as_deref()).map_err(|error| error.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
                );
            } else {
                print_status(&report);
            }
        }
        Command::Preflight { json } => {
            let report = preflight(cli.config.as_deref()).map_err(|error| error.to_string())?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
                );
            } else {
                print_preflight(&report);
            }
        }
        Command::Mcp {
            command: McpCommand::Stdio,
        } => {
            syncsteward_mcp::serve_stdio_blocking(cli.config)?;
        }
    }

    Ok(())
}

fn print_status(report: &StatusReport) {
    println!("SyncSteward Status");
    println!("Config source: {}", report.config_source);
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

fn print_path_samples(title: &str, paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    println!("{}:", title);
    for path in paths {
        println!("  {}", path.display());
    }
}
