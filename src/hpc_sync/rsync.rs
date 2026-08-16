use std::path::Path;

use super::config::JobConfig;
use super::error::{AppError, AppResult};

pub struct TransferRequest<'a> {
    pub job: &'a JobConfig,
    pub run_id: &'a str,
    pub previous_run_id: Option<&'a str>,
    pub dry_run: bool,
}

pub fn snapshot_path(job: &JobConfig, run_id: &str) -> AppResult<String> {
    validate_run_id(run_id)?;
    let root = path_text(&job.destination_root)?.trim_end_matches('/');
    Ok(format!("{root}/snapshots/{run_id}/"))
}

pub fn build_transfer(request: TransferRequest<'_>) -> AppResult<Vec<String>> {
    let TransferRequest {
        job,
        run_id,
        previous_run_id,
        dry_run,
    } = request;
    validate_run_id(run_id)?;
    let mut command = common_options(job);
    if dry_run {
        command.extend(["--dry-run".to_owned(), "--info=stats2".to_owned()]);
    }
    match previous_run_id {
        Some(previous) => {
            let prior = snapshot_path(job, previous)?;
            command.push(format!("--link-dest={}", prior.trim_end_matches('/')));
        }
        None if job.bootstrap_from_destination_root => {
            command.push(format!("--link-dest={}", path_text(&job.destination_root)?));
        }
        None => {}
    }
    command.extend([
        "-e".to_owned(),
        ssh_shell(job)?,
        format!("{}/", path_text(&job.source)?.trim_end_matches('/')),
        remote_target(job, &snapshot_path(job, run_id)?),
    ]);
    Ok(command)
}

pub fn build_verify(job: &JobConfig, run_id: &str, checksum: bool) -> AppResult<Vec<String>> {
    let mut command = common_options(job);
    command.extend(["--dry-run".to_owned(), "--omit-dir-times".to_owned()]);
    if checksum {
        command.push("--checksum".to_owned());
    }
    command.extend([
        "-e".to_owned(),
        ssh_shell(job)?,
        format!("{}/", path_text(&job.source)?.trim_end_matches('/')),
        remote_target(job, &snapshot_path(job, run_id)?),
    ]);
    Ok(command)
}

pub fn build_marker(job: &JobConfig, run_id: &str, marker: &Path) -> AppResult<Vec<String>> {
    Ok(vec![
        "rsync".to_owned(),
        "--archive".to_owned(),
        "--no-owner".to_owned(),
        "--no-group".to_owned(),
        "-e".to_owned(),
        ssh_shell(job)?,
        path_text(marker)?.to_owned(),
        remote_target(
            job,
            &format!("{}.hpc-sync-complete.json", snapshot_path(job, run_id)?),
        ),
    ])
}

fn common_options(job: &JobConfig) -> Vec<String> {
    let mut options = vec![
        "rsync".to_owned(),
        "--archive".to_owned(),
        "--no-owner".to_owned(),
        "--no-group".to_owned(),
        "--no-devices".to_owned(),
        "--no-specials".to_owned(),
        "--partial-dir=.rsync-partial".to_owned(),
        "--delay-updates".to_owned(),
        "--itemize-changes".to_owned(),
    ];
    if job.preserve_hard_links {
        options.push("--hard-links".to_owned());
    }
    if job.preserve_acls {
        options.push("--acls".to_owned());
    }
    if job.preserve_xattrs {
        options.push("--xattrs".to_owned());
    }
    options.extend(job.excludes.iter().map(|item| format!("--exclude={item}")));
    options
}

fn ssh_shell(job: &JobConfig) -> AppResult<String> {
    let ssh = &job.ssh;
    let args = [
        "ssh".to_owned(),
        "-p".to_owned(),
        ssh.port.to_string(),
        "-i".to_owned(),
        path_text(&ssh.identity_file)?.to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "StrictHostKeyChecking=yes".to_owned(),
        "-o".to_owned(),
        format!("UserKnownHostsFile={}", path_text(&ssh.known_hosts_file)?),
        "-o".to_owned(),
        "KnownHostsCommand=none".to_owned(),
        "-o".to_owned(),
        "ProxyCommand=none".to_owned(),
        "-o".to_owned(),
        "IdentitiesOnly=yes".to_owned(),
        "-o".to_owned(),
        "PasswordAuthentication=no".to_owned(),
        "-o".to_owned(),
        "KbdInteractiveAuthentication=no".to_owned(),
        "-o".to_owned(),
        format!("ConnectTimeout={}", ssh.connect_timeout_seconds),
        "-o".to_owned(),
        "ServerAliveInterval=30".to_owned(),
        "-o".to_owned(),
        "ServerAliveCountMax=3".to_owned(),
    ];
    Ok(args
        .iter()
        .map(|argument| shell_quote(argument))
        .collect::<Vec<_>>()
        .join(" "))
}

