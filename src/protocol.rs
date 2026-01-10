use serde::{Deserialize, Serialize};

/// Sync mode determines how rsync handles file synchronization
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SyncMode {
    Mirror,      // --delete: Exact copy, deletes files on remote if missing on local
    AddOnly,     // No delete: Only adds/updates, never deletes
    SafeSync,    // --delete --backup --backup-dir: Moves deleted files to backup
    Update,      // --update: Only overwrites if local file is newer
}

/// A single sync task
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncTask {
    pub id: String,          // Unique ID (e.g., "task_1")
    pub source: String,      // Local Path
    pub remote_host: String, // Remote host (e.g., "user@host")
    pub remote_port: Option<u16>, // Remote port (e.g., 22)
    pub remote_path: String, // Remote path (e.g., "/path/to/destination")
    pub status: String,      // IDLE, SYNCING, ERROR, PENDING...
    pub last_log: String,
    pub poll_interval: u64,
    pub sync_mode: SyncMode, // How to sync (Mirror, AddOnly, etc.)
    pub compress: bool,      // Enable compression (-z flag)
    #[serde(default)]
    pub password: Option<String>, // Optional password for SSH authentication
}

/// Commands the Client sends to Server
#[derive(Debug, Serialize, Deserialize)]
pub enum ClientRequest {
    GetState,                        // "Tell me everything"
    ListLocalDirs(String),           // "What folders are in /home/user?"
    ListRemoteDirs(String, Option<u16>, String, Option<String>),  // "What folders are on remote server at path X?" (host, path, port, password)
    GetRemoteHome(String, Option<u16>, Option<String>), // "What is the home directory on remote server?" (host, port, password)
    StartTask(SyncTask),             // "Start syncing this new pair"
    StopTask(String),                // "Stop task with ID 'X'"
    RestartTask(String),
    DryRun(String),                  // "Show what would change for task X (dry run)"
    CreateRemoteDir(String,Option<u16>,String,Option<String>), // "Create a directory on remote server at path X?" (host, port, path, password)
}

/// Responses the Server sends back
#[derive(Debug, Serialize, Deserialize)]
pub enum ServerResponse {
    State(Vec<SyncTask>),            // List of all active tasks
    RemoteHome(String),              // The home directory on remote server
    DirList(Vec<String>),            // List of subdirectories
    Ack,                             // "Okay, done"
    Error(String),
    DryRunResult(Vec<String>),       // List of file changes from dry run
}
