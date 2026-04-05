use crate::config::{AppConfig, PolicyMode, current_home_dir, load_config};
use crate::model::{LegacySyncMode, SyncTargetInventoryReport, SyncTargetRecord};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

pub fn targets(config_path: Option<&Path>) -> Result<SyncTargetInventoryReport> {
    let loaded = load_config(config_path)?;
    let report = build_target_inventory(&loaded.config, loaded.source.description())?;
    Ok(report)
}

pub(crate) fn build_target_inventory(
    config: &AppConfig,
    config_source: String,
) -> Result<SyncTargetInventoryReport> {
    let home_dir = current_home_dir();
    build_target_inventory_with_home(config, config_source, &home_dir)
}

fn build_target_inventory_with_home(
    config: &AppConfig,
    config_source: String,
    home_dir: &Path,
) -> Result<SyncTargetInventoryReport> {
    let script_path = config.sync_script_path.clone();
    let mut targets = Vec::new();
    let legacy_inventory_available = script_path.exists();

    if legacy_inventory_available {
        let contents = fs::read_to_string(&script_path)
            .with_context(|| format!("read sync script at {}", script_path.display()))?;

        let bisync_folders = parse_array(&contents, "BISYNC_FOLDERS")
            .with_context(|| format!("parse BISYNC_FOLDERS in {}", script_path.display()))?;
        let backup_folders = parse_array(&contents, "BACKUP_FOLDERS")
            .with_context(|| format!("parse BACKUP_FOLDERS in {}", script_path.display()))?;

        for folder in bisync_folders {
            let local_path = home_dir.join(&folder);
            let (recommended_mode, rationale) =
                recommend_policy(&folder, LegacySyncMode::Bisync, &local_path);
            targets.push(SyncTargetRecord {
                target_id: None,
                name: folder.clone(),
                local_path: local_path.clone(),
                remote_path: format!("OneDrive/{}", folder),
                legacy_mode: LegacySyncMode::Bisync,
                recommended_mode,
                configured_mode: find_configured_mode(config, &local_path),
                rationale: rationale.to_string(),
            });
        }

        for mapping in backup_folders {
            let (local_name, remote_name) = mapping
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("invalid BACKUP_FOLDERS mapping: {mapping}"))?;
            let local_path = home_dir.join(local_name);
            let (recommended_mode, rationale) =
                recommend_policy(local_name, LegacySyncMode::BackupOneWay, &local_path);
            targets.push(SyncTargetRecord {
                target_id: None,
                name: local_name.to_string(),
                local_path: local_path.clone(),
                remote_path: format!("OneDrive/{}", remote_name),
                legacy_mode: LegacySyncMode::BackupOneWay,
                recommended_mode,
                configured_mode: find_configured_mode(config, &local_path),
                rationale: rationale.to_string(),
            });
        }
    } else if config.managed_targets.is_empty() {
        bail!(
            "sync script not found at {} and no managed targets are configured",
            script_path.display()
        );
    }

    for managed in &config.managed_targets {
        targets.push(SyncTargetRecord {
            target_id: managed.target_id.clone(),
            name: managed.name.clone(),
            local_path: managed.local_path.clone(),
            remote_path: managed.remote_path.clone(),
            legacy_mode: LegacySyncMode::Managed,
            recommended_mode: managed.mode,
            configured_mode: Some(managed.mode),
            rationale: managed.rationale.clone().unwrap_or_else(|| {
                "Managed target defined explicitly in SyncSteward config.".to_string()
            }),
        });
    }

    targets.sort_by(|a, b| a.local_path.cmp(&b.local_path));

    Ok(SyncTargetInventoryReport {
        config_source,
        script_path,
        legacy_inventory_available,
        targets,
    })
}

fn parse_array(contents: &str, array_name: &str) -> Result<Vec<String>> {
    let start_marker = format!("{array_name}=(");
    let start = contents
        .find(&start_marker)
        .ok_or_else(|| anyhow::anyhow!("missing {array_name} array"))?;
    let after_start = &contents[start + start_marker.len()..];
    let end = after_start
        .find("\n)")
        .ok_or_else(|| anyhow::anyhow!("missing closing ) for {array_name}"))?;
    let block = &after_start[..end];

    let mut entries = Vec::new();
    for raw_line in block.lines() {
        let cleaned = strip_shell_comment(raw_line);
        let line = cleaned.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let unquoted = line
            .strip_prefix('"')
            .and_then(|line| line.strip_suffix('"'))
            .or_else(|| {
                line.strip_prefix('\'')
                    .and_then(|line| line.strip_suffix('\''))
            })
            .unwrap_or(line);

        let value = unquoted
            .split_whitespace()
            .next()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("could not parse {array_name} entry from line: {line}")
            })?;

        entries.push(value.to_string());
    }

    if entries.is_empty() {
        bail!("{array_name} did not contain any entries");
    }

    Ok(entries)
}

fn strip_shell_comment(line: &str) -> String {
    let mut result = String::new();
    let mut in_single = false;
    let mut in_double = false;

    for ch in line.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                result.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                result.push(ch);
            }
            '#' if !in_single && !in_double => break,
            _ => result.push(ch),
        }
    }

    result
}

