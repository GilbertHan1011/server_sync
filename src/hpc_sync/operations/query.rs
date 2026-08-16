use std::fs;

use super::super::config::AppConfig;
use super::super::error::{AppError, AppResult};
use super::super::records::{HistoryRecord, RunRecord, StatusRecord};
use super::super::state::read_json;

pub fn status_job(config: &AppConfig, job_id: &str) -> AppResult<StatusRecord> {
    let _job = config.job(job_id)?;
    let latest_path = config
        .state_dir
        .join("latest")
        .join(format!("{job_id}.json"));
    let latest = if latest_path.is_file() {
        Some(read_json(&latest_path)?)
    } else {
        None
    };
    Ok(StatusRecord {
        schema_version: 1,
        operation: "status",
        status: if latest.is_some() {
            "available"
        } else {
            "never_run"
        },
        job_id: job_id.to_owned(),
        latest,
        next_actions: vec![format!("hpc-sync plan {job_id} --json")],
    })
}

pub fn history_job(config: &AppConfig, job_id: &str, limit: usize) -> AppResult<HistoryRecord> {
    let _job = config.job(job_id)?;
    let root = config.state_dir.join("runs").join(job_id);
    let mut paths = if root.is_dir() {
        fs::read_dir(&root)
            .map_err(|error| {
                AppError::new(
                    "state_unreadable",
                    error.to_string(),
                    false,
                    "Check state directory permissions.",
                )
            })?
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("result.json"))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    paths.sort_by(|left, right| right.cmp(left));
    let runs = paths
        .into_iter()
        .take(limit)
        .map(|path| read_json::<RunRecord>(&path))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(HistoryRecord {
        schema_version: 1,
        operation: "history",
        status: "available",
        job_id: job_id.to_owned(),
        count: runs.len(),
        runs,
        next_actions: vec![format!("hpc-sync status {job_id} --json")],
    })
}
