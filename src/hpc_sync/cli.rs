use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde::Serialize;

use super::config::{expand_home, load_config};
use super::error::{AppError, AppResult};
use super::operations::{
    RunRequest, VerifyRequest, check_job, history_job, plan_job, run_job, status_job, verify_job,
};
use super::records::{DescribeRecord, ErrorRecord, RunStatus, VerifyStatus};

#[derive(Debug, Parser)]
#[command(
    name = "hpc-sync",
    version,
    about = "One-shot, no-delete HPC backup orchestration",
    arg_required_else_help = true
)]
struct Cli {
    #[arg(
        long,
        env = "HPC_SYNC_CONFIG",
        default_value = "~/.config/hpc-sync/config.toml",
        global = true
    )]
    config: PathBuf,

    /// Explicitly request the default compact JSON output.
    #[arg(long, global = true)]
    json: bool,

    /// Indent JSON output for human inspection.
    #[arg(long, global = true, conflicts_with = "json")]
    pretty: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate local prerequisites without starting a transfer.
    Check { job_id: String },
    /// Create an audited rsync dry-run and approval hash.
    Plan { job_id: String },
    /// Apply one reviewed plan and create a recovery point.
    Run {
        job_id: String,
        #[arg(long)]
        plan_run_id: String,
        #[arg(long)]
        approval: String,
    },
    /// Compare a recovery point with the current source.
    Verify {
        job_id: String,
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        checksum: bool,
    },
    /// Return the latest authoritative run state.
    Status { job_id: String },
    /// Return a bounded list of audited run results.
    History {
        job_id: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Describe the stable agent contract and safety boundaries.
    Describe,
}

pub fn run() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return handle_clap_error(error),
    };
    match dispatch(&cli) {
        Ok(exit_code) => exit_code,
        Err(error) => {
            let record = ErrorRecord {
                schema_version: 1,
                operation: "error",
                status: "error",
                code: error.code,
                retryable: error.retryable,
                message: &error.message,
                suggested_fix: &error.suggested_fix,
            };
            if emit(&record, false).is_err() {
                return 2;
            }
            error.exit_code
        }
    }
}

fn dispatch(cli: &Cli) -> AppResult<i32> {
    let pretty = cli.pretty;
    match &cli.command {
        Command::Describe => {
            let record = describe_record();
            emit(&record, pretty)?;
            Ok(0)
        }
        Command::Check { job_id } => {
            emit(
                &check_job(&load_config(&expand_home(&cli.config))?, job_id)?,
                pretty,
            )?;
            Ok(0)
        }
        Command::Plan { job_id } => {
            emit(
                &plan_job(&load_config(&expand_home(&cli.config))?, job_id)?,
                pretty,
            )?;
            Ok(0)
        }
        Command::Run {
            job_id,
            plan_run_id,
            approval,
        } => {
            let config = load_config(&expand_home(&cli.config))?;
            let result = run_job(RunRequest {
                config: &config,
                job_id,
                plan_run_id,
                approval,
            })?;
            let success = result.status == RunStatus::Success;
            emit(&result, pretty)?;
            Ok(if success { 0 } else { 10 })
        }
        Command::Verify {
            job_id,
            run_id,
            checksum,
        } => {
            let config = load_config(&expand_home(&cli.config))?;
            let result = verify_job(VerifyRequest {
                config: &config,
                job_id,
                run_id,
                checksum: *checksum,
            })?;
            let verified = result.status == VerifyStatus::Verified;
            emit(&result, pretty)?;
            Ok(if verified { 0 } else { 11 })
        }
        Command::Status { job_id } => {
            emit(
                &status_job(&load_config(&expand_home(&cli.config))?, job_id)?,
                pretty,
            )?;
            Ok(0)
        }
        Command::History { job_id, limit } => {
            if !(1..=100).contains(limit) {
                return Err(AppError::new(
                    "limit_invalid",
                    "history limit must be between 1 and 100",
                    false,
                    "Pass --limit with a value from 1 through 100.",
                ));
            }
            emit(
                &history_job(&load_config(&expand_home(&cli.config))?, job_id, *limit)?,
                pretty,
            )?;
            Ok(0)
        }
    }
}

fn emit<T: Serialize>(record: &T, pretty: bool) -> AppResult<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let result = if pretty {
        serde_json::to_writer_pretty(&mut handle, record)
    } else {
        serde_json::to_writer(&mut handle, record)
    };
    result.map_err(|error| {
        AppError::new("output_failed", error.to_string(), false, "Check stdout.")
    })?;
    writeln!(handle).map_err(|error| {
        AppError::new(
            "output_failed",
            error.to_string(),
            false,
            "Check stdout availability.",
        )
    })
}

fn handle_clap_error(error: clap::Error) -> i32 {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _printed = error.print();
            0
        }
        _ => {
            let message = error.to_string();
            let record = ErrorRecord {
                schema_version: 1,
                operation: "error",
                status: "error",
                code: "cli_usage",
                retryable: false,
                message: &message,
                suggested_fix: "Run hpc-sync --help or hpc-sync describe --json.",
            };
            let _emitted = emit(&record, false);
            2
        }
    }
}

fn describe_record() -> DescribeRecord {
    DescribeRecord {
        schema_version: 1,
        operation: "describe",
        status: "available",
        profile: "backup",
        commands: vec![
            "check", "plan", "run", "verify", "status", "history", "describe",
        ],
        safety_invariants: vec![
            "predefined_jobs_only",
            "hpc_to_local_only",
            "backup_profile_only",
            "no_delete",
            "no_inplace",
            "strict_host_key_checking",
            "non_interactive_ssh",
            "one_shot_process",
        ],
        exit_codes: BTreeMap::from([
            ("success", 0),
            ("usage_or_config", 2),
            ("rsync_failed", 10),
            ("verify_failed", 11),
        ]),
    }
}