fn recommend_policy(
    name: &str,
    legacy_mode: LegacySyncMode,
    local_path: &Path,
) -> (PolicyMode, &'static str) {
    match name {
        ".memloft" => (
            PolicyMode::BackupOnly,
            "Runtime database and app-state folders should remain one-way backup only.",
        ),
        "Desktop" | "Documents" | "Notes" | "Personal" | "Ministry" | "Books" | "Business"
        | "Mac-Notes" => (
            PolicyMode::Hold,
            "Broad live workspaces need curated subfolders before they are safe to re-enable.",
        ),
        "Pictures" | "Music" | "Videos" => (
            PolicyMode::BackupOnly,
            "Library-style media collections are safer as backup-only until a narrower curation policy exists.",
        ),
        "Software" => (
            PolicyMode::Excluded,
            "Code, toolchains, and build trees need a dedicated sync workflow instead of blanket folder sync.",
        ),
        _ if matches!(legacy_mode, LegacySyncMode::BackupOneWay) => (
            PolicyMode::BackupOnly,
            "The legacy script already treated this target as one-way backup.",
        ),
        _ if local_path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| matches!(ext, "app" | "pkg")) =>
        {
            (
                PolicyMode::Excluded,
                "Bundle/package-style targets should not be managed by broad bidirectional sync.",
            )
        }
        _ => (
            PolicyMode::Hold,
            "Default to hold until this target is explicitly classified and validated.",
        ),
    }
}

fn find_configured_mode(config: &AppConfig, local_path: &Path) -> Option<PolicyMode> {
    config
        .policy
        .folders
        .iter()
        .find(|policy| policy.path == local_path)
        .map(|policy| policy.mode)
}

#[cfg(test)]
mod tests {
    use super::{build_target_inventory, build_target_inventory_with_home, parse_array};
    use crate::config::{AppConfig, ManagedTarget};
    use crate::model::LegacySyncMode;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn parses_shell_arrays_with_comments() {
        let script = r#"
BISYNC_FOLDERS=(
    "Notes"
    "Desktop"
    "Books"
)

BACKUP_FOLDERS=(
    ".memloft:.memloft"  # comment
)
"#;

        let bisync = parse_array(script, "BISYNC_FOLDERS").expect("bisync");
        let backup = parse_array(script, "BACKUP_FOLDERS").expect("backup");

        assert_eq!(bisync, vec!["Notes", "Desktop", "Books"]);
        assert_eq!(backup, vec![".memloft:.memloft"]);
    }

    #[test]
    fn falls_back_to_managed_targets_when_legacy_script_is_missing() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-inventory-test-{}",
            uuid::Uuid::now_v7()
        ));
        let state_path = temp_root.join("state.json");
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.sync_script_path = temp_root.join("missing-cloud-sync.sh");
        config.state_path = state_path;
        config.managed_targets = vec![ManagedTarget {
            target_id: Some("managed-1".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: PathBuf::from("/Users/example/Notes/Personal"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: crate::config::PolicyMode::BackupOnly,
            rationale: Some("managed target fallback".to_string()),
        }];

        let report = build_target_inventory(&config, "test config".to_string()).expect("inventory");
        assert!(!report.legacy_inventory_available);
        assert_eq!(report.targets.len(), 1);
        assert_eq!(report.targets[0].legacy_mode, LegacySyncMode::Managed);

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn errors_when_no_legacy_script_and_no_managed_targets_exist() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-inventory-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");

        let mut config = AppConfig::default();
        config.sync_script_path = temp_root.join("missing-cloud-sync.sh");
        config.managed_targets.clear();

        let error =
            build_target_inventory(&config, "test config".to_string()).expect_err("inventory");
        assert!(error.to_string().contains("sync script not found"));

        let _ = fs::remove_dir_all(temp_root);
    }

    #[test]
    fn legacy_inventory_uses_supplied_home_dir() {
        let temp_root = std::env::temp_dir().join(format!(
            "syncsteward-inventory-test-{}",
            uuid::Uuid::now_v7()
        ));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let script_path = temp_root.join("cloud-sync.sh");
        fs::write(
            &script_path,
            "BISYNC_FOLDERS=(\n  \"Books\"\n)\n\nBACKUP_FOLDERS=(\n  \".memloft:.memloft\"\n)\n",
        )
        .expect("write script");

        let mut config = AppConfig::default();
        config.sync_script_path = script_path;
        config.managed_targets.clear();

        let report = build_target_inventory_with_home(
            &config,
            "test config".to_string(),
            PathBuf::from("/Users/syncsteward").as_path(),
        )
        .expect("inventory");

        let books = report
            .targets
            .iter()
            .find(|target| target.name == "Books")
            .expect("books target");
        let memloft = report
            .targets
            .iter()
            .find(|target| target.name == ".memloft")
            .expect("memloft target");

        assert_eq!(books.local_path, PathBuf::from("/Users/syncsteward/Books"));
        assert_eq!(
            memloft.local_path,
            PathBuf::from("/Users/syncsteward/.memloft")
        );

        let _ = fs::remove_dir_all(temp_root);
    }
}
