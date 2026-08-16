use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::config::JobConfig;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Success,
    Failed,
    VerificationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifyStatus {
    Verified,
    Mismatch,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestRecord {
    pub schema_version: u8,
    pub operation: String,
    pub job_id: String,
    pub run_id: String,
    pub config_hash: String,
    pub job: JobConfig,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanRecord {
    pub schema_version: u8,
    pub operation: String,
    pub status: String,
    pub job_id: String,
    pub run_id: String,
    pub config_hash: String,
    pub plan_hash: String,
    pub previous_run_id: Option<String>,
    pub snapshot_path: String,
    pub started_at: String,
    pub finished_at: String,
    pub rsync_exit_code: i32,
    pub item_count: usize,
    pub run_dir: PathBuf,
    pub items_path: PathBuf,
    pub command: Vec<String>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u8,
    pub operation: String,
    pub status: RunStatus,
    pub job_id: String,
    pub run_id: String,
    pub config_hash: String,
    pub plan_hash: String,
    pub previous_run_id: Option<String>,
    pub snapshot_path: String,
    pub started_at: String,
    pub finished_at: String,
    pub rsync_exit_code: i32,
    pub classification: String,
    pub retryable: bool,
    pub verify_exit_code: Option<i32>,
    pub verify_item_count: Option<usize>,
    pub marker_exit_code: Option<i32>,
    pub run_dir: PathBuf,
    pub completed_marker: Option<PathBuf>,
    pub command: Vec<String>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyRecord {
    pub schema_version: u8,
    pub operation: String,
    pub status: VerifyStatus,
    pub job_id: String,
    pub run_id: String,
    pub snapshot_path: String,
    pub checksum: bool,
    pub started_at: String,
    pub finished_at: String,
    pub rsync_exit_code: i32,
    pub item_count: usize,
    pub classification: String,
    pub retryable: bool,
    pub output_path: PathBuf,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CheckRecord {
    pub schema_version: u8,
    pub operation: &'static str,
    pub status: &'static str,
    pub job_id: String,
    pub source: PathBuf,
    pub destination_root: PathBuf,
    pub state_dir: PathBuf,
    pub checks: Vec<&'static str>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct StatusRecord {
    pub schema_version: u8,
    pub operation: &'static str,
    pub status: &'static str,
    pub job_id: String,
    pub latest: Option<RunRecord>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryRecord {
    pub schema_version: u8,
    pub operation: &'static str,
    pub status: &'static str,
    pub job_id: String,
    pub count: usize,
    pub runs: Vec<RunRecord>,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CompletionRecord {
    pub schema_version: u8,
    pub job_id: String,
    pub run_id: String,
    pub config_hash: String,
    pub snapshot_path: String,
    pub completed_at: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorRecord<'a> {
    pub schema_version: u8,
    pub operation: &'static str,
    pub status: &'static str,
    pub code: &'a str,
    pub retryable: bool,
    pub message: &'a str,
    pub suggested_fix: &'a str,
}

#[derive(Debug, Serialize)]
pub struct DescribeRecord {
    pub schema_version: u8,
    pub operation: &'static str,
    pub status: &'static str,
    pub profile: &'static str,
    pub commands: Vec<&'static str>,
    pub safety_invariants: Vec<&'static str>,
    pub exit_codes: std::collections::BTreeMap<&'static str, i32>,
}
