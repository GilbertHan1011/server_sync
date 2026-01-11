use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use chrono::Local;
use crate::protocol::{SyncTask, SyncMode, SyncDirection};
use crate::common::utils::is_valid_host;
use crate::server::state::{ServerState, update_status, update_log};
use crate::server::ssh::setup_askpass_script;

// --- LOG UTILITIES ---
pub fn get_task_log_path(task_id: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.sync_daemon_logs/{}.log", home, task_id)
}

async fn ensure_log_dir() -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let log_dir = format!("{}/.sync_daemon_logs", home);
    tokio::fs::create_dir_all(&log_dir).await
}

async fn rotate_log_if_needed(log_path: &str) -> std::io::Result<()> {
    const MAX_SIZE: u64 = 10 * 1024 * 1024; // 10MB
    
    if let Ok(metadata) = tokio::fs::metadata(log_path).await {
        if metadata.len() > MAX_SIZE {
            let old_path = format!("{}.old", log_path);
            // Overwrite old file if it exists
            let _ = tokio::fs::rename(log_path, &old_path).await;
        }
    }
    Ok(())
}

async fn write_log_line(log_path: &str, prefix: &str, line: &str) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_line = format!("[{}] {}: {}\n", timestamp, prefix, line);
    
    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await
    {
        let _ = file.write_all(log_line.as_bytes()).await;
    }
}

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
        let mut debounce_deadline: Option<tokio::time::Instant> = None;
        
        loop {
            // Determine how long to sleep in the select! block
            let timeout = if let Some(deadline) = debounce_deadline {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    Duration::from_millis(0) // Trigger immediately
                } else {
                    deadline - now
                }
            } else {
                Duration::from_secs(3600) // Sleep forever (almost) if nothing to do
            };

            tokio::select! {
                // 1. Kill Signal
                _ = rx_kill.recv() => {
                    update_log(&task_id, "Stopped", &state_handle);
                    break;
                }
                
                // 2. File Change Detected
                _ = rx_file.recv() => {
                    if debounce_deadline.is_none() {
                        update_status(&task_id, "PENDING...", &state_handle);
                        update_log(&task_id, "📝 Change detected, waiting 2s...", &state_handle);
                        // Start the 2-second timer
                        debounce_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                    }
                }

                // 3. Timeout / Timer Expiry
                _ = tokio::time::sleep(timeout), if debounce_deadline.is_some() => {
                    // Time is up! Run the sync.
                    debounce_deadline = None; // Reset timer
                    retry_run_rsync(&task, &state_handle).await;
                }
            }
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
    
    // Determine if source is a file or directory
    let source_arg = match tokio::fs::metadata(&task.source).await {
        Ok(metadata) => {
            if metadata.is_file() {
                // For files, don't append trailing slash
                task.source.clone()
            } else {
                // For directories, append trailing slash to sync contents
                format!("{}/", task.source)
            }
        }
        Err(_) => {
            // If metadata check fails, default to directory behavior (with slash)
            // This maintains backward compatibility
            format!("{}/", task.source)
        }
    };
    cmd.arg(source_arg);
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

    // Setup logging
    let log_path = get_task_log_path(&task.id);
    let _ = ensure_log_dir().await;
    let _ = rotate_log_if_needed(&log_path).await;
    let mut cmd = Command::new("rsync"); // Now tokio::process::Command
    
    let full_remote = format!("{}:{}", task.remote_host, task.remote_path);
    let local_path = task.source.clone();
    let source_arg = match tokio::fs::metadata(&task.source).await {
        Ok(metadata) => {
            if metadata.is_file() {
                task.source.clone()
            } else {
                format!("{}/", task.source)
            }
        }
    Err(_) => {
        format!("{}/", task.source)
        }
    };
    match task.sync_direction {
        SyncDirection::Push => {
            cmd.arg(&source_arg);
            cmd.arg(&full_remote);
        }
        SyncDirection::Pull => {
            cmd.arg(format!("{}/", full_remote)); // NOTE:We need to add a function to check whether remote is a file or not
            cmd.arg(&local_path);
        }
    }
    let sync_start_msg = format!("--- Starting Sync: {} -> {} ---", task.source, full_remote);
    write_log_line(&log_path, "SYNC", &sync_start_msg).await;

    update_status(&task.id, "SYNCING 0%", state);
    update_log(&task.id, "🔄 Starting sync...", state);

    let port = task.remote_port.unwrap_or(22);
    
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
    
    // Paths are already added above based on sync direction
    // (Push: local source -> remote destination, Pull: remote source -> local destination)
    
    // Stream output for real-time progress
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    match cmd.spawn() {
        Ok(mut child) => {
            let log_path_stdout = log_path.clone();
            let log_path_stderr = log_path.clone();
            
            // ASYNC STDOUT READING
            if let Some(stdout) = child.stdout.take() {
                let task_id = task.id.clone();
                let state_clone = state.clone();
                
                // Spawn lightweight task to read lines asynchronously
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stdout).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        // Write to log file
                        write_log_line(&log_path_stdout, "STDOUT", &line).await;
                        
                        // Parse progress for status updates
                        if let Some(percent) = parse_rsync_percentage(&line) {
                            update_status(&task_id, &format!("SYNCING {}%", percent), &state_clone);
                        }
                    }
                });
            }

            // ASYNC STDERR READING
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = reader.next_line().await {
                        write_log_line(&log_path_stderr, "STDERR", &line).await;
                    }
                });
            }

            // NON-BLOCKING WAIT
            match child.wait().await { // <--- The magic .await
                Ok(status) => {
                    let status_msg = format!("Process finished with: {}", status);
                    write_log_line(&log_path, "SYNC", &status_msg).await;
                    
                    if status.success() {
                        update_log(&task.id, "✅ Sync Successful", state);
                        update_status(&task.id, "IDLE", state);
                    } else {
                        update_log(&task.id, &format!("❌ Failed (Exit {})", status), state);
                        update_status(&task.id, "ERROR", state);
                    }
                }
                Err(e) => {
                    let error_msg = format!("Wait error: {}", e);
                    write_log_line(&log_path, "SYNC", &error_msg).await;
                    update_log(&task.id, &format!("❌ Process Error: {}", e), state);
                    update_status(&task.id, "ERROR", state);
                }
            }
        }
        Err(e) => {
            let error_msg = format!("Exec Failed: {}", e);
            write_log_line(&log_path, "SYNC", &error_msg).await;
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
