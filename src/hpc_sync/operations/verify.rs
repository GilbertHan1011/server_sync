use super::super::error::{AppError, AppResult};
use super::super::exit_class::classify_exit;
use super::super::process::{Execution, count_itemized_changes, execute};
use super::super::records::{PlanRecord, RequestRecord, VerifyRecord, VerifyStatus};
use super::super::rsync::build_verify;
use super::super::state::{JobLock, atomic_write_json, read_json};
use super::VerifyRequest;
use super::common::{config_hash, run_dir, utc_now};

pub fn verify_job(request: VerifyRequest<'_>) -> AppResult<VerifyRecord> {
    let config = request.config;
    let job_id = request.job_id;
    let run_id_value = request.run_id;
    let checksum = request.checksum;
    let job = config.job(job_id)?;
    let run_dir = run_dir(config, job_id, run_id_value);
    let plan: PlanRecord = read_json(&run_dir.join("plan.json"))?;
    let request: RequestRecord = read_json(&run_dir.join("request.json"))?;
    if config_hash(config, job_id)? != request.config_hash {
        return Err(AppError::new(
            "config_changed",
            "current configuration differs from the recovery point",
            false,
            "Restore the audited config before verifying this recovery point.",
        ));
    }
    let started_at = utc_now();
    let suffix = if checksum { "checksum" } else { "quick" };
    let command = build_verify(job, run_id_value, checksum)?;
    let _lock = JobLock::acquire(
        &config
            .state_dir
            .join("locks")
            .join(format!("{job_id}.lock")),
    )?;
    let output_path = run_dir.join(format!("verify-{suffix}.stdout"));
    let output = execute(Execution {
        command: &command,
        timeout_seconds: job.max_runtime_seconds,
        stdout_path: &output_path,
        stderr_path: &run_dir.join(format!("verify-{suffix}.stderr")),
    })?;
    let item_count = count_itemized_changes(&output.stdout);
    let class = classify_exit(output.exit_code, &output.stderr);
    let (status, classification, retryable, next_actions) =
        if output.exit_code == 0 && item_count == 0 {
            (
                VerifyStatus::Verified,
                "success".to_owned(),
                false,
                vec!["Recovery point matches the current source scope.".to_owned()],
            )
        } else if output.exit_code == 0 {
            (
                VerifyStatus::Mismatch,
                "verification_mismatch".to_owned(),
                true,
                vec![format!("Inspect {}", output_path.display())],
            )
        } else {
            (
                VerifyStatus::Failed,
                class.name.to_owned(),
                class.retryable,
                vec![format!(
                    "Inspect {}",
                    run_dir.join(format!("verify-{suffix}.stderr")).display()
                )],
            )
        };
    let record = VerifyRecord {
        schema_version: 1,
        operation: "verify".to_owned(),
        status,
        job_id: job_id.to_owned(),
        run_id: run_id_value.to_owned(),
        snapshot_path: plan.snapshot_path,
        checksum,
        started_at,
        finished_at: utc_now(),
        rsync_exit_code: output.exit_code,
        item_count,
        classification,
        retryable,
        output_path,
        next_actions,
    };
    atomic_write_json(&run_dir.join(format!("verify-{suffix}.json")), &record)?;
    Ok(record)
}
