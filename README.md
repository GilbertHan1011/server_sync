# Server Sync

A terminal UI (TUI) for managing **= sync tasks**, backed by a small local daemon.

- Create **Push** tasks (local → remote) or **Pull** tasks (remote → local)
- Choose a sync strategy: **Mirror**, **AddOnly**, **SafeSync**, **Update**
- Browse local/remote directories inside the TUI
- View per-task logs and run dry-runs
- Tasks are persisted and restored automatically

## Requirements

- Rust toolchain (edition 2024)
- `rsync` available on your machine
- `ssh` available on your machine (OpenSSH)
- For password-based SSH: a working system keyring is recommended (the app stores passwords in the OS keyring when provided)

## Install

Build locally:

```bash
cargo build --release
```

Run:

```bash
cargo run --release
```

## Usage

The main binary is `sync_app` (default).

### Start the TUI (default)

```bash
./target/release/sync_app
```


### Manage the daemon

```bash
sync_app start
sync_app status
sync_app stop
sync_app restart
```

Notes:
- The TUI will **auto-start** the daemon if it is not running.
- The `server` subcommand is internal.

## TUI controls

### Dashboard

- **A**: Add task (Push)
- **P**: Add task (Pull)
- **↑/↓**: Select task (list scrolls)
- **D**: Delete selected task
- **R**: Dry run selected task
- **S**: Restart selected task
- **L**: View selected task logs
- **Ctrl+R**: Restart daemon
- **Q**: Quit

### Browsers (local/remote)

- **↑/↓**: Move selection (list scrolls)
- **Enter**: Enter directory
- **Space**: Select current entry (source/destination depending on flow)
- **N**: Create directory
- **Esc**: Back/cancel

## Where data is stored

All state is kept under your home directory:

- **Daemon socket**: `~/.sync_daemon.sock`
- **Daemon PID**: `~/.sync_daemon.pid`
- **Saved tasks**: `~/.sync_daemon_tasks.json`
- **Saved remote hosts list**: `~/.sync_hosts`
- **Logs directory**: `~/.sync_daemon_logs/`
  - Daemon log: `~/.sync_daemon_logs/daemon.log`
  - Client log: `~/.sync_daemon_logs/client.log`
  - Per-task logs: `~/.sync_daemon_logs/<task_id>.log`

## Sync modes

- **Mirror**: exact copy (uses `--delete`)
- **AddOnly**: add/update only (no delete)
- **SafeSync**: mirror, but deleted files are moved to `.rsync-backup/` (uses `--delete --backup --backup-dir=.rsync-backup`)
- **Update**: only overwrite if local file is newer (uses `--update`)

## Security notes

- Remote host input is validated to reduce SSH flag-injection risk.
- The SSH commands used by the daemon currently include `StrictHostKeyChecking=no` for non-interactive operation. If you need stricter host key verification, you may want to change this in the server SSH/rsync command builders.

## Troubleshooting

- **Nothing happens / can’t connect**: check daemon status with `sync_app status`, and inspect `~/.sync_daemon_logs/daemon.log`.
- **UI shows few items**: lists support scrolling; use **↑/↓** to move selection and the UI will scroll automatically.