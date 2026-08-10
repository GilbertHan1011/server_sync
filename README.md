# hpc-sync

`hpc-sync` is a one-shot, agent-friendly CLI for backing up predefined HPC directories to a
restricted remote receiver with `rsync`.

It replaces the resident polling daemon for HPC use. Each invocation plans, runs, verifies, and
exits. Version 0.1 supports only one-way **HPC → local backup** jobs.

## Safety contract

- Only predefined TOML job IDs are accepted; runtime source, destination, and shell fragments are
  not accepted.
- Every transfer creates a unique `snapshots/<run-id>/` recovery point.
- Unchanged files are hard-linked with `--link-dest` after the first successful run.
- No `--delete`, `--inplace`, password, TOTP automation, interactive SSH, or persistent daemon.
- SSH uses a dedicated identity, pinned `known_hosts`, `BatchMode=yes`, and
  `StrictHostKeyChecking=yes`.
- A per-job OS file lock prevents overlapping operations.
- The run is marked complete only after transfer, comparison, and remote completion-marker upload
  succeed.
- JSON stdout is stable for agents; command output and audit files are stored under `state_dir`.

`rsync` does not snapshot a changing source tree. Prefer immutable release directories, or require
`source_complete_marker` in the job configuration.

## Requirements

- Rust 1.85+ toolchain (edition 2024)
- rsync 3.2+ on both ends
- OpenSSH client on the HPC
- A dedicated receiver key restricted with `rrsync`

Build and install the one-shot binary:

```bash
cargo build --release --bin hpc-sync
install -m 0755 target/release/hpc-sync "$HOME/.local/bin/hpc-sync"
```

It can also be exercised directly from this repository:

```bash
cargo run --release --bin hpc-sync -- describe --json
```

## Configure

Copy `examples/hpc-sync.example.toml` to a private location such as:

```text
~/.config/hpc-sync/config.toml
```

Unknown TOML keys are rejected. Paths must be absolute. `destination_root` is the path in the
restricted receiver namespace, not an unrestricted local filesystem path.

Provision and pin the receiver host key out of band. The private key must have mode `0600`.

On the local receiver, dedicate a directory and add a forced command to `~/.ssh/authorized_keys`:

```text
restrict,command="/usr/bin/rrsync -wo -no-del /home/gilberthan/disk1/hpc-backups" ssh-ed25519 AAAA... hpc-sync
```

With that restriction, `destination_root = "/macrophage-atac"` maps to
`/home/gilberthan/disk1/hpc-backups/macrophage-atac`. Use a separate key/root per trust boundary.
If stable HPC egress addresses are known, add an authorized-keys `from="..."` restriction.

## Agent workflow

Set the configuration once:

```bash
export HPC_SYNC_CONFIG="$HOME/.config/hpc-sync/config.toml"
```

Discover the contract and validate local prerequisites:

```bash
hpc-sync describe --json
hpc-sync check macrophage-atac --json
```

Create a dry-run plan:

```bash
hpc-sync plan macrophage-atac --json
```

Review the returned `items_path`, then apply exactly the returned `run_id` and `plan_hash`:

```bash
hpc-sync run macrophage-atac \
  --plan-run-id 20260721T120000Z-abcd1234 \
  --approval PLAN_HASH \
  --json
```

Query and verify recovery points:

```bash
hpc-sync status macrophage-atac --json
hpc-sync history macrophage-atac --limit 10 --json
hpc-sync verify macrophage-atac --run-id RUN_ID --checksum --json
```

Exit codes are described by `hpc-sync describe --json`. Failures return typed JSON with a stable
error code, retryability, and suggested fix.

## Audit state

```text
state_dir/
  locks/<job-id>.lock
  latest/<job-id>.json
  runs/<job-id>/<run-id>/
    request.json
    plan.json
    items.txt
    result.json
    COMPLETED
    *.stdout
    *.stderr
```

Files are written atomically with mode `0600`; state directories use mode `0700`.

## Deployment boundary

Version 0.1 intentionally has no daemon, watcher, scheduler, deletion, retention cleanup, mirror,
publish, restore-overwrite, or bidirectional synchronization. Run it manually first on a small
noncritical release and perform a scratch restore before scheduling it.

Do not add cron on the HPC until administrators confirm that bounded login-node transfers are
allowed. Slurm-based transfer scheduling remains deferred until compute-node storage visibility,
network egress, strict SSH, and the restricted key are tested.

## Legacy Rust TUI

The existing Rust TUI/daemon remains in `src/` for migration reference. It is not recommended on
the HPC: it restores resident workers, polls recursively, and its current SSH path disables strict
host-key checking. Do not restart it as a fallback for `hpc-sync`.
