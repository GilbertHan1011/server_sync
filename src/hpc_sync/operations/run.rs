use super::super::config::AppConfig;
use super::super::error::{AppError, AppResult};
use super::super::exit_class::classify_exit;
use super::super::process::{Execution, count_itemized_changes, execute};
use super::super::records::{CompletionRecord, PlanRecord, RequestRecord, RunRecord, RunStatus};
use super::super::rsync::{TransferRequest, build_marker, build_transfer, build_verify};
use super::super::state::{JobLock, atomic_write_json, read_json};
use super::RunRequest;
use super::common::{config_hash, run_dir, utc_now};

pub fn run_job(input: RunRequest<'_>) -> AppResult<RunRecord> {
    let config = input.config;
    let job_id = input.job_id;
    let plan_run_id = input.plan_run_id;
    let job = config.job(job_id)?;
    let run_dir = run_dir(config, job_id, plan_run_id);
    let result_path = run_dir.join("result.json");
    if result_path.is_file() {
        let existing: RunRecord = read_json(&result_path)?;
        if existing.status == RunStatus::Success {
            return Ok(existing);
        }
    }
    let plan: PlanRecord = read_json(&run_dir.join("plan.json"))?;
    let request: RequestRecord = read_json(&run_dir.join("request.json"))?;
    validate_approval(&input, &plan, &request)?;
    let started_at = utc_now();
    let command = build_transfer(TransferRequest {
        job,
        run_id: plan_run_id,
        previous_run_id: plan.previous_run_id.as_deref(),
        dry_run: false,
    })?;
    let _lock = JobLock::acquire(
        &config
            .state_dir
            .join("locks")
            .join(format!("{job_id}.lock")),
    )?;
    let transfer = execute(Execution {
        command: &command,
        timeout_seconds: job.max_runtime_seconds,
        stdout_path: &run_dir.join("rsync.stdout"),
        stderr_path: &run_dir.join("rsync.stderr"),
    })?;
    if transfer.exit_code != 0 {
        let class = classify_exit(transfer.exit_code, &transfer.stderr);
        let result = RunRecord {
            schema_version: 1,
            operation: "run".to_owned(),
            status: RunStatus::Failed,
            job_id: job_id.to_owned(),
            run_id: plan_run_id.to_owned(),
            config_hash: plan.config_hash.clone(),
            plan_hash: plan.plan_hash.clone(),
            previous_run_id: plan.previous_run_id.clone(),
            snapshot_path: plan.snapshot_path.clone(),
            started_at,
            finished_at: utc_now(),
            rsync_exit_code: transfer.exit_code,
            classification: class.name.to_owned(),
            retryable: class.retryable,
            verify_exit_code: None,
            verify_item_count: None,
            marker_exit_code: None,
            run_dir: run_dir.clone(),
            completed_marker: None,
            command,
            next_actions: vec![format!(
                "Inspect {}",
                run_dir.join("rsync.stderr").display()
            )],
        };
        persist_result(config, &result)?;
        return Ok(result);
    }

    let mut verify_exit_code = None;
    let mut verify_item_count = None;
    if job.verify_after_run {
        let verify_command = build_verify(job, plan_run_id, false)?;
        let verification = execute(Execution {
            command: &verify_command,
            timeout_seconds: job.max_runtime_seconds,
            stdout_path: &run_dir.join("verify.stdout"),
            stderr_path: &run_dir.join("verify.stderr"),
        })?;
        let item_count = count_itemized_changes(&verification.stdout);
        verify_exit_code = Some(verification.exit_code);
        verify_item_count = Some(item_count);
        if verification.exit_code != 0 || item_count != 0 {
            let class = classify_exit(verification.exit_code, &verification.stderr);
            let mismatch = verification.exit_code == 0;
            let result = RunRecord {
                schema_version: 1,
                operation: "run".to_owned(),
                status: RunStatus::VerificationFailed,
                job_id: job_id.to_owned(),
                run_id: plan_run_id.to_owned(),
                config_hash: plan.config_hash.clone(),
                plan_hash: plan.plan_hash.clone(),
                previous_run_id: plan.previous_run_id.clone(),
                snapshot_path: plan.snapshot_path.clone(),
                started_at,
                finished_at: utc_now(),
                rsync_exit_code: transfer.exit_code,
                classification: if mismatch {
                    "verification_mismatch".to_owned()
                } else {
                    class.name.to_owned()
                },
                retryable: mismatch || class.retryable,
                verify_exit_code,
                verify_item_count,
                marker_exit_code: None,
                run_dir: run_dir.clone(),
                completed_marker: None,
                command,
                next_actions: vec![format!(
                    "Inspect {}",
                    run_dir.join("verify.stdout").display()
                )],
            };
            persist_result(config, &result)?;
            return Ok(result);
        }
    }

    let completion = CompletionRecord {
        schema_version: 1,
        job_id: job_id.to_owned(),
        run_id: plan_run_id.to_owned(),
        config_hash: plan.config_hash.clone(),
        snapshot_path: plan.snapshot_path.clone(),
        completed_at: utc_now(),
    };
    let remote_marker = run_dir.join("remote-complete.json");
    atomic_write_json(&remote_marker, &completion)?;
    let marker_command = build_marker(job, plan_run_id, &remote_marker)?;
    let marker = execute(Execution {
        command: &marker_command,
        timeout_seconds: job.max_runtime_seconds,
        stdout_path: &run_dir.join("marker.stdout"),
        stderr_path: &run_dir.join("marker.stderr"),
    })?;
    if marker.exit_code != 0 {
        let class = classify_exit(marker.exit_code, &marker.stderr);
        let result = RunRecord {
            schema_version: 1,
            operation: "run".to_owned(),
            status: RunStatus::Failed,
            job_id: job_id.to_owned(),
            run_id: plan_run_id.to_owned(),
            config_hash: plan.config_hash.clone(),
            plan_hash: plan.plan_hash.clone(),
            previous_run_id: plan.previous_run_id.clone(),
            snapshot_path: plan.snapshot_path.clone(),
            started_at,
            finished_at: utc_now(),
            rsync_exit_code: transfer.exit_code,
            classification: class.name.to_owned(),
            retryable: class.retryable,
            verify_exit_code,
            verify_item_count,
            marker_exit_code: Some(marker.exit_code),
            run_dir: run_dir.clone(),
            completed_marker: None,
            command,
            next_actions: vec![format!(
                "Inspect {}",
                run_dir.join("marker.stderr").display()
            )],
        };
        persist_result(config, &result)?;
        return Ok(result);
    }

    let completed_marker = run_dir.join("COMPLETED");
    atomic_write_json(&completed_marker, &completion)?;
    let result = RunRecord {
        schema_version: 1,
        operation: "run".to_owned(),
        status: RunStatus::Success,
        job_id: job_id.to_owned(),
        run_id: plan_run_id.to_owned(),
        config_hash: plan.config_hash,
        plan_hash: plan.plan_hash,
        previous_run_id: plan.previous_run_id,
        snapshot_path: plan.snapshot_path,
        started_at,
        finished_at: utc_now(),
        rsync_exit_code: transfer.exit_code,
        classification: "success".to_owned(),
        retryable: false,
        verify_exit_code,
        verify_item_count,
        marker_exit_code: Some(marker.exit_code),
        run_dir: run_dir.clone(),
        completed_marker: Some(completed_marker),
        command,
        next_actions: vec![format!(
            "hpc-sync verify {job_id} --run-id {plan_run_id} --checksum --json"
        )],
    };
    persist_result(config, &result)?;
    Ok(result)
}

fn validate_approval(
    run: &RunRequest<'_>,
    plan: &PlanRecord,
    request: &RequestRecord,
) -> AppResult<()> {
    if plan.job_id != run.job_id || request.job_id != run.job_id {
        return Err(AppError::new(
            "plan_job_mismatch",
            "stored plan belongs to a different job",
            false,
            "Use the job id returned by hpc-sync plan.",
        ));
    }
    if run.approval != plan.plan_hash {
        return Err(AppError::new(
            "approval_mismatch",
            "approval does not match the stored plan hash",
            false,
            "Pass the exact plan_hash returned by hpc-sync plan.",
        ));
    }
    let current_hash = config_hash(run.config, run.job_id)?;
    if current_hash != plan.config_hash || current_hash != request.config_hash {
        return Err(AppError::new(
            "config_changed",
            "job configuration changed after planning",
            false,
            "Create and review a new plan.",
        ));
    }
    Ok(())
}

fn persist_result(config: &AppConfig, result: &RunRecord) -> AppResult<()> {
    atomic_write_json(&result.run_dir.join("result.json"), result)?;
    atomic_write_json(
        &config
            .state_dir
            .join("latest")
            .join(format!("{}.json", result.job_id)),
        result,
    )
}
