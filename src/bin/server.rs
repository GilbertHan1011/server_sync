use std::{process::Command, time::Duration, sync::{Arc, Mutex}, fs};
use std::os::unix::fs::PermissionsExt;
use tokio::{net::UnixListener, io::{AsyncReadExt, AsyncWriteExt}};
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use server_sync::protocol::ServerState;

// --- CONFIG & STATE ---
#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    source_dir: String,
    remote_host: String,
    remote_dir: String,
    #[serde(default)]
    excludes: Vec<String>,
    #[serde(default = "default_poll")]
    poll_interval: u64,
}

fn default_poll() -> u64 { 30 }

// Internal State
struct AppState {
    logs: Vec<String>,
    status: String,
    sync_count: u32,
    config: AppConfig,
}

impl AppState {
    fn new(config: AppConfig) -> Self {
        Self {
            logs: vec![format!("[{}] Daemon Started - Monitoring: {}", 
                chrono::Local::now().format("%H:%M:%S"), config.source_dir)],
            status: "IDLE".to_string(),
            sync_count: 0,
            config,
        }
    }

    fn add_log(&mut self, msg: String) {
        let time = chrono::Local::now().format("%H:%M:%S");
        self.logs.push(format!("[{}] {}", time, msg));
        if self.logs.len() > 50 { self.logs.remove(0); }
    }

    fn to_server_state(&self) -> ServerState {
        ServerState {
            logs: self.logs.clone(),
            status: self.status.clone(),
            sync_count: self.sync_count,
        }
    }
}

// --- SYNC LOGIC ---
async fn run_rsync(state: Arc<Mutex<AppState>>) {
    let config = {
        let mut s = state.lock().unwrap();
        s.status = "SYNCING...".to_string();
        s.add_log("🔄 Starting sync...".to_string());
        s.config.clone()
    };

    let mut cmd = Command::new("rsync");
    cmd.arg("-avz").arg("--delete");
    for exc in &config.excludes {
        cmd.arg(format!("--exclude={}", exc));
    }
    cmd.arg(format!("{}/", config.source_dir)); 
    cmd.arg(format!("{}:{}", config.remote_host, config.remote_dir));

    let output = cmd.output();

    let mut s = state.lock().unwrap();
    match output {
        Ok(out) if out.status.success() => {
            s.add_log("✅ Sync Successful".to_string());
            s.sync_count += 1;
            s.status = "IDLE".to_string();
        }
        Ok(out) => {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            s.add_log(format!("❌ Sync Failed: {}", err_msg));
            s.status = "ERROR".to_string();
        }
        Err(e) => {
            s.add_log(format!("❌ Exec Error: {}", e));
            s.status = "ERROR".to_string();
        }
    }
}

fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Load Config
    let config_str = fs::read_to_string("config.yaml")?;
    let config: AppConfig = serde_yaml::from_str(&config_str)?;
    let path_to_watch = fs::canonicalize(&config.source_dir)?;

    // 2. Initialize State
    let app_state = Arc::new(Mutex::new(AppState::new(config.clone())));

    // 3. Initial Sync
    {
        let s = app_state.clone();
        tokio::spawn(async move { 
            run_rsync(s).await; 
        });
    }

    // 4. File Watcher Setup
    let (tx_watch, mut rx_watch) = tokio::sync::mpsc::channel(100);
    let interval = config.poll_interval;
    let path_clone = path_to_watch.clone();

    std::thread::spawn(move || {
        let (wt_tx, wt_rx) = std::sync::mpsc::channel();
        let notify_config = Config::default()
            .with_poll_interval(Duration::from_secs(interval));
        
        let mut watcher = match PollWatcher::new(wt_tx, notify_config) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("PollWatcher Error: {:?}", e);
                return;
            }
        };
        
        if let Err(e) = watcher.watch(&path_clone, RecursiveMode::Recursive) {
            eprintln!("PollWatcher Watch Error: {:?}", e);
            return;
        }

        for res in wt_rx {
            match res {
                Ok(_) => {
                    let _ = tx_watch.blocking_send(());
                }
                Err(e) => {
                    eprintln!("Watch error: {:?}", e);
                }
            }
        }
    });

    // 5. Sync Trigger Logic (Debouncing)
    let state_sync = app_state.clone();
    tokio::spawn(async move {
        let mut debounce = None;
        loop {
            // Check for file events
            match rx_watch.try_recv() {
                Ok(_) => {
                    let mut s = state_sync.lock().unwrap();
                    if s.status != "SYNCING..." {
                        s.status = "PENDING...".to_string();
                        s.add_log("📝 File change detected, debouncing...".to_string());
                    }
                    debounce = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                }
                Err(_) => {}
            }
            
            // Check Debounce Timer
            if let Some(time) = debounce {
                if tokio::time::Instant::now() >= time {
                    debounce = None;
                    run_rsync(state_sync.clone()).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // 6. Unix Socket Server
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

    println!("Daemon running on socket: {}", socket_path);
    println!("Press Ctrl+C to stop");

    // Handle client connections
    loop {
        match listener.accept().await {
            Ok((mut stream, _)) => {
                let state_conn = app_state.clone();
                
                tokio::spawn(async move {
                    let mut buf = vec![0; 1024];
                    
                    loop {
                        // Send state to client
                        let snapshot = {
                            let s = state_conn.lock().unwrap();
                            s.to_server_state()
                        };
                        
                        let json = match serde_json::to_string(&snapshot) {
                            Ok(j) => j,
                            Err(e) => {
                                eprintln!("Serialization error: {}", e);
                                break;
                            }
                        };
                        
                        // Send length prefix + JSON
                        let data = format!("{}\n", json);
                        if stream.write_all(data.as_bytes()).await.is_err() {
                            break; // Client disconnected
                        }
                        
                        // Try to read command from client (non-blocking check)
                        match stream.try_read(&mut buf) {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                // Parse command
                                if let Ok(cmd_str) = String::from_utf8(buf[..n].to_vec()) {
                                    if cmd_str.trim() == "F" || cmd_str.trim() == "ForceSync" {
                                        let s = state_conn.clone();
                                        tokio::spawn(async move {
                                            run_rsync(s).await;
                                        });
                                    }
                                }
                            }
                            Err(_) => {
                                // No data available, continue
                            }
                        }
                        
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                });
            }
            Err(e) => {
                eprintln!("Accept error: {}", e);
            }
        }
    }
}

