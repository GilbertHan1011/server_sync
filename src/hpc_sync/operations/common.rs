use std::fs;
use std::path::PathBuf;

use chrono::{SecondsFormat, Utc};

use super::super::config::AppConfig;
use super::super::error::{AppError, AppResult};
use super::super::process::stable_hash;
use super::super::records::{RunRecord, RunStatus};
use super::super::state::read_json;

pub fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn new_run_id() -> String {
    let now = Utc::now();
    let seed = format!(
        "{}:{}",
        now.timestamp_nanos_opt().map_or(0, |value| value),
        std::process::id()
    );
    let suffix: String = stable_hash(&[&seed]).chars().take(8).collect();
    format!("{}-{suffix}", now.format("%Y%m%dT%H%M%SZ"))
}

pub fn run_dir(config: &AppConfig, job_id: &str, run_id: &str) -> PathBuf {
    config.state_dir.join("runs").join(job_id).join(run_id)
}

pub fn config_hash(config: &AppConfig, job_id: &str) -> AppResult<String> {
    let job = config.job(job_id)?;
    let payload = serde_json::to_string(job).map_err(|error| {
        AppError::new(
            "config_serialize_failed",
            format!("cannot fingerprint job {job_id}: {error}"),
            false,
            "Report this hpc-sync serialization defect.",
        )
    })?;
    Ok(stable_hash(&["1", job_id, &payload]))
}

pub fn latest_successful_run_id(config: &AppConfig, job_id: &str) -> AppResult<Option<String>> {
    let root = config.state_dir.join("runs").join(job_id);
    if !root.is_dir() {
        return Ok(None);
    }
    let entries = fs::read_dir(&root).map_err(|error| {
        AppError::new(
            "state_unreadable",
            format!("cannot list {}: {error}", root.display()),
            false,
            "Check state directory permissions.",
        )
    })?;
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("result.json"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.cmp(left));
    for path in paths {
        let record: RunRecord = read_json(&path)?;
        if record.status == RunStatus::Success {
            return Ok(Some(record.run_id));
        }
    }
    Ok(None)
}
