use std::{collections::HashMap, fs, process::Command, sync::{Arc, Mutex}, time::Duration};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::io::{BufRead, BufReader};
use tokio::{net::UnixListener, io::{AsyncReadExt, AsyncWriteExt}, sync::mpsc};
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use server_sync::protocol::{SyncTask, ClientRequest, ServerResponse, SyncMode};
use serde::Deserialize;
use clap::Parser;
use daemonize::Daemonize;
use regex::Regex;

// --- CLI ARGUMENTS ---
#[derive(Parser)]
#[command(name = "server_sync")]
#[command(about = "File synchronization daemon server", long_about = None)]
struct ServerArgs {
    /// Path to configuration file
    #[arg(short, long, default_value = "server_config.yaml")]
    config: String,
    
    /// Path to log file (stdout if not provided)
    #[arg(short, long)]
    log: Option<String>,
    
    /// Run in foreground instead of daemonizing
    #[arg(short, long)]
    foreground: bool,
}

// --- PERSISTENCE FUNCTIONS ---
fn save_tasks(tasks: &HashMap<String, SyncTask>) -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = format!("{}/.sync_daemon_tasks.json", home);
    
    let task_list: Vec<SyncTask> = tasks.values().cloned().collect();
    let json = serde_json::to_string_pretty(&task_list)?;
    fs::write(path, json)?;
    Ok(())
}

fn load_tasks() -> HashMap<String, SyncTask> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = format!("{}/.sync_daemon_tasks.json", home);
    
    if let Ok(content) = fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<SyncTask>>(&content) {
            println!("Loaded {} tasks from {}", list.len(), path);
            return list.into_iter().map(|t| (t.id.clone(), t)).collect();
        }
    }
    HashMap::new()
}

// --- PROGRESS PARSING ---
fn parse_rsync_percentage(line: &str) -> Option<u32> {
    // rsync --info=progress2 format: "   1,234,567  12%  123.45kB/s    0:00:12"
    let re = Regex::new(r"\s+(\d+)%").ok()?;
    re.captures(line)
        .and_then(|cap| cap.get(1))
        .and_then(|m| m.as_str().parse::<u32>().ok())
}

// --- SERVER CONFIG ---
#[derive(Debug, Deserialize, Clone)]
struct ServerConfig {
    remote_host: String,
}

fn load_server_config(config_path: &str) -> ServerConfig {
    match fs::read_to_string(config_path) {
        Ok(content) => {
            match serde_yaml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Failed to parse {}: {}. Using default config.", config_path, e);
                    ServerConfig {
                        remote_host: "user@remote".to_string(),
                    }
                }
            }
        }
        Err(_) => {
            eprintln!("Config file {} not found. Using default config.", config_path);
            ServerConfig {
                remote_host: "user@remote".to_string(),
            }
        }
    }
}

// --- SERVER STATE ---
struct ServerState {
    tasks: HashMap<String, SyncTask>, // Store task data
    stoppers: HashMap<String, mpsc::Sender<()>>, // Channels to kill worker threads
    remote_host: String, // Default remote host (e.g., "user@host")
}

// --- REMOTE DIRECTORY LISTING ---
fn list_remote_dirs_ssh(remote_host: &str, path: &str) -> Vec<String> {
    // Default to current directory if empty
    let target_path = if path.is_empty() { "." } else { path };

    // Run: ssh user@host "ls -1F --group-directories-first /path"
    // Use ControlMaster for connection reuse
    let output = Command::new("ssh")
        .arg("-o").arg("ControlMaster=auto")
        .arg("-o").arg("ControlPath=~/.ssh/sockets/%r@%h-%p")
        .arg("-o").arg("ControlPersist=600")
        .arg(remote_host)
        .arg(format!("ls -1F --group-directories-first {}", target_path))
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            raw.lines()
                .filter(|line| line.ends_with('/')) // Only show directories
                .map(|line| line.replace("/", ""))   // Remove the trailing slash for display
                .collect()
        }
        Ok(o) => vec![format!("SSH Error: {}", String::from_utf8_lossy(&o.stderr))],
        Err(e) => vec![format!("Exec Error: {}", e)],
    }
}

