use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::error::{AppError, AppResult};
use super::state::ensure_private_dir;

#[derive(Debug)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct Execution<'a> {
    pub command: &'a [String],
    pub timeout_seconds: u64,
    pub stdout_path: &'a Path,
    pub stderr_path: &'a Path,
}

pub fn execute(request: Execution<'_>) -> AppResult<CommandOutput> {
    let Execution {
        command,
        timeout_seconds,
        stdout_path,
        stderr_path,
    } = request;
    let Some(program) = command.first() else {
        return Err(AppError::new(
            "command_empty",
            "cannot execute an empty command",
            false,
            "Report this hpc-sync command-construction defect.",
        ));
    };
    ensure_private_dir(stdout_path.parent().unwrap_or_else(|| Path::new(".")))?;
    let stdout_file = create_output(stdout_path)?;
    let stderr_file = create_output(stderr_path)?;
    let mut child = Command::new(program)
        .args(command.iter().skip(1))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|error| {
            AppError::new(
                "command_spawn_failed",
                format!("cannot start {program}: {error}"),
                false,
                "Check that required binaries are installed and executable.",
            )
        })?;
    let deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let exit_code = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| wait_error(program, &error))?
        {
            break status.code().map_or(128, |code| code);
        }
        if Instant::now() >= deadline {
            child.kill().map_err(|error| wait_error(program, &error))?;
            let _status = child.wait().map_err(|error| wait_error(program, &error))?;
            break 30;
        }
        thread::sleep(Duration::from_millis(100));
    };
    let stdout =
        fs::read_to_string(stdout_path).map_err(|error| read_error(stdout_path, &error))?;
    let mut stderr =
        fs::read_to_string(stderr_path).map_err(|error| read_error(stderr_path, &error))?;
    if exit_code == 30 && !stderr.contains("hpc-sync timeout") {
        stderr.push_str("\nhpc-sync timeout\n");
        fs::write(stderr_path, &stderr).map_err(|error| read_error(stderr_path, &error))?;
    }
    Ok(CommandOutput {
        exit_code,
        stdout,
        stderr,
    })
}

pub fn count_itemized_changes(output: &str) -> usize {
    output
        .lines()
        .filter(|line| {
            line.as_bytes()
                .first()
                .is_some_and(|byte| matches!(byte, b'<' | b'>' | b'c' | b'h' | b'.' | b'*'))
        })
        .count()
}

pub fn stable_hash(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn create_output(path: &Path) -> AppResult<std::fs::File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| read_error(path, &error))
}

fn wait_error(program: &str, error: &std::io::Error) -> AppError {
    AppError::new(
        "command_wait_failed",
        format!("cannot wait for {program}: {error}"),
        true,
        "Inspect the host process state and retry once safe.",
    )
}

fn read_error(path: &Path, error: &std::io::Error) -> AppError {
    AppError::new(
        "command_output_failed",
        format!("cannot access command output {}: {error}", path.display()),
        false,
        "Check state directory permissions and free disk space.",
    )
}

#[cfg(test)]
mod tests {
    use super::stable_hash;

    #[test]
    fn stable_hash_is_unambiguous_sha256() {
        let digest = stable_hash(&["ab", "c"]);
        assert_eq!(digest.len(), 64);
        assert_ne!(digest, stable_hash(&["a", "bc"]));
    }
}
