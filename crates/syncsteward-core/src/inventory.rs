use crate::config::{AppConfig, PolicyMode, load_config};
use crate::model::{LegacySyncMode, SyncTargetInventoryReport, SyncTargetRecord};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

pub fn targets(config_path: Option<&Path>) -> Result<SyncTargetInventoryReport> {
    let loaded = load_config(config_path)?;
    let report = build_target_inventory(&loaded.config, loaded.source.description())?;
    Ok(report)
}

pub(crate) fn build_target_inventory(
    config: &AppConfig,
    config_source: String,
) -> Result<SyncTargetInventoryReport> {
    let script_path = config.sync_script_path.clone();
    let contents = fs::read_to_string(&script_path)
        .with_context(|| format!("read sync script at {}", script_path.display()))?;

    let bisync_folders = parse_array(&contents, "BISYNC_FOLDERS")
        .with_context(|| format!("parse BISYNC_FOLDERS in {}", script_path.display()))?;
    let backup_folders = parse_array(&contents, "BACKUP_FOLDERS")
        .with_context(|| format!("parse BACKUP_FOLDERS in {}", script_path.display()))?;

    let mut targets = Vec::new();

    for folder in bisync_folders {
        let local_path = PathBuf::from(format!(
            "{}/{}",
            std::env::var("HOME").unwrap_or_default(),
            folder
        ));
        let (recommended_mode, rationale) =
            recommend_policy(&folder, LegacySyncMode::Bisync, &local_path);
        targets.push(SyncTargetRecord {
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
        let local_path = PathBuf::from(format!(
            "{}/{}",
            std::env::var("HOME").unwrap_or_default(),
            local_name
        ));
        let (recommended_mode, rationale) =
            recommend_policy(local_name, LegacySyncMode::BackupOneWay, &local_path);
        targets.push(SyncTargetRecord {
            name: local_name.to_string(),
            local_path: local_path.clone(),
            remote_path: format!("OneDrive/{}", remote_name),
            legacy_mode: LegacySyncMode::BackupOneWay,
            recommended_mode,
            configured_mode: find_configured_mode(config, &local_path),
            rationale: rationale.to_string(),
        });
    }

    targets.sort_by(|a, b| a.local_path.cmp(&b.local_path));

    Ok(SyncTargetInventoryReport {
        config_source,
        script_path,
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
        ".memloft" | "_hub" => (
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
    use super::parse_array;

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
    "_hub:_hub"
)
"#;

        let bisync = parse_array(script, "BISYNC_FOLDERS").expect("bisync");
        let backup = parse_array(script, "BACKUP_FOLDERS").expect("backup");

        assert_eq!(bisync, vec!["Notes", "Desktop", "Books"]);
        assert_eq!(backup, vec![".memloft:.memloft", "_hub:_hub"]);
    }
}
