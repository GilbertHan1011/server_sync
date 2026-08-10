use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::{AppError, AppResult};

fn default_port() -> u16 {
    22
}

fn default_connect_timeout() -> u64 {
    15
}

fn default_verify() -> bool {
    true
}

fn default_runtime() -> u64 {
    21_600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SshConfig {
    pub host: String,
    pub user: String,
    pub identity_file: PathBuf,
    pub known_hosts_file: PathBuf,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobConfig {
    #[serde(default = "default_profile")]
    pub profile: String,
    pub source: PathBuf,
    pub destination_root: PathBuf,
    pub ssh: SshConfig,
    #[serde(default)]
    pub source_complete_marker: Option<PathBuf>,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub preserve_hard_links: bool,
    #[serde(default)]
    pub preserve_acls: bool,
    #[serde(default)]
    pub preserve_xattrs: bool,
    #[serde(default)]
    pub bootstrap_from_destination_root: bool,
    #[serde(default = "default_verify")]
    pub verify_after_run: bool,
    #[serde(default = "default_runtime")]
    pub max_runtime_seconds: u64,
}

fn default_profile() -> String {
    "backup".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub version: u8,
    pub state_dir: PathBuf,
    pub jobs: BTreeMap<String, JobConfig>,
}

impl AppConfig {
    pub fn job(&self, job_id: &str) -> AppResult<&JobConfig> {
        self.jobs.get(job_id).ok_or_else(|| {
            AppError::new(
                "job_not_found",
                format!("job {job_id:?} is not defined"),
                false,
                "Select a job id from the configured jobs table.",
            )
        })
    }

    fn validate(&self) -> AppResult<()> {
        if self.version != 1 {
            return Err(config_error("version must be 1"));
        }
        validate_absolute_root(&self.state_dir, "state_dir")?;
        if self.jobs.is_empty() {
            return Err(config_error("at least one job is required"));
        }
        for (job_id, job) in &self.jobs {
            if !is_safe_name(job_id) {
                return Err(config_error(format!("invalid job id {job_id:?}")));
            }
            job.validate()?;
        }
        Ok(())
    }
}

impl JobConfig {
    fn validate(&self) -> AppResult<()> {
        if self.profile != "backup" {
            return Err(config_error("only profile = \"backup\" is supported"));
        }
        validate_absolute_root(&self.source, "source")?;
        validate_absolute_root(&self.destination_root, "destination_root")?;
        validate_absolute_file(&self.ssh.identity_file, "identity_file")?;
        validate_absolute_file(&self.ssh.known_hosts_file, "known_hosts_file")?;
        if !is_safe_name(&self.ssh.host) || !is_safe_name(&self.ssh.user) {
            return Err(config_error(
                "SSH host and user contain unsupported characters",
            ));
        }
        if self.max_runtime_seconds < 30 || self.max_runtime_seconds > 604_800 {
            return Err(config_error(
                "max_runtime_seconds must be between 30 and 604800",
            ));
        }
        if let Some(marker) = &self.source_complete_marker {
            let invalid = marker.is_absolute()
                || marker
                    .components()
                    .any(|component| matches!(component, Component::ParentDir));
            if invalid {
                return Err(config_error(
                    "source_complete_marker must stay inside the source directory",
                ));
            }
        }
        if self
            .excludes
            .iter()
            .any(|item| item.is_empty() || item.contains(['\n', '\r']))
        {
            return Err(config_error(
                "exclude patterns must be non-empty single lines",
            ));
        }
        Ok(())
    }
}

pub fn load_config(path: &Path) -> AppResult<AppConfig> {
    let content = fs::read_to_string(path).map_err(|error| {
        AppError::new(
            "config_unreadable",
            format!("cannot read config {}: {error}", path.display()),
            false,
            "Create the config file and make it readable by the current user.",
        )
    })?;
    let config: AppConfig = toml::from_str(&content).map_err(|error| {
        AppError::new(
            "config_invalid",
            format!("invalid config {}: {error}", path.display()),
            false,
            "Fix the reported TOML field; unknown keys are rejected.",
        )
    })?;
    config.validate()?;
    Ok(config)
}

pub fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let Some(suffix) = text.strip_prefix("~/") else {
        return path.to_path_buf();
    };
    std::env::var_os("HOME").map_or_else(
        || path.to_path_buf(),
        |home| PathBuf::from(home).join(suffix),
    )
}

fn config_error(message: impl Into<String>) -> AppError {
    AppError::new(
        "config_invalid",
        message,
        false,
        "Correct the closed-schema TOML configuration.",
    )
}

fn validate_absolute_root(path: &Path, field: &str) -> AppResult<()> {
    if !path.is_absolute() || path == Path::new("/") {
        return Err(config_error(format!(
            "{field} must be an absolute non-root path"
        )));
    }
    validate_text_path(path, field)
}

fn validate_absolute_file(path: &Path, field: &str) -> AppResult<()> {
    if !path.is_absolute() {
        return Err(config_error(format!("{field} must be an absolute path")));
    }
    validate_text_path(path, field)
}

fn validate_text_path(path: &Path, field: &str) -> AppResult<()> {
    let Some(text) = path.to_str() else {
        return Err(config_error(format!("{field} must be valid UTF-8")));
    };
    if text.contains(['\n', '\r']) {
        return Err(config_error(format!("{field} contains a line break")));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(config_error(format!(
            "{field} must not contain parent-directory components"
        )));
    }
    Ok(())
}

fn is_safe_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_rejects_unknown_keys() {
        let parsed = toml::from_str::<AppConfig>(
            r#"
version = 1
state_dir = "/tmp/state"
[jobs.demo]
source = "/tmp/source"
destination_root = "/backup"
unknown = true
[jobs.demo.ssh]
host = "backup.local"
user = "sync"
identity_file = "/tmp/key"
known_hosts_file = "/tmp/hosts"
"#,
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn config_rejects_parent_components_in_roots() {
        let parsed = toml::from_str::<AppConfig>(
            r#"
version = 1
state_dir = "/tmp/state/../escape"
[jobs.demo]
source = "/tmp/source"
destination_root = "/backup"
[jobs.demo.ssh]
host = "backup.local"
user = "sync"
identity_file = "/tmp/key"
known_hosts_file = "/tmp/hosts"
"#,
        )
        .expect("parse closed schema");
        assert!(parsed.validate().is_err());
    }
}
