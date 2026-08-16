use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn plan_then_run_records_completed_recovery_point() {
    // Given a valid predefined job and non-interactive fake transport binaries.
    let root = test_root();
    let source = root.join("source");
    let bin = root.join("bin");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&bin).expect("create bin");
    fs::write(source.join(".complete"), "ready\n").expect("write source marker");
    let key = root.join("id_hpc_sync");
    fs::write(&key, "test key\n").expect("write key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).expect("protect key");
    let known_hosts = root.join("known_hosts");
    fs::write(&known_hosts, "backup.local ssh-ed25519 AAAATEST\n").expect("write hosts");
    write_executable(
        &bin.join("rsync"),
        r#"#!/bin/sh
case " $* " in
  *" --dry-run "*" --info=stats2 "*) printf '>f+++++++++ data.txt\n' ;;
esac
exit 0
"#,
    );
    write_executable(&bin.join("ssh"), "#!/bin/sh\nexit 0\n");
    let config = root.join("config.toml");
    fs::write(
        &config,
        format!(
            r#"version = 1
state_dir = "{}"
[jobs.demo]
profile = "backup"
source = "{}"
destination_root = "/backups/demo"
source_complete_marker = ".complete"
verify_after_run = true
max_runtime_seconds = 60
[jobs.demo.ssh]
host = "backup.local"
user = "sync"
identity_file = "{}"
known_hosts_file = "{}"
"#,
            root.join("state").display(),
            source.display(),
            key.display(),
            known_hosts.display()
        ),
    )
    .expect("write config");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH"));

    // When an agent creates and applies the approved plan.
    let planned = invoke(
        &path,
        &["plan", "demo", "--config", text(&config), "--json"],
    );
    assert!(
        planned.status.success(),
        "{}",
        String::from_utf8_lossy(&planned.stderr)
    );
    let plan: Value = serde_json::from_slice(&planned.stdout).expect("parse plan");
    let run_id = plan["run_id"].as_str().expect("plan run id");
    let plan_hash = plan["plan_hash"].as_str().expect("plan hash");
    let applied = invoke(
        &path,
        &[
            "run",
            "demo",
            "--config",
            text(&config),
            "--plan-run-id",
            run_id,
            "--approval",
            plan_hash,
            "--json",
        ],
    );

    // Then JSON and durable state identify a verified recovery point.
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let result: Value = serde_json::from_slice(&applied.stdout).expect("parse result");
    assert_eq!(result["status"], "success");
    let run_dir = PathBuf::from(result["run_dir"].as_str().expect("run directory"));
    assert!(run_dir.join("COMPLETED").is_file());
    fs::remove_dir_all(root).expect("remove test tree");
}

#[test]
fn invalid_config_returns_typed_json_error() {
    // Given a closed-schema config containing an unknown field.
    let root = test_root();
    fs::create_dir_all(&root).expect("create test root");
    let config = root.join("invalid.toml");
    fs::write(
        &config,
        format!(
            "version = 1\nstate_dir = {:?}\nunknown = true\n",
            root.join("state")
        ),
    )
    .expect("write invalid config");

    // When an agent invokes a config-dependent command.
    let output = invoke(
        &std::env::var("PATH").expect("PATH"),
        &["check", "demo", "--config", text(&config), "--json"],
    );

    // Then stdout is machine-readable and identifies a non-retryable config error.
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).expect("parse error");
    assert_eq!(error["code"], "config_invalid");
    assert_eq!(error["retryable"], false);
    fs::remove_dir_all(root).expect("remove test tree");
}

fn invoke(path: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hpc-sync"))
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run hpc-sync")
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod executable");
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 test path")
}

fn test_root() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("hpc-sync-e2e-{}-{nonce}", std::process::id()))
}
