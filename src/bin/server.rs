use std::{collections::HashMap, fs, sync::{Arc, Mutex}, time::Duration};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{net::UnixListener, io::{AsyncReadExt, AsyncWriteExt}, sync::mpsc, fs as tokio_fs};
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


// --- MAIN ---
// Synchronous main - daemonize BEFORE tokio runtime
fn main() -> anyhow::Result<()> {
    // Parse command-line arguments
    let args = ServerArgs::parse();
    
    // Daemonize if not in foreground mode (BEFORE tokio runtime)
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
                // We're now in daemon mode - NOW create tokio runtime
            }
            Err(e) => {
                eprintln!("Failed to daemonize: {}", e);
                return Err(e.into());
            }
        }
    } else {
        println!("Running in foreground mode");
    }
    
    // Create tokio runtime AFTER daemonization (if any)
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_server(args))
}

// Async server logic - runs AFTER daemonization
async fn run_server(args: ServerArgs) -> anyhow::Result<()> {
    
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
                                // BROWSER LOGIC: Read dir contents (ASYNC)
                                let p = if path.is_empty() {
                                    "/".to_string()
                                } else {
                                    path
                                };

                                match tokio_fs::read_dir(&p).await {
                                    Ok(mut entries) => {
                                        let mut dirs = Vec::new();
                                        while let Ok(Some(entry)) = entries.next_entry().await {
                                            if let Ok(metadata) = entry.metadata().await {
                                                if metadata.is_dir() {
                                                    if let Ok(name) = entry.file_name().into_string() {
                                                        dirs.push(name);
                                                    }
                                                }
                                            }
                                        }
                                        ServerResponse::DirList(dirs)
                                    }
                                    Err(e) => ServerResponse::Error(e.to_string()),
                                }
                            }
                            ClientRequest::ListRemoteDirs(host, path, password) => {
                                // Use the host and password from the client request
                                let dirs = list_remote_dirs_ssh(&host, &path, &password).await;
                                ServerResponse::DirList(dirs)
                            }
                            ClientRequest::GetRemoteHome(host, password) => {
                                let path = get_remote_home_ssh(&host, &password).await;
                                ServerResponse::RemoteHome(path)
                            }
                            ClientRequest::StartTask(task) => {
                                // SECURITY: Validate host before starting task
                                if !is_valid_host(&task.remote_host) {
                                    ServerResponse::Error("Invalid remote host format".to_string())
                                } else {
                                    let task_id = task.id.clone();
                                    let mut s = state_ref.lock().unwrap();
                                    if !s.tasks.contains_key(&task_id) {
                                        let task_clone = task.clone();
                                        let stopper = spawn_sync_worker(task_clone, state_ref.clone());
                                        s.tasks.insert(task_id.clone(), task);
                                        s.stoppers.insert(task_id, stopper);
                                        
                                        // Save tasks to disk (ASYNC - spawn to avoid blocking)
                                        let tasks_to_save = s.tasks.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) = save_tasks(&tasks_to_save).await {
                                                eprintln!("Warning: Failed to save tasks: {}", e);
                                            }
                                        });
                                        
                                        ServerResponse::Ack
                                    } else {
                                        ServerResponse::Error(format!("Task {} already exists", task_id))
                                    }
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
                                
                                // Save tasks to disk (ASYNC - spawn to avoid blocking)
                                let tasks_to_save = s.tasks.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = save_tasks(&tasks_to_save).await {
                                        eprintln!("Warning: Failed to save tasks: {}", e);
                                    }
                                });
                                
                                ServerResponse::Ack
                            }
                            ClientRequest::DryRun(task_id) => {
                                let task = {
                                    let s = state_ref.lock().unwrap();
                                    s.tasks.get(&task_id).cloned()
                                };
                                
                                if let Some(task) = task {
                                    let changes = run_dry_run(&task).await;
                                    ServerResponse::DryRunResult(changes)
                                } else {
                                    ServerResponse::Error(format!("Task {} not found", task_id))
                                }
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
    // Unreachable - loop runs forever, but required for return type
    #[allow(unreachable_code)]
    Ok(())
}
