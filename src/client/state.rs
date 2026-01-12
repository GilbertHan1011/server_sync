use crate::protocol::{SyncTask, SyncMode, SyncDirection};

// --- UI STATE ---
#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Dashboard,
    LocalBrowser,     // Browse local directories (source selection)
    RemoteHostInput,  // Edit remote host before browsing remote
    RemotePortInput,  // Edit remote port before browsing remote
    HostSelect,
    PasswordInput,    // Enter SSH password (optional)
    SyncModeSelect,   // Select sync mode (Mirror, AddOnly, SafeSync, Update)
    CreateRemoteDir,  // Create a remote directory
    CreateLocalDir,   // Create a local directory
    RemoteBrowser,    // Browse remote directories (destination selection)
    DryRunView,       // Display dry run results
    LogView,          // Display task logs
}

pub struct App {
    pub mode: AppMode,
    pub tasks: Vec<SyncTask>,
    pub dashboard_selected_idx : usize,
    // Browser State
    pub current_path: String,
    pub dir_entries: Vec<String>,
    pub selected_idx: usize,

    // Task Creation State
    pub pending_source: String,        // Selected local path before remote browsing
    pub remote_current_path: String,   // Current path in remote browser
    pub pending_remote_host: String,   // Remote host (e.g., "user@host")
    pub pending_remote_port: Option<u16>, // Remote port (e.g., 22)
    // Remote Host Input State
    pub input_remote_host: String,     // User's edited remote host
    pub input_remote_port: String, // User's edited remote port
    pub input_cursor_pos: usize,       // Cursor position in input field
    // Password Input State
    pub pending_password: Option<String>, // Stores the final confirmed password
    pub input_password: String,           // Buffer for typing password
    pub show_password: bool,              // Toggle to show/hide characters
    // Sync Mode Selection State
    pub pending_sync_direction: SyncDirection,   // Selected sync direction
    pub pending_sync_mode: SyncMode,   // Selected sync mode
    pub pending_compress: bool,        // Compression enabled
    pub sync_mode_selected_idx: usize, // 0-3 for the 4 modes
    pub sync_direction_selected_idx: usize, // 0-1 for the 2 directions
    // Dry Run State
    pub dry_run_results: Vec<String>,  // Results from dry run
    pub dry_run_task_id: String,       // Which task was dry-run
    pub dry_run_scroll: usize,         // Scroll position in dry run view
    // Log Viewer State
    pub view_task_log: String,         // Content of the log
    pub view_log_scroll: usize,        // Scroll position
    pub view_log_task_id: String,      // Which task's log is being viewed
    pub view_log_last_fetch: std::time::Instant, // For auto-refresh timing
    // Saved Host Names
    pub saved_hosts: Vec<String>,
    pub host_list_idx: usize,
    pub is_editing_host: bool,
    pub input_new_dir: String,
    // Server Status
    pub server_status: Option<bool>, // None = unknown, Some(true) = running, Some(false) = stopped
    pub server_status_last_check: std::time::Instant,
}
