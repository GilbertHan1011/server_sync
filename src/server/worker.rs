use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use regex::Regex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use crate::protocol::{SyncTask, SyncMode};
use crate::common::utils::is_valid_host;
use crate::server::state::{ServerState, update_status, update_log};
use crate::server::ssh::setup_askpass_script;

// --- SYNC WORKER ---
// Spawns a dedicated thread for a single folder
pub fn spawn_sync_worker(
    task_data: SyncTask,
    state_handle: Arc<Mutex<ServerState>>,
) -> mpsc::Sender<()> {
    let (tx_kill, mut rx_kill) = mpsc::channel(1);
    let task_id = task_data.id.clone();
    let source = task_data.source.clone();
    let interval = task_data.poll_interval;
    let task = Arc::new(task_data); // Wrap in Arc for sharing

    tokio::spawn(async move {
        // 1. Initial Sync with Retry
        retry_run_rsync(&task, &state_handle).await;

        // 2. Setup Watcher
        let (tx_file, mut rx_file) = mpsc::channel(100);
        let path_clone = source.clone();

        // Blocking Watcher Thread
        std::thread::spawn(move || {
            let (wt_tx, wt_rx) = std::sync::mpsc::channel();
            let config = Config::default().with_poll_interval(Duration::from_secs(interval));
            
            if let Ok(mut watcher) = PollWatcher::new(wt_tx, config) {
                if watcher.watch(std::path::Path::new(&path_clone), RecursiveMode::Recursive).is_ok() {
                    for _ in wt_rx {
                        if tx_file.blocking_send(()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // 3. Event Loop
        let mut debounce = None;
        
        loop {
            tokio::select! {
                _ = rx_kill.recv() => {
                    update_log(&task_id, "Stopped", &state_handle);
                    break;
                }
                _ = rx_file.recv() => {
                    // Only set debounce if we aren't already waiting
                    // This creates a "grouping" window of 2 seconds
                    if debounce.is_none() {
                        update_status(&task_id, "PENDING...", &state_handle);
                        update_log(&task_id, "📝 Change detected, waiting 2s...", &state_handle);
                        debounce = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                    }
                }
            }

            if let Some(time) = debounce {
                if tokio::time::Instant::now() >= time {
                    debounce = None; // Reset
                    retry_run_rsync(&task, &state_handle).await;
                }
            }
            
            // Tiny sleep to prevent tight loop burning CPU
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });

    tx_kill
}

// Wrapper to handle Retries with exponential backoff
pub async fn retry_run_rsync(task: &SyncTask, state: &Arc<Mutex<ServerState>>) {
    let mut attempts = 0;
    const MAX_RETRIES: u32 = 3;

    while attempts < MAX_RETRIES {
        run_rsync(task, state).await;
        
        // Check if success
        let success = {
            let s = state.lock().unwrap();
            if let Some(t) = s.tasks.get(&task.id) {
                t.status == "IDLE"
            } else { 
                false 
            }
        };

        if success { 
            return; 
        }

        attempts += 1;
        if attempts < MAX_RETRIES {
            let wait = Duration::from_secs(2u64.pow(attempts));
            update_log(&task.id, &format!("⚠️ Retry {}/{} in {}s...", attempts, MAX_RETRIES, wait.as_secs()), state);
            tokio::time::sleep(wait).await;
        }
    }
    update_status(&task.id, "ERROR (Max Retries)", state);
}

// Dry run: Show what would change without making changes
pub async fn run_dry_run(task: &SyncTask) -> Vec<String> {
    // SECURITY: Validate host before using in rsync command
    if !is_valid_host(&task.remote_host) {
        return vec!["Error: Invalid remote host format".to_string()];
    }

    let full_remote = format!("{}:{}", task.remote_host, task.remote_path);
    let mut cmd = Command::new("rsync");
    
    // --- ASKPASS LOGIC ---
    if let Some(pass) = &task.password {
        if let Ok(script_path) = setup_askpass_script() {
            cmd.env("SSH_ASKPASS", &script_path);
            cmd.env("SSH_ASKPASS_REQUIRE", "force");
            cmd.env("SERVER_SYNC_PW", pass);
            cmd.env("DISPLAY", ":0");
        }
    }
    // ---------------------
    
    cmd.arg("-avn"); // -n = dry run, -a = archive, -v = verbose
    cmd.arg("--itemize-changes"); // Show detailed changes
    
    // Use same SSH optimization with StrictHostKeyChecking=no
    let ssh_cmd = "ssh -T -c aes128-gcm@openssh.com -o Compression=no -o StrictHostKeyChecking=no -o ControlMaster=auto -o ControlPath=~/.ssh/sockets/%r@%h-%p -o ControlPersist=600";
    cmd.arg("-e").arg(ssh_cmd);
    
    if task.compress { 
        cmd.arg("-z"); 
    }
    
    match task.sync_mode {
        SyncMode::Mirror => { 
            cmd.arg("--delete"); 
        }
        SyncMode::SafeSync => { 
            cmd.arg("--delete").arg("--backup").arg("--backup-dir=.rsync-backup"); 
        }
        SyncMode::Update => { 
            cmd.arg("--update"); 
        }
        SyncMode::AddOnly => {}
    }
    
    cmd.arg("--filter=:- .gitignore");
    cmd.arg(format!("{}/", task.source));
    cmd.arg(&full_remote);
    
    match cmd.output().await {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect()
        }
        Ok(o) => vec![format!("Error: {}", String::from_utf8_lossy(&o.stderr))],
        Err(e) => vec![format!("Exec Error: {}", e)],
    }
}

pub async fn run_rsync(task: &SyncTask, state: &Arc<Mutex<ServerState>>) {
    // SECURITY: Validate host before using in rsync command
    if !is_valid_host(&task.remote_host) {
        update_status(&task.id, "ERROR (Bad Host)", state);
        update_log(&task.id, "❌ Invalid remote host format", state);
        return;
    }

    update_status(&task.id, "SYNCING 0%", state);
    update_log(&task.id, "🔄 Starting sync...", state);

    let full_remote = format!("{}:{}", task.remote_host, task.remote_path);
    let port = task.remote_port.unwrap_or(22);
    
    let mut cmd = Command::new("rsync"); // Now tokio::process::Command
    if let Some(pass) = &task.password {
        // 1. We assume the script is created. Since run_rsync is called often,
        // you might want to create it once in main(), or just overwrite it here.
        // Overwriting is safer to ensure it exists.
        if let Ok(script_path) = setup_askpass_script() { 
            cmd.env("SSH_ASKPASS", &script_path);
            cmd.env("SSH_ASKPASS_REQUIRE", "force");
            cmd.env("SERVER_SYNC_PW", pass);
            cmd.env("DISPLAY", ":0");
        }
    }
    cmd.kill_on_drop(true); // Safety: kill rsync if task is cancelled
    
    // Base flags: archive + verbose
    cmd.arg("-av");
    
    // Progress tracking
    cmd.arg("--info=progress2");
    cmd.arg("--no-inc-recursive");
    
    // --- PERFORMANCE OPTIMIZATION ---
    // --partial: Keeps partial files if connection drops (Critical for large files)
    cmd.arg("--partial");
    // --inplace: Modifies files directly. Saves disk space, but slightly riskier if crash.
    cmd.arg("--inplace");
    
    // SSH with ControlMaster for connection reuse
    // --- SSH OPTIMIZATION ---
    // -T: Disable pseudo-tty (faster)
    // -c aes128-gcm@openssh.com: Fastest hardware cipher
    // Compression=no: Don't double-compress large binary files
    // StrictHostKeyChecking=no: Auto-accept host keys (prevents hanging on yes/no prompt)
    let ssh_cmd = format!(
        "ssh -p {} -T -c aes128-gcm@openssh.com -o Compression=no -o StrictHostKeyChecking=no -o ControlMaster=auto -o ControlPath=~/.ssh/sockets/%r@%h-%p -o ControlPersist=600", 
        port
    );
    cmd.arg("-e").arg(ssh_cmd);
    
    // Compression (only if user explicitly asked)
    if task.compress {
        cmd.arg("-z");
    }
    
    // Sync mode specific flags
    match task.sync_mode {
        SyncMode::Mirror => {
            cmd.arg("--delete");
        }
        SyncMode::AddOnly => {
            // No delete flags - only add/update files
        }
        SyncMode::SafeSync => {
            cmd.arg("--delete");
            cmd.arg("--backup");
            cmd.arg("--backup-dir=.rsync-backup");
        }
        SyncMode::Update => {
            cmd.arg("--update");
        }
    }
    
    // Respect .gitignore files
    cmd.arg("--filter=:- .gitignore");
    
    // Paths
    cmd.arg(format!("{}/", task.source));
    cmd.arg(&full_remote);
    
    // Stream output for real-time progress
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    match cmd.spawn() {
        Ok(mut child) => {
            // ASYNC PROGRESS READING
            if let Some(stdout) = child.stdout.take() {
                let task_id = task.id.clone();
                let state_clone = state.clone();
                
                // Spawn lightweight task to read lines asynchronously
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        if let Some(percent) = parse_rsync_percentage(&line) {
                            update_status(&task_id, &format!("SYNCING {}%", percent), &state_clone);
                        }
                    }
                });
            }

            // NON-BLOCKING WAIT
            match child.wait().await { // <--- The magic .await
                Ok(status) if status.success() => {
                    update_log(&task.id, "✅ Sync Successful", state);
                    update_status(&task.id, "IDLE", state);
                }
                Ok(status) => {
                    update_log(&task.id, &format!("❌ Failed (Exit {})", status), state);
                    update_status(&task.id, "ERROR", state);
                }
                Err(e) => {
                    update_log(&task.id, &format!("❌ Process Error: {}", e), state);
                    update_status(&task.id, "ERROR", state);
                }
            }
        }
        Err(e) => {
            update_log(&task.id, &format!("❌ Exec Error: {}", e), state);
            update_status(&task.id, "ERROR", state);
        }
    }
}


// --- PROGRESS PARSING ---
pub fn parse_rsync_percentage(line: &str) -> Option<u32> {
    // rsync --info=progress2 format: "   1,234,567  12%  123.45kB/s    0:00:12"
    let re = Regex::new(r"\s+(\d+)%").ok()?;
    re.captures(line)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u32>().ok())
}
