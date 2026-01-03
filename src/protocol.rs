use serde::{Deserialize, Serialize};

/// A single sync task
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncTask {
    pub id: String,          // Unique ID (e.g., "task_1")
    pub source: String,      // Local Path
    pub remote_host: String, // Remote host (e.g., "user@host")
    pub remote_path: String, // Remote path (e.g., "/path/to/destination")
    pub status: String,      // IDLE, SYNCING, ERROR, PENDING...
    pub last_log: String,
    pub poll_interval: u64,
}

/// Commands the Client sends to Server
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientRequest {
    GetState,                        // "Tell me everything"
    GetRemoteHost,                   // "What is the default remote host?"
    ListLocalDirs(String),           // "What folders are in /home/user?"
    ListRemoteDirs(String, String),  // "What folders are on remote server at path X?" (host, path)
    StartTask(SyncTask),             // "Start syncing this new pair"
    StopTask(String),                // "Stop task with ID 'X'"
}

/// Responses the Server sends back
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerResponse {
    State(Vec<SyncTask>),            // List of all active tasks
    RemoteHost(String),              // The default remote host
    DirList(Vec<String>),            // List of subdirectories
    Ack,                             // "Okay, done"
    Error(String),
}
