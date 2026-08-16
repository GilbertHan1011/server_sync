use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::error::{AppError, AppResult};

pub struct JobLock {
    file: File,
}

impl JobLock {
    pub fn acquire(path: &Path) -> AppResult<Self> {
        ensure_private_dir(path.parent().unwrap_or_else(|| Path::new(".")))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|error| state_error("state_write_failed", path, &error))?;
        FileExt::try_lock_exclusive(&file).map_err(|error| {
            AppError::new(
                "job_locked",
                format!("another operation holds {}: {error}", path.display()),
                true,
                "Wait for the current one-shot operation to finish, then retry.",
            )
            .with_exit_code(0)
        })?;
        Ok(Self { file })
    }
}

impl Drop for JobLock {
    fn drop(&mut self) {
        let _result = FileExt::unlock(&self.file);
    }
}

pub fn ensure_private_dir(path: &Path) -> AppResult<PathBuf> {
    fs::create_dir_all(path).map_err(|error| state_error("state_write_failed", path, &error))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| state_error("state_write_failed", path, &error))?;
    Ok(path.to_path_buf())
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let mut content = serde_json::to_vec_pretty(value).map_err(|error| {
        AppError::new(
            "state_serialize_failed",
            format!("cannot serialize {}: {error}", path.display()),
            false,
            "Report this hpc-sync serialization defect.",
        )
    })?;
    content.push(b'\n');
    atomic_write(path, &content)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    let content = fs::read(path).map_err(|error| {
        AppError::new(
            "state_unreadable",
            format!("cannot read state file {}: {error}", path.display()),
            false,
            "Check the run id and state directory permissions.",
        )
    })?;
    serde_json::from_slice(&content).map_err(|error| {
        AppError::new(
            "state_corrupt",
            format!("invalid state file {}: {error}", path.display()),
            false,
            "Preserve the file for audit and create a new plan.",
        )
    })
}

fn atomic_write(path: &Path, content: &[u8]) -> AppResult<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ensure_private_dir(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let temporary = parent.join(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));
    let result = write_then_rename(&temporary, path, content);
    if result.is_err() {
        let _cleanup = fs::remove_file(&temporary);
    }
    result
}

fn write_then_rename(temporary: &Path, target: &Path, content: &[u8]) -> AppResult<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temporary)
        .map_err(|error| state_error("state_write_failed", temporary, &error))?;
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| state_error("state_write_failed", temporary, &error))?;
    fs::rename(temporary, target).map_err(|error| state_error("state_write_failed", target, &error))
}

fn state_error(code: &'static str, path: &Path, error: &io::Error) -> AppError {
    AppError::new(
        code,
        format!("cannot access {}: {error}", path.display()),
        false,
        "Check state directory permissions and free disk space.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_is_rejected() {
        let root = std::env::temp_dir().join(format!("hpc-sync-lock-{}", std::process::id()));
        fs::create_dir_all(&root).expect("create test directory");
        let path = root.join("job.lock");
        let first = JobLock::acquire(&path).expect("acquire first lock");
        let second = JobLock::acquire(&path);
        assert!(second.is_err());
        drop(first);
        fs::remove_dir_all(root).expect("remove test directory");
    }
}