fn remote_target(job: &JobConfig, path: &str) -> String {
    format!("{}@{}:{path}", job.ssh.user, job.ssh.host)
}

fn shell_quote(value: &str) -> String {
    let safe = value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b'=' | b':')
    });
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn validate_run_id(run_id: &str) -> AppResult<()> {
    let valid = run_id.len() == 25
        && run_id.bytes().enumerate().all(|(index, byte)| match index {
            8 => byte == b'T',
            15 => byte == b'Z',
            16 => byte == b'-',
            0..=7 | 9..=14 => byte.is_ascii_digit(),
            17..=24 => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
            _ => false,
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            "run_id_invalid",
            format!("invalid run id {run_id:?}"),
            false,
            "Use the run id returned by hpc-sync plan.",
        ))
    }
}

fn path_text(path: &Path) -> AppResult<&str> {
    path.to_str().ok_or_else(|| {
        AppError::new(
            "path_invalid",
            format!("path is not valid UTF-8: {}", path.display()),
            false,
            "Use UTF-8 paths in the job configuration.",
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::hpc_sync::config::SshConfig;

    fn job() -> JobConfig {
        JobConfig {
            profile: "backup".to_owned(),
            source: PathBuf::from("/source"),
            destination_root: PathBuf::from("/backup"),
            ssh: SshConfig {
                host: "backup.local".to_owned(),
                user: "sync".to_owned(),
                identity_file: PathBuf::from("/key"),
                known_hosts_file: PathBuf::from("/hosts"),
                port: 22,
                connect_timeout_seconds: 15,
            },
            source_complete_marker: None,
            excludes: Vec::new(),
            preserve_hard_links: false,
            preserve_acls: false,
            preserve_xattrs: false,
            bootstrap_from_destination_root: true,
            verify_after_run: true,
            max_runtime_seconds: 60,
        }
    }

    #[test]
    fn transfer_is_no_delete_and_rrsync_compatible() {
        let job = job();
        let command = build_transfer(TransferRequest {
            job: &job,
            run_id: "20260721T120000Z-abcd1234",
            previous_run_id: Some("20260720T120000Z-1234abcd"),
            dry_run: false,
        })
        .expect("build command");
        assert!(
            command.contains(&"--link-dest=/backup/snapshots/20260720T120000Z-1234abcd".to_owned())
        );
        assert!(!command.iter().any(|argument| argument.contains("--delete")));
        assert!(
            !command
                .iter()
                .any(|argument| argument.contains("--inplace"))
        );
        assert!(
            !command
                .iter()
                .any(|argument| argument == "--protect-args" || argument == "-s")
        );
        assert!(!command.iter().any(|argument| argument == "--mkpath"));
        assert!(
            command
                .iter()
                .any(|argument| argument.contains("KnownHostsCommand=none"))
        );
        assert!(
            command
                .iter()
                .any(|argument| argument.contains("ProxyCommand=none"))
        );
        assert!(!command.iter().any(|argument| argument.contains("..")));
    }

    #[test]
    fn first_snapshot_uses_configured_destination_root_as_baseline() {
        let command = build_transfer(TransferRequest {
            job: &job(),
            run_id: "20260721T120000Z-abcd1234",
            previous_run_id: None,
            dry_run: true,
        })
        .expect("build bootstrap command");
        assert!(command.contains(&"--link-dest=/backup".to_owned()));
    }

    #[test]
    fn verify_ignores_directory_timestamps() {
        let command = build_verify(&job(), "20260721T120000Z-abcd1234", true)
            .expect("build verification command");
        assert!(command.contains(&"--omit-dir-times".to_owned()));
    }
}
