use std::fs;
use std::os::unix::fs::PermissionsExt;
use tokio::net::UnixListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use crate::protocol::ClientRequest;
use crate::server::state::{ServerState, load_tasks};
use crate::server::worker::spawn_sync_worker;
use crate::server::handler::handle_request;
use crate::common::utils::get_socket_path;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

/// Main server logic - runs the sync daemon
pub async fn run_server() -> anyhow::Result<()> {
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
        remote_host: "user@remote".to_string(), // Default, can be made configurable later
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

                        // Process Request using handler
                        let resp = handle_request(req, state_ref.clone()).await;

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
