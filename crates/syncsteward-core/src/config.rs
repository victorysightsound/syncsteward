use anyhow::{Context, Result, bail};
use dirs::home_dir;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub launch_agent_label: String,
    pub launch_agent_path: PathBuf,
    pub sync_script_path: PathBuf,
    pub rclone_log_dir: PathBuf,
    pub ssh_key_path: PathBuf,
    #[serde(default = "default_sync_filter_path")]
    pub sync_filter_path: PathBuf,
    #[serde(default = "default_memloft_filter_path")]
    pub memloft_filter_path: PathBuf,
    #[serde(default = "default_legacy_lock_path")]
    pub legacy_lock_path: PathBuf,
    #[serde(default = "default_audit_log_path")]
    pub audit_log_path: PathBuf,
    #[serde(default = "default_state_path")]
    pub state_path: PathBuf,
    pub remote: RemoteConfig,
    pub scan: ScanConfig,
    #[serde(default)]
    pub managed_targets: Vec<ManagedTarget>,
    #[serde(default)]
    pub alerts: AlertConfig,
    #[serde(default)]
    pub runner: RunnerConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub ssh_user: String,
    pub preferred_hosts: Vec<String>,
    pub onedrive_service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanConfig {
    pub roots: Vec<PathBuf>,
    pub max_examples: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ManagedTarget {
    #[serde(default)]
    pub target_id: Option<String>,
    pub name: String,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub mode: PolicyMode,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AlertConfig {
    #[serde(default = "default_stale_success_after_hours")]
    pub stale_success_after_hours: u64,
    #[serde(default = "default_enable_macos_notifications")]
    pub enable_macos_notifications: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunnerConfig {
    #[serde(default)]
    pub approved_targets: Vec<String>,
    #[serde(default = "default_cycle_interval_minutes")]
    pub cycle_interval_minutes: u64,
    #[serde(default = "default_notify_after_cycle")]
    pub notify_after_cycle: bool,
    #[serde(default = "default_notify_after_tick")]
    pub notify_after_tick: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PolicyConfig {
    #[serde(default)]
    pub folders: Vec<FolderPolicy>,
    #[serde(default = "default_file_class_policies")]
    pub file_classes: Vec<FileClassPolicy>,
    #[serde(default = "default_target_exclusions")]
    pub target_exclusions: Vec<TargetExclusion>,
    #[serde(default = "default_target_snapshots")]
    pub target_snapshots: Vec<TargetSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FolderPolicy {
    pub path: PathBuf,
    pub mode: PolicyMode,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FileClassPolicy {
    pub class: FileClass,
    pub mode: PolicyMode,
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetExclusion {
    pub target: String,
    pub patterns: Vec<String>,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TargetSnapshot {
    pub target: String,
    pub sqlite_paths: Vec<PathBuf>,
    #[serde(default)]
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyMode {
    TwoWayCurated,
    BackupOnly,
    Excluded,
    Hold,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    Database,
    SqliteWal,
    SqliteShm,
    ConflictArtifact,
    SafeBackupArtifact,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            folders: Vec::new(),
            file_classes: default_file_class_policies(),
            target_exclusions: default_target_exclusions(),
            target_snapshots: default_target_snapshots(),
        }
    }
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            stale_success_after_hours: default_stale_success_after_hours(),
            enable_macos_notifications: default_enable_macos_notifications(),
        }
    }
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            approved_targets: Vec::new(),
            cycle_interval_minutes: default_cycle_interval_minutes(),
            notify_after_cycle: default_notify_after_cycle(),
            notify_after_tick: default_notify_after_tick(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub enum ConfigSource {
    Explicit(PathBuf),
    DefaultFile(PathBuf),
    BuiltInDefaults,
}

impl ConfigSource {
    pub fn description(&self) -> String {
        match self {
            Self::Explicit(path) => format!("explicit config {}", path.display()),
            Self::DefaultFile(path) => format!("default config {}", path.display()),
            Self::BuiltInDefaults => "built-in defaults".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: AppConfig,
    pub source: ConfigSource,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            launch_agent_label: "com.cloud-sync".to_string(),
            launch_agent_path: PathBuf::from("~/Library/LaunchAgents/com.cloud-sync.plist"),
            sync_script_path: PathBuf::from("~/bin/cloud-sync.sh"),
            rclone_log_dir: PathBuf::from("~/.config/rclone/logs"),
            ssh_key_path: PathBuf::from("~/.ssh/id_ed25519"),
            sync_filter_path: default_sync_filter_path(),
            memloft_filter_path: default_memloft_filter_path(),
            legacy_lock_path: default_legacy_lock_path(),
            audit_log_path: default_audit_log_path(),
            state_path: default_state_path(),
            remote: RemoteConfig {
                ssh_user: "john".to_string(),
                preferred_hosts: vec!["192.168.77.135".to_string(), "192.168.195.155".to_string()],
                onedrive_service: "onedrive@john.service".to_string(),
            },
            scan: ScanConfig {
                roots: vec![
                    PathBuf::from("~/Ministry"),
                    PathBuf::from("~/Books"),
                    PathBuf::from("~/Desktop"),
                    PathBuf::from("~/Documents"),
                ],
                max_examples: 20,
            },
            managed_targets: Vec::new(),
            alerts: AlertConfig::default(),
            runner: RunnerConfig::default(),
            policy: PolicyConfig::default(),
        }
    }
}

fn default_audit_log_path() -> PathBuf {
    PathBuf::from("~/.local/state/syncsteward/audit.jsonl")
}

fn default_state_path() -> PathBuf {
    PathBuf::from("~/.local/state/syncsteward/state.json")
}

fn default_sync_filter_path() -> PathBuf {
    PathBuf::from("~/.config/rclone/sync-filters.txt")
}

fn default_memloft_filter_path() -> PathBuf {
    PathBuf::from("~/.config/rclone/sync-filters-memloft.txt")
}

fn default_legacy_lock_path() -> PathBuf {
    PathBuf::from("/tmp/cloud-sync.lock")
}

fn default_stale_success_after_hours() -> u64 {
    24
}

fn default_enable_macos_notifications() -> bool {
    true
}

fn default_notify_after_cycle() -> bool {
    true
}

fn default_cycle_interval_minutes() -> u64 {
    60
}

fn default_notify_after_tick() -> bool {
    true
}

fn default_file_class_policies() -> Vec<FileClassPolicy> {
    vec![
        FileClassPolicy {
            class: FileClass::Database,
            mode: PolicyMode::BackupOnly,
            patterns: vec![
                "*.db".to_string(),
                "*.sqlite".to_string(),
                "*.sqlite3".to_string(),
            ],
        },
        FileClassPolicy {
            class: FileClass::SqliteWal,
            mode: PolicyMode::BackupOnly,
            patterns: vec![
                "*.db-wal".to_string(),
                "*.sqlite-wal".to_string(),
                "*.sqlite3-wal".to_string(),
            ],
        },
        FileClassPolicy {
            class: FileClass::SqliteShm,
            mode: PolicyMode::BackupOnly,
            patterns: vec![
                "*.db-shm".to_string(),
                "*.sqlite-shm".to_string(),
                "*.sqlite3-shm".to_string(),
            ],
        },
        FileClassPolicy {
            class: FileClass::ConflictArtifact,
            mode: PolicyMode::Hold,
            patterns: vec!["*.conflict*".to_string()],
        },
        FileClassPolicy {
            class: FileClass::SafeBackupArtifact,
            mode: PolicyMode::Hold,
            patterns: vec!["*victorystore-safeBackup*".to_string()],
        },
    ]
}

fn default_target_exclusions() -> Vec<TargetExclusion> {
    vec![
        TargetExclusion {
            target: "Pictures".to_string(),
            patterns: vec![
                "Photos Library.photoslibrary/".to_string(),
                "Photos Library.photoslibrary/**".to_string(),
            ],
            rationale: Some(
                "Protect the native Photos bundle and sync only ordinary folders/files."
                    .to_string(),
            ),
        },
        TargetExclusion {
            target: "Music".to_string(),
            patterns: vec![
                "Music Library.musiclibrary/".to_string(),
                "Music Library.musiclibrary/**".to_string(),
            ],
            rationale: Some(
                "Protect the native Music bundle and sync only ordinary folders/files.".to_string(),
            ),
        },
    ]
}

fn default_target_snapshots() -> Vec<TargetSnapshot> {
    vec![TargetSnapshot {
        target: ".memloft".to_string(),
        sqlite_paths: vec![
            PathBuf::from("memloft.db"),
            PathBuf::from("payroll.db"),
            PathBuf::from("vault.db"),
        ],
        rationale: Some(
            "Runtime SQLite files should be uploaded from sqlite3 backup snapshots instead of the live WAL-backed files."
                .to_string(),
        ),
    }]
}

pub fn load_config(config_path: Option<&Path>) -> Result<LoadedConfig> {
    if let Some(path) = config_path {
        let explicit_path = expand_path(path);
        return Ok(LoadedConfig {
            config: read_config(&explicit_path)?,
            source: ConfigSource::Explicit(explicit_path),
        });
    }

    let default_path = default_config_path();
    if default_path.exists() {
        return Ok(LoadedConfig {
            config: read_config(&default_path)?,
            source: ConfigSource::DefaultFile(default_path),
        });
    }

    Ok(LoadedConfig {
        config: normalize_config(AppConfig::default())?,
        source: ConfigSource::BuiltInDefaults,
    })
}

pub fn default_config_path() -> PathBuf {
    expand_path(Path::new("~/.config/syncsteward/config.toml"))
}

pub fn normalize_app_config(config: AppConfig) -> Result<AppConfig> {
    normalize_config(config)
}

fn read_config(path: &Path) -> Result<AppConfig> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("read config at {}", path.display()))?;
    let parsed: AppConfig =
        toml::from_str(&raw).with_context(|| format!("parse config at {}", path.display()))?;
    normalize_config(parsed)
}

fn normalize_config(mut config: AppConfig) -> Result<AppConfig> {
    config.launch_agent_path = expand_path(&config.launch_agent_path);
    config.sync_script_path = expand_path(&config.sync_script_path);
    config.rclone_log_dir = expand_path(&config.rclone_log_dir);
    config.ssh_key_path = expand_path(&config.ssh_key_path);
    config.sync_filter_path = expand_path(&config.sync_filter_path);
    config.memloft_filter_path = expand_path(&config.memloft_filter_path);
    config.legacy_lock_path = expand_path(&config.legacy_lock_path);
    config.audit_log_path = expand_path(&config.audit_log_path);
    config.state_path = expand_path(&config.state_path);
    config.scan.roots = config
        .scan
        .roots
        .iter()
        .map(|path| expand_path(path))
        .collect();
    config.runner.approved_targets = config
        .runner
        .approved_targets
        .iter()
        .map(|target| target.trim().to_string())
        .filter(|target| !target.is_empty())
        .collect();
    config.managed_targets = config
        .managed_targets
        .iter()
        .map(|target| ManagedTarget {
            target_id: normalize_optional_value(target.target_id.as_deref()),
            name: target.name.clone(),
            local_path: expand_path(&target.local_path),
            remote_path: target.remote_path.clone(),
            mode: target.mode,
            rationale: target.rationale.clone(),
        })
        .collect();
    config.policy.folders = config
        .policy
        .folders
        .iter()
        .map(|policy| FolderPolicy {
            path: expand_path(&policy.path),
            mode: policy.mode,
            label: policy.label.clone(),
        })
        .collect();
    config.policy.target_snapshots = config
        .policy
        .target_snapshots
        .iter()
        .map(|policy| TargetSnapshot {
            target: policy.target.clone(),
            sqlite_paths: policy
                .sqlite_paths
                .iter()
                .map(|path| expand_path(path))
                .collect(),
            rationale: policy.rationale.clone(),
        })
        .collect();

    if config.launch_agent_label.trim().is_empty() {
        bail!("launch_agent_label must not be empty");
    }
    if config.remote.ssh_user.trim().is_empty() {
        bail!("remote.ssh_user must not be empty");
    }
    for target in &config.managed_targets {
        if let Some(target_id) = &target.target_id {
            if target_id.trim().is_empty() {
                bail!("managed_targets.target_id must not be empty when provided");
            }
        }
        if target.name.trim().is_empty() {
            bail!("managed_targets.name must not be empty");
        }
        if target.remote_path.trim().is_empty() {
            bail!("managed_targets.remote_path must not be empty");
        }
    }
    let mut managed_target_ids = BTreeSet::new();
    for target in &config.managed_targets {
        if let Some(target_id) = &target.target_id {
            if !managed_target_ids.insert(target_id.clone()) {
                bail!("managed_targets.target_id must be unique: {target_id}");
            }
        }
    }
    let mut managed_target_names = BTreeSet::new();
    for target in &config.managed_targets {
        if !managed_target_names.insert(target.name.clone()) {
            bail!("managed_targets.name must be unique: {}", target.name);
        }
    }
    let mut managed_local_paths = BTreeSet::new();
    for target in &config.managed_targets {
        if !managed_local_paths.insert(target.local_path.clone()) {
            bail!(
                "managed_targets.local_path must be unique: {}",
                target.local_path.display()
            );
        }
    }
    let mut managed_remote_paths = BTreeSet::new();
    for target in &config.managed_targets {
        if !managed_remote_paths.insert(target.remote_path.clone()) {
            bail!(
                "managed_targets.remote_path must be unique: {}",
                target.remote_path
            );
        }
    }
    if config.remote.preferred_hosts.is_empty() {
        bail!("remote.preferred_hosts must contain at least one host");
    }
    if config.remote.onedrive_service.trim().is_empty() {
        bail!("remote.onedrive_service must not be empty");
    }
    if config.alerts.stale_success_after_hours == 0 {
        bail!("alerts.stale_success_after_hours must be greater than zero");
    }
    let mut approved_targets = BTreeSet::new();
    for selector in &config.runner.approved_targets {
        if !approved_targets.insert(selector.clone()) {
            bail!("runner.approved_targets must be unique: {selector}");
        }
    }
    if config.runner.cycle_interval_minutes == 0 {
        bail!("runner.cycle_interval_minutes must be greater than zero");
    }
    if config.scan.max_examples == 0 {
        bail!("scan.max_examples must be greater than zero");
    }

    Ok(config)
}

pub fn expand_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("/"));
    }
    if let Some(suffix) = raw.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(suffix);
        }
    }
    path.to_path_buf()
}