// --- SYNC WORKER ---
// Spawns a dedicated thread for a single folder
fn spawn_sync_worker(
    task_data: SyncTask,
    state_handle: Arc<Mutex<ServerState>>,
) -> mpsc::Sender<()> {
    let (tx_kill, mut rx_kill) = mpsc::channel(1);
    let task_id = task_data.id.clone();
    let source = task_data.source.clone();
    let interval = task_data.poll_interval;
    let task = Arc::new(task_data); // Wrap in Arc for sharing

    tokio::spawn(async move {
        // 1. Initial Sync
        run_rsync(&task, &state_handle).await;

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
                    update_status(&task_id, "PENDING...", &state_handle);
                    update_log(&task_id, "📝 File change detected, debouncing...", &state_handle);
                    debounce = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                }
            }

            if let Some(time) = debounce {
                if tokio::time::Instant::now() >= time {
                    debounce = None;
                    run_rsync(&task, &state_handle).await;
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    tx_kill
}

async fn run_rsync(task: &SyncTask, state: &Arc<Mutex<ServerState>>) {
    update_status(&task.id, "SYNCING 0%", state);
    update_log(&task.id, "🔄 Starting sync...", state);

    let full_remote = format!("{}:{}", task.remote_host, task.remote_path);
    
    let mut cmd = Command::new("rsync");
    
    // Base flags: archive + verbose
    cmd.arg("-av");
    
    // Progress tracking
    cmd.arg("--info=progress2");
    cmd.arg("--no-inc-recursive");
    
    // SSH with ControlMaster for connection reuse
    cmd.arg("-e").arg("ssh -o ControlMaster=auto -o ControlPath=~/.ssh/sockets/%r@%h-%p -o ControlPersist=600");
    
    // Compression
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
            // Read stdout in real-time
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                let task_id = task.id.clone();
                let state_clone = state.clone();
                
                // Spawn blocking thread to read output
                std::thread::spawn(move || {
                    for line in reader.lines().flatten() {
                        if let Some(percent) = parse_rsync_percentage(&line) {
                            update_status(&task_id, &format!("SYNCING {}%", percent), &state_clone);
                        }
                    }
                });
            }
            
            // Wait for completion
            match child.wait() {
                Ok(status) if status.success() => {
                    update_log(&task.id, "✅ Sync Successful", state);
                    update_status(&task.id, "IDLE", state);
                }
                Ok(_) => {
                    update_log(&task.id, "❌ Sync Failed", state);
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

fn update_status(id: &str, status: &str, state: &Arc<Mutex<ServerState>>) {
    let mut s = state.lock().unwrap();
    if let Some(t) = s.tasks.get_mut(id) {
        t.status = status.to_string();
    }
}

fn update_log(id: &str, log: &str, state: &Arc<Mutex<ServerState>>) {
    let mut s = state.lock().unwrap();
    if let Some(t) = s.tasks.get_mut(id) {
        t.last_log = format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), log);
    }
}

fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

// --- MAIN ---
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command-line arguments
    let args = ServerArgs::parse();
    
    // Daemonize if not in foreground mode
    if !args.foreground {
        let mut daemon = Daemonize::new()
            .pid_file("/tmp/server_sync.pid")
            .working_directory(".");
        
        // Setup log file redirection if provided
        if let Some(ref log_path) = args.log {
            match fs::File::create(log_path) {
                Ok(log_file) => {
                    daemon = daemon
                        .stdout(log_file.try_clone()?)
                        .stderr(log_file);
                    eprintln!("Daemonizing with log file: {}", log_path);
                }
                Err(e) => {
                    eprintln!("Failed to create log file {}: {}", log_path, e);
                    eprintln!("Continuing without log file redirection");
                }
            }
        }
        
        eprintln!("Starting server daemon...");
        eprintln!("PID will be written to /tmp/server_sync.pid");
        
        match daemon.start() {
            Ok(_) => {
                // We're now in daemon mode
            }
            Err(e) => {
                eprintln!("Failed to daemonize: {}", e);
                return Err(e.into());
            }
        }
    } else {
        println!("Running in foreground mode");
    }
    
    let socket_path = get_socket_path();

    // Load configuration
    let config = load_server_config(&args.config);
    println!("Loaded config: remote_host = {}", config.remote_host);

    // Clean up old socket file if it exists
    if std::path::Path::new(&socket_path).exists() {
        fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    
    // Set permissions to 600 (owner read/write only)
    let mut perms = fs::metadata(&socket_path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&socket_path, perms)?;

    let state = Arc::new(Mutex::new(ServerState {
        tasks: HashMap::new(),
        stoppers: HashMap::new(),
        remote_host: config.remote_host,
    }));

    // Ensure SSH control sockets directory exists
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let socket_dir = format!("{}/.ssh/sockets", home);
    let _ = fs::create_dir_all(&socket_dir);

    // Load persisted tasks and spawn workers
    let loaded_tasks = load_tasks();
    for (task_id, task_data) in loaded_tasks {
        let tx_kill = spawn_sync_worker(task_data.clone(), state.clone());
        let mut state_lock = state.lock().unwrap();
        state_lock.tasks.insert(task_id.clone(), task_data);
        state_lock.stoppers.insert(task_id, tx_kill);
    }
    println!("Restored {} tasks from disk", state.lock().unwrap().tasks.len());

    println!("Multi-Sync Server running on {}", socket_path);
    println!("Press Ctrl+C to stop");

    loop {
        match listener.accept().await {
            Ok((mut socket, _)) => {
                let state_ref = state.clone();

                tokio::spawn(async move {
                    let mut buf = vec![0; 4096];

                    loop {
                        // Read Request
                        let n = match socket.read(&mut buf).await {
                            Ok(n) if n == 0 => break, // EOF
                            Ok(n) => n,
                            Err(_) => break,
                        };

                        let req: ClientRequest = match serde_json::from_slice(&buf[..n]) {
                            Ok(r) => r,
                            Err(e) => {
                                eprintln!("Failed to parse request: {}", e);
                                continue;
                            }
                        };

                        // Process Request
                        let resp = match req {
                            ClientRequest::GetState => {
                                let s = state_ref.lock().unwrap();
                                let list: Vec<SyncTask> = s.tasks.values().cloned().collect();
                                ServerResponse::State(list)
                            }
                            ClientRequest::GetRemoteHost => {
                                let s = state_ref.lock().unwrap();
                                ServerResponse::RemoteHost(s.remote_host.clone())
                            }
                            ClientRequest::ListLocalDirs(path) => {
                                // BROWSER LOGIC: Read dir contents
                                let p = if path.is_empty() {
                                    "/".to_string()
                                } else {
                                    path
                                };

                                match fs::read_dir(&p) {
                                    Ok(entries) => {
                                        let dirs: Vec<String> = entries
                                            .filter_map(|e| e.ok())
                                            .filter(|e| e.path().is_dir())
                                            .filter_map(|e| e.file_name().into_string().ok())
                                            .collect();
                                        ServerResponse::DirList(dirs)
                                    }
                                    Err(e) => ServerResponse::Error(e.to_string()),
                                }
                            }
                            ClientRequest::ListRemoteDirs(host, path) => {
                                // Use the host from the client request (user's TUI input)
                                let dirs = list_remote_dirs_ssh(&host, &path);
                                ServerResponse::DirList(dirs)
                            }
                            ClientRequest::StartTask(task) => {
                                let task_id = task.id.clone();
                                let mut s = state_ref.lock().unwrap();
                                if !s.tasks.contains_key(&task_id) {
                                    let task_clone = task.clone();
                                    let stopper = spawn_sync_worker(task_clone, state_ref.clone());
                                    s.tasks.insert(task_id.clone(), task);
                                    s.stoppers.insert(task_id, stopper);
                                    
                                    // Save tasks to disk
                                    if let Err(e) = save_tasks(&s.tasks) {
                                        eprintln!("Warning: Failed to save tasks: {}", e);
                                    }
                                    
                                    ServerResponse::Ack
                                } else {
                                    ServerResponse::Error(format!("Task {} already exists", task_id))
                                }
                            }
                            ClientRequest::StopTask(id) => {
                                let stopper = {
                                    let mut s = state_ref.lock().unwrap();
                                    s.stoppers.remove(&id)
                                };
                                
                                if let Some(tx) = stopper {
                                    let _ = tx.send(()).await; // Kill thread
                                }
                                
                                let mut s = state_ref.lock().unwrap();
                                s.tasks.remove(&id);
                                
                                // Save tasks to disk
                                if let Err(e) = save_tasks(&s.tasks) {
                                    eprintln!("Warning: Failed to save tasks: {}", e);
                                }
                                
                                ServerResponse::Ack
                            }
                        };

                        // Send Response
                        match serde_json::to_string(&resp) {
                            Ok(json) => {
                                if socket.write_all(json.as_bytes()).await.is_err() {
                                    break; // Client disconnected
                                }
                            }
                            Err(e) => {
                                eprintln!("Failed to serialize response: {}", e);
                                break;
                            }
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }
    }
}
