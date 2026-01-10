use std::sync::{Arc, Mutex};
use tokio::fs as tokio_fs;
use crate::protocol::{ClientRequest, ServerResponse, SyncTask};
use crate::server::state::ServerState;
use crate::server::ssh::{list_remote_dirs_ssh, get_remote_home_ssh};
use crate::server::worker::{self, spawn_sync_worker, run_dry_run, get_task_log_path};
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
        ClientRequest::ListLocalDirs(path) => {
            // BROWSER LOGIC: Read dir contents (ASYNC) - Now includes files and directories
            let p = if path.is_empty() {
                "/".to_string()
            } else {
                path
            };

            match tokio_fs::read_dir(&p).await {
                Ok(mut entries) => {
                    let mut items = Vec::new();
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        // Get file type (cheap, no extra syscall usually)
                        if let Ok(ft) = entry.file_type().await {
                            if let Ok(name) = entry.file_name().into_string() {
                                // Filter out hidden files (starting with '.')
                                if !name.starts_with('.') {
                                    if ft.is_dir() {
                                        // Append '/' to indicate directory
                                        items.push(format!("{}/", name));
                                    } else {
                                        // Files stay as is
                                        items.push(name);
                                    }
                                }
                            }
                        }
                    }
                    // Sort: Directories first, then files (both alphabetically)
                    items.sort_by(|a, b| {
                        let a_is_dir = a.ends_with('/');
                        let b_is_dir = b.ends_with('/');
                        if a_is_dir && !b_is_dir {
                            std::cmp::Ordering::Less
                        } else if !a_is_dir && b_is_dir {
                            std::cmp::Ordering::Greater
                        } else {
                            a.cmp(b)
                        }
                    });
                    ServerResponse::DirList(items)
                }
                Err(e) => ServerResponse::Error(format!("{}", e)),
            }
        }
        ClientRequest::ListRemoteDirs(host, port,path, password) => {
            // Use the host and password from the client request
            let dirs = list_remote_dirs_ssh(&host, port, &path, &password).await;
            ServerResponse::DirList(dirs)
        }
        ClientRequest::GetRemoteHome(host, port, password) => {
            let path = get_remote_home_ssh(&host, port, &password).await;
            ServerResponse::RemoteHome(path)
        }
        ClientRequest::CreateRemoteDir(host, port, path, password) => {
            match crate::server::ssh::create_remote_dir_ssh(&host, port, &path, &password).await {
                Ok(_) => ServerResponse::Ack,
                Err(e) => ServerResponse::Error(format!("Mkdir failed: {}", e)),
            }
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
        ClientRequest::RestartTask(id) => {
            // 1. Identify task and remove old stopper (Scope the lock to drop it quickly)
            let (task_data, old_stopper) = {
                let mut s: std::sync::MutexGuard<'_, ServerState> = state.lock().unwrap();
                let task = s.tasks.get(&id).cloned();
                let stopper = s.stoppers.remove(&id);
                (task, stopper)
            };

            if let Some(task) = task_data {
                // 2. Kill the old worker (Async - wait for it to die)
                if let Some(tx) = old_stopper {
                    let _ = tx.send(()).await;
                }

                // 3. Spawn a new worker
                let new_stopper = worker::spawn_sync_worker(task.clone(), state.clone());

                // 4. Re-acquire lock to store new stopper and update status
                let mut s = state.lock().unwrap();
                s.stoppers.insert(id.clone(), new_stopper);

                // Optional: Update status text immediately so user sees feedback
                if let Some(t) = s.tasks.get_mut(&id) {
                    t.status = "RESTARTING...".to_string();
                }

                ServerResponse::Ack
            } else {
                ServerResponse::Error(format!("Task {} not found", id))
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
        ClientRequest::GetTaskLog(id) => {
            let path = get_task_log_path(&id);
            match tokio_fs::read_to_string(&path).await {
                Ok(content) => ServerResponse::TaskLog(id, content),
                Err(_) => ServerResponse::TaskLog(id, "No logs found yet.".to_string()),
            }
        }
    }
}

