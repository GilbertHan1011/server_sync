mod check_plan;
mod common;
mod query;
mod run;
mod verify;

use super::config::AppConfig;

pub struct RunRequest<'a> {
    pub config: &'a AppConfig,
    pub job_id: &'a str,
    pub plan_run_id: &'a str,
    pub approval: &'a str,
}

pub struct VerifyRequest<'a> {
    pub config: &'a AppConfig,
    pub job_id: &'a str,
    pub run_id: &'a str,
    pub checksum: bool,
}

pub use check_plan::{check_job, plan_job};
pub use query::{history_job, status_job};
pub use run::run_job;
pub use verify::verify_job;
