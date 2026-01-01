use std::{collections::HashMap, fs, process::Command, sync::{Arc, Mutex}, time::Duration};
use std::os::unix::fs::PermissionsExt;
use tokio::{net::UnixListener, io::{AsyncReadExt, AsyncWriteExt}, sync::mpsc};
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use server_sync::protocol::{SyncTask, ClientRequest, ServerResponse};

// --- SERVER STATE ---
struct ServerState {
    tasks: HashMap<String, SyncTask>, // Store task data
    stoppers: HashMap<String, mpsc::Sender<()>>, // Channels to kill worker threads
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
    let remote = task_data.remote.clone();
    let interval = task_data.poll_interval;

    tokio::spawn(async move {
        // 1. Initial Sync
        run_rsync(&task_id, &source, &remote, &state_handle).await;

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
                    run_rsync(&task_id, &source, &remote, &state_handle).await;
                }
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    tx_kill
}

async fn run_rsync(id: &str, src: &str, remote: &str, state: &Arc<Mutex<ServerState>>) {
    update_status(id, "SYNCING...", state);
    update_log(id, "🔄 Starting sync...", state);

    let output = Command::new("rsync")
        .arg("-avz")
        .arg("--delete")
        .arg(format!("{}/", src))
        .arg(remote)
        .output();

    match output {
        Ok(o) if o.status.success() => {
            update_log(id, "✅ Sync Successful", state);
            update_status(id, "IDLE", state);
        }
        Ok(o) => {
            let err_msg = String::from_utf8_lossy(&o.stderr);
            update_log(id, &format!("❌ Sync Failed: {}", err_msg), state);
            update_status(id, "ERROR", state);
        }
        Err(e) => {
            update_log(id, &format!("❌ Exec Error: {}", e), state);
            update_status(id, "ERROR", state);
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
    let socket_path = get_socket_path();

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
    }));

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
                            ClientRequest::StartTask(task) => {
                                let task_id = task.id.clone();
                                let mut s = state_ref.lock().unwrap();
                                if !s.tasks.contains_key(&task_id) {
                                    let task_clone = task.clone();
                                    let stopper = spawn_sync_worker(task_clone, state_ref.clone());
                                    s.tasks.insert(task_id.clone(), task);
                                    s.stoppers.insert(task_id, stopper);
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
