use std::sync::{Arc, Mutex};
use tokio::fs as tokio_fs;
use crate::protocol::{ClientRequest, ServerResponse, SyncTask};
use crate::server::state::ServerState;
use crate::server::ssh::{list_remote_dirs_ssh, get_remote_home_ssh};
use crate::server::worker::{spawn_sync_worker, run_dry_run};
use crate::server::state::save_tasks;
use crate::common::utils::is_valid_host;

pub async fn handle_request(
    req: ClientRequest,
    state: Arc<Mutex<ServerState>>,
) -> ServerResponse {
    match req {
        ClientRequest::GetState => {
            let s = state.lock().unwrap();
            let list: Vec<SyncTask> = s.tasks.values().cloned().collect();
            ServerResponse::State(list)
        }
        ClientRequest::GetRemoteHost => {
            let s = state.lock().unwrap();
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
                Err(e) => ServerResponse::Error(format!("{}", e)),
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
                let mut s = state.lock().unwrap();
                if !s.tasks.contains_key(&task_id) {
                    let task_clone = task.clone();
                    let stopper = spawn_sync_worker(task_clone, state.clone());
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
                let mut s = state.lock().unwrap();
                s.stoppers.remove(&id)
            };
            
            if let Some(tx) = stopper {
                let _ = tx.send(()).await; // Kill thread
            }
            
            let mut s = state.lock().unwrap();
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
                let s = state.lock().unwrap();
                s.tasks.get(&task_id).cloned()
            };
            
            if let Some(task) = task {
                let changes = run_dry_run(&task).await;
                ServerResponse::DryRunResult(changes)
            } else {
                ServerResponse::Error(format!("Task {} not found", task_id))
            }
        }
    }
}

