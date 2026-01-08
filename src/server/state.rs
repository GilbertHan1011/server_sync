use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use tokio::fs as tokio_fs;
use tokio::sync::mpsc;
use crate::protocol::SyncTask;

// --- SERVER STATE ---
pub struct ServerState {
    pub tasks: HashMap<String, SyncTask>, // Store task data
    pub stoppers: HashMap<String, mpsc::Sender<()>>, // Channels to kill worker threads
    pub remote_host: String, // Default remote host (e.g., "user@host")
}

// --- PERSISTENCE FUNCTIONS ---
pub async fn save_tasks(tasks: &HashMap<String, SyncTask>) -> std::io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = format!("{}/.sync_daemon_tasks.json", home);
    let tmp_path = format!("{}/.sync_daemon_tasks.json.tmp", home); // Temp file

    let task_list: Vec<SyncTask> = tasks.values().cloned().collect();
    let json = serde_json::to_string_pretty(&task_list)?;

    // 1. Write to temp file (ASYNC)
    tokio_fs::write(&tmp_path, json).await?;
    // 2. Atomic rename (overwrites old file instantly) (ASYNC)
    tokio_fs::rename(&tmp_path, &path).await?;
    
    Ok(())
}

pub fn load_tasks() -> HashMap<String, SyncTask> {
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

// --- STATE UPDATE HELPERS ---
pub fn update_status(id: &str, status: &str, state: &Arc<Mutex<ServerState>>) {
    let mut s = state.lock().unwrap();
    if let Some(t) = s.tasks.get_mut(id) {
        t.status = status.to_string();
    }
}

pub fn update_log(id: &str, log: &str, state: &Arc<Mutex<ServerState>>) {
    let mut s = state.lock().unwrap();
    if let Some(t) = s.tasks.get_mut(id) {
        t.last_log = format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), log);
    }
}