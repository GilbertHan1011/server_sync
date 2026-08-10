use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::super::config::AppConfig;
use super::super::error::{AppError, AppResult};
use super::super::exit_class::classify_exit;
use super::super::process::{Execution, count_itemized_changes, execute, stable_hash};
use super::super::records::{CheckRecord, PlanRecord, RequestRecord};
use super::super::rsync::{TransferRequest, build_transfer, snapshot_path};
use super::super::state::{JobLock, atomic_write_json, ensure_private_dir};
use super::common::{config_hash, latest_successful_run_id, new_run_id, run_dir, utc_now};

pub fn check_job(config: &AppConfig, job_id: &str) -> AppResult<CheckRecord> {
    let job = config.job(job_id)?;
    for binary in ["rsync", "ssh"] {
        if find_binary(binary).is_none() {
            return Err(AppError::new(
                "binary_missing",
                format!("{binary} is not available on PATH"),
                false,
                "Install rsync and OpenSSH or load their HPC modules.",
            ));
        }
    }
    if !job.source.is_dir() {
        return Err(AppError::new(
            "source_missing",
            format!("source directory does not exist: {}", job.source.display()),
            false,
            "Correct the predefined source path.",
        ));
    }
    let mut checks = vec!["rsync_available", "ssh_available", "source_directory"];
    if let Some(marker) = &job.source_complete_marker {
        let path = job.source.join(marker);
        if !path.is_file() {
            return Err(AppError::new(
                "source_incomplete",
                format!("required source marker is missing: {}", path.display()),
                true,
                "Wait for the producer to finish and write its completion marker.",
            ));
        }
        checks.push("source_complete_marker");
    }
    for path in [&job.ssh.identity_file, &job.ssh.known_hosts_file] {
        if !path.is_file() {
            return Err(AppError::new(
                "ssh_file_missing",
                format!("required SSH file is missing: {}", path.display()),
                false,
                "Provision the dedicated key and pinned known_hosts file.",
            ));
        }
    }
    let mode = fs::metadata(&job.ssh.identity_file)
        .map_err(|error| {
            AppError::new(
                "ssh_file_unreadable",
                error.to_string(),
                false,
                "Check key permissions.",
            )
        })?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(AppError::new(
            "identity_permissions",
            format!(
                "SSH identity is accessible by group or others: {}",
                job.ssh.identity_file.display()
            ),
            false,
            format!("Run chmod 600 {}", job.ssh.identity_file.display()),
        ));
    }
    ensure_private_dir(&config.state_dir)?;
    Ok(CheckRecord {
        schema_version: 1,
        operation: "check",
        status: "ready",
        job_id: job_id.to_owned(),
        source: job.source.clone(),
        destination_root: job.destination_root.clone(),
        state_dir: config.state_dir.clone(),
        checks,
        next_actions: vec![format!("hpc-sync plan {job_id} --json")],
    })
}

pub fn plan_job(config: &AppConfig, job_id: &str) -> AppResult<PlanRecord> {
    let _check = check_job(config, job_id)?;
    let job = config.job(job_id)?;
    let run_id = new_run_id();
    let run_dir = ensure_private_dir(&run_dir(config, job_id, &run_id))?;
    let started_at = utc_now();
    let config_hash = config_hash(config, job_id)?;
    let previous_run_id = latest_successful_run_id(config, job_id)?;
    let command = build_transfer(TransferRequest {
        job,
        run_id: &run_id,
        previous_run_id: previous_run_id.as_deref(),
        dry_run: true,
    })?;
    let _lock = JobLock::acquire(
        &config
            .state_dir
            .join("locks")
            .join(format!("{job_id}.lock")),
    )?;
    let output = execute(Execution {
        command: &command,
        timeout_seconds: job.max_runtime_seconds,
        stdout_path: &run_dir.join("items.txt"),
        stderr_path: &run_dir.join("plan.stderr"),
    })?;
    let class = classify_exit(output.exit_code, &output.stderr);
    if output.exit_code != 0 {
        return Err(AppError::new(
            "rsync_plan_failed",
            format!(
                "rsync plan failed with exit code {} ({})",
                output.exit_code, class.name
            ),
            class.retryable,
            format!(
                "Inspect {} before retrying.",
                run_dir.join("plan.stderr").display()
            ),
        )
        .with_exit_code(10));
    }
    let snapshot = snapshot_path(job, &run_id)?;
    let plan_hash = stable_hash(&[
        &config_hash,
        &run_id,
        previous_run_id.as_deref().unwrap_or(""),
        &snapshot,
        &stable_hash(&[&output.stdout]),
    ]);
    let request = RequestRecord {
        schema_version: 1,
        operation: "request".to_owned(),
        job_id: job_id.to_owned(),
        run_id: run_id.clone(),
        config_hash: config_hash.clone(),
        job: job.clone(),
        created_at: started_at.clone(),
    };
    let plan = PlanRecord {
        schema_version: 1,
        operation: "plan".to_owned(),
        status: "planned".to_owned(),
        job_id: job_id.to_owned(),
        run_id: run_id.clone(),
        config_hash,
        plan_hash: plan_hash.clone(),
        previous_run_id,
        snapshot_path: snapshot,
        started_at,
        finished_at: utc_now(),
        rsync_exit_code: output.exit_code,
        item_count: count_itemized_changes(&output.stdout),
        run_dir: run_dir.clone(),
        items_path: run_dir.join("items.txt"),
        command,
        next_actions: vec![format!(
            "hpc-sync run {job_id} --plan-run-id {run_id} --approval {plan_hash} --json"
        )],
    };
    atomic_write_json(&run_dir.join("request.json"), &request)?;
    atomic_write_json(&run_dir.join("plan.json"), &plan)?;
    Ok(plan)
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}