fn normalize_optional_value(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        FileClass, ManagedTarget, PolicyConfig, PolicyMode, expand_path, normalize_optional_value,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn expands_tilde_prefix() {
        let expanded = expand_path(Path::new("~/Library"));
        assert!(expanded.ends_with(PathBuf::from("Library")));
        assert_ne!(expanded, PathBuf::from("~/Library"));
    }

    #[test]
    fn default_policy_protects_database_files() {
        let policy = PolicyConfig::default();

        let database = policy
            .file_classes
            .iter()
            .find(|entry| entry.class == FileClass::Database)
            .expect("database policy");
        let wal = policy
            .file_classes
            .iter()
            .find(|entry| entry.class == FileClass::SqliteWal)
            .expect("sqlite wal policy");
        let shm = policy
            .file_classes
            .iter()
            .find(|entry| entry.class == FileClass::SqliteShm)
            .expect("sqlite shm policy");

        assert_eq!(database.mode, PolicyMode::BackupOnly);
        assert_eq!(wal.mode, PolicyMode::BackupOnly);
        assert_eq!(shm.mode, PolicyMode::BackupOnly);
    }

    #[test]
    fn default_policy_protects_native_apple_libraries() {
        let policy = PolicyConfig::default();

        let pictures = policy
            .target_exclusions
            .iter()
            .find(|entry| entry.target == "Pictures")
            .expect("pictures exclusions");
        let music = policy
            .target_exclusions
            .iter()
            .find(|entry| entry.target == "Music")
            .expect("music exclusions");

        assert!(
            pictures
                .patterns
                .iter()
                .any(|pattern| pattern == "Photos Library.photoslibrary/**")
        );
        assert!(
            music
                .patterns
                .iter()
                .any(|pattern| pattern == "Music Library.musiclibrary/**")
        );
    }

    #[test]
    fn default_policy_includes_memloft_snapshot_files() {
        let policy = PolicyConfig::default();
        let memloft = policy
            .target_snapshots
            .iter()
            .find(|entry| entry.target == ".memloft")
            .expect("memloft snapshot policy");

        assert!(
            memloft
                .sqlite_paths
                .iter()
                .any(|path| path == Path::new("memloft.db"))
        );
        assert!(
            memloft
                .sqlite_paths
                .iter()
                .any(|path| path == Path::new("payroll.db"))
        );
        assert!(
            memloft
                .sqlite_paths
                .iter()
                .any(|path| path == Path::new("vault.db"))
        );
    }

    #[test]
    fn normalize_optional_value_trims_and_drops_blank_values() {
        assert_eq!(
            normalize_optional_value(Some(" abc ")),
            Some("abc".to_string())
        );
        assert_eq!(normalize_optional_value(Some("   ")), None);
        assert_eq!(normalize_optional_value(None), None);
    }

    #[test]
    fn managed_target_can_carry_an_optional_target_id() {
        let target = ManagedTarget {
            target_id: Some("target-123".to_string()),
            name: "Notes/Personal".to_string(),
            local_path: PathBuf::from("/tmp/notes"),
            remote_path: "OneDrive/Notes/Personal".to_string(),
            mode: PolicyMode::BackupOnly,
            rationale: None,
        };

        assert_eq!(target.target_id.as_deref(), Some("target-123"));
    }
}
