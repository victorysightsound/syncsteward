use anyhow::{Context, Result, bail};
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub launch_agent_label: String,
    pub launch_agent_path: PathBuf,
    pub sync_script_path: PathBuf,
    pub rclone_log_dir: PathBuf,
    pub ssh_key_path: PathBuf,
    pub remote: RemoteConfig,
    pub scan: ScanConfig,
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
        }
    }
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

fn read_config(path: &Path) -> Result<AppConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read config at {}", path.display()))?;
    let parsed: AppConfig =
        toml::from_str(&raw).with_context(|| format!("parse config at {}", path.display()))?;
    normalize_config(parsed)
}

fn normalize_config(mut config: AppConfig) -> Result<AppConfig> {
    config.launch_agent_path = expand_path(&config.launch_agent_path);
    config.sync_script_path = expand_path(&config.sync_script_path);
    config.rclone_log_dir = expand_path(&config.rclone_log_dir);
    config.ssh_key_path = expand_path(&config.ssh_key_path);
    config.scan.roots = config
        .scan
        .roots
        .iter()
        .map(|path| expand_path(path))
        .collect();

    if config.launch_agent_label.trim().is_empty() {
        bail!("launch_agent_label must not be empty");
    }
    if config.remote.ssh_user.trim().is_empty() {
        bail!("remote.ssh_user must not be empty");
    }
    if config.remote.preferred_hosts.is_empty() {
        bail!("remote.preferred_hosts must contain at least one host");
    }
    if config.remote.onedrive_service.trim().is_empty() {
        bail!("remote.onedrive_service must not be empty");
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

#[cfg(test)]
mod tests {
    use super::expand_path;
    use std::path::{Path, PathBuf};

    #[test]
    fn expands_tilde_prefix() {
        let expanded = expand_path(Path::new("~/Library"));
        assert!(expanded.ends_with(PathBuf::from("Library")));
        assert_ne!(expanded, PathBuf::from("~/Library"));
    }
}
