use std::path::Path;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::client::state::{App, AppMode};
use crate::client::network::send_req;
use crate::client::config::{load_hosts, save_hosts};
use crate::protocol::{ClientRequest, ServerResponse, SyncMode, SyncTask, SyncDirection};
use crate::common::daemon;

#[derive(Debug, Clone, PartialEq)]
pub enum HandlerResult {
    Continue,
    Quit,
}

pub fn handle_key_event(key: KeyEvent, app: &mut App) -> HandlerResult {
    match app.mode {
        AppMode::Dashboard => handle_dashboard_keys(key, app),
        AppMode::LocalBrowser => handle_local_browser_keys(key, app),
        AppMode::HostSelect => handle_host_select_keys(key, app),
        AppMode::RemoteHostInput => handle_remote_host_input_keys(key, app),
        AppMode::RemotePortInput => handle_remote_port_input_keys(key, app),
        AppMode::PasswordInput => handle_password_input_keys(key, app),
        AppMode::SyncModeSelect => handle_sync_mode_select_keys(key, app),
        AppMode::RemoteBrowser => handle_remote_browser_keys(key, app),
        AppMode::DryRunView => handle_dry_run_view_keys(key, app),
        AppMode::CreateRemoteDir => handle_remote_mkdir_input_keys(key, app),
        AppMode::LogView => handle_log_view_keys(key, app),
        AppMode::CreateLocalDir => handle_local_mkdir_input_keys(key, app),
    }
}

fn handle_dashboard_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    // Check for Ctrl+R to restart server
    if key.modifiers.contains(KeyModifiers::CONTROL) && (key.code == KeyCode::Char('r') || key.code == KeyCode::Char('R')) {
        if let Err(e) = daemon::kill_server() {
            eprintln!("Error stopping server: {}", e);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Err(e) = daemon::spawn_server() {
            eprintln!("Error starting server: {}", e);
        } else {
            app.server_status = Some(true);
            app.server_status_last_check = std::time::Instant::now();
        }
        return HandlerResult::Continue;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => HandlerResult::Quit,
        KeyCode::Down => {
            if app.dashboard_selected_idx < app.tasks.len().saturating_sub(1) {
                app.dashboard_selected_idx += 1;
                update_dashboard_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.dashboard_selected_idx > 0 {
                app.dashboard_selected_idx -= 1;
                update_dashboard_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            // Set direction to Push and enter LocalBrowser Mode
            app.pending_sync_direction = SyncDirection::Push;
            app.sync_direction_selected_idx = 0;
            app.mode = AppMode::LocalBrowser;
            match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                ServerResponse::DirList(d) => {
                    app.dir_entries = d;
                    app.dir_entries.insert(0, "..".to_string());
                    app.selected_idx = 0;
                    update_browser_scroll(app);
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error listing dirs: {}", e);
                    app.mode = AppMode::Dashboard;
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        KeyCode::Char('p') | KeyCode::Char('P') => {
            // Set direction to Pull and always show host selection
            app.pending_sync_direction = SyncDirection::Pull;
            app.sync_direction_selected_idx = 1;
            app.saved_hosts = load_hosts();
            
            // Always allow user to select remote server
            if app.saved_hosts.is_empty() {
                // No saved hosts, go to host input
                app.mode = AppMode::RemoteHostInput;
                app.input_remote_host = String::new();
                app.input_cursor_pos = 0;
                app.is_editing_host = false;
            } else {
                // Have saved hosts, show selection screen
                app.mode = AppMode::HostSelect;
                app.host_list_idx = 0;
            }
            HandlerResult::Continue
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if !app.tasks.is_empty() {
                // Safety check for bounds
                if app.dashboard_selected_idx >= app.tasks.len() {
                    app.dashboard_selected_idx = app.tasks.len().saturating_sub(1);
                }
                
                // Delete the SELECTED task
                if let Some(t) = app.tasks.get(app.dashboard_selected_idx) {
                    send_req(ClientRequest::StopTask(t.id.clone()));
                }
                
                // Adjust selection if we deleted the last item
                if app.dashboard_selected_idx > 0 && app.dashboard_selected_idx >= app.tasks.len().saturating_sub(1) {
                     app.dashboard_selected_idx -= 1;
                }
                update_dashboard_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if !app.tasks.is_empty() {
                // Safety check
                if app.dashboard_selected_idx >= app.tasks.len() {
                    app.dashboard_selected_idx = app.tasks.len().saturating_sub(1);
                }

                // Dry Run the SELECTED task
                if let Some(target_task) = app.tasks.get(app.dashboard_selected_idx) {
                    match send_req(ClientRequest::DryRun(target_task.id.clone())) {
                        ServerResponse::DryRunResult(changes) => {
                            app.dry_run_results = changes;
                            app.dry_run_task_id = target_task.id.clone();
                            app.dry_run_scroll = 0;
                            app.mode = AppMode::DryRunView;
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Dry run error: {}", e);
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            if !app.tasks.is_empty() {
                // Safety check for index
                if app.dashboard_selected_idx >= app.tasks.len() {
                    app.dashboard_selected_idx = app.tasks.len().saturating_sub(1);
                }

                if let Some(target_task) = app.tasks.get(app.dashboard_selected_idx) {
                    // Send Restart Request
                    match send_req(ClientRequest::RestartTask(target_task.id.clone())) {
                        ServerResponse::Ack => {
                            // Optional: Force an immediate refresh of list to see "RESTARTING"
                            // You can call ListTasks logic here or just wait for next tick
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Failed to restart: {}", e);
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            if !app.tasks.is_empty() {
                // Safety clamp
                if app.dashboard_selected_idx >= app.tasks.len() {
                    app.dashboard_selected_idx = app.tasks.len().saturating_sub(1);
                }
                
                if let Some(t) = app.tasks.get(app.dashboard_selected_idx) {
                    let tid = t.id.clone();
                    match send_req(ClientRequest::GetTaskLog(tid.clone())) {
                        ServerResponse::TaskLog(_, content) => {
                            app.view_task_log = content;
                            app.view_log_task_id = tid.clone();
                            app.view_log_scroll = 0;
                            app.view_log_last_fetch = std::time::Instant::now();
                            app.mode = AppMode::LogView;
                            
                            // Auto-scroll to bottom (simple heuristic: huge number)
                            let lines = app.view_task_log.lines().count();
                            if lines > 20 {
                                app.view_log_scroll = lines.saturating_sub(20);
                            }
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_local_browser_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            // In Pull mode, if we came from RemoteBrowser, go back there
            // Otherwise, go to Dashboard
            match app.pending_sync_direction {
                SyncDirection::Pull => {
                    // Check if we have remote info (meaning we came from RemoteBrowser)
                    if !app.pending_remote_host.is_empty() {
                        app.mode = AppMode::RemoteBrowser;
                    } else {
                        app.mode = AppMode::Dashboard;
                    }
                }
                SyncDirection::Push => {
                    app.mode = AppMode::Dashboard;
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.selected_idx < app.dir_entries.len().saturating_sub(1) {
                app.selected_idx += 1;
                update_browser_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.selected_idx > 0 {
                app.selected_idx -= 1;
                update_browser_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            // Navigate into directory (only if it's ".." or ends with "/")
            let selected = &app.dir_entries[app.selected_idx];
            
            if selected == ".." {
                let new_path = Path::new(&app.current_path)
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .display()
                    .to_string();
                app.current_path = new_path.replace("//", "/");
                request_local_list(app);
                update_browser_scroll(app);
            } else if selected.ends_with('/') {
                // It's a directory! Enter it.
                // Remove the trailing slash for the path construction
                let clean_name = selected.trim_end_matches('/');
                
                let new_path = if app.current_path == "/" {
                    format!("/{}", clean_name)
                } else {
                    format!("{}/{}", app.current_path, clean_name)
                };

                app.current_path = new_path.replace("//", "/");
                request_local_list(app);
                update_browser_scroll(app);
            }
            // If it's a file (no "/" suffix), do nothing
            HandlerResult::Continue
        }
        KeyCode::Char(' ') => {
            // Select the file or folder
            let selected_item = &app.dir_entries[app.selected_idx];
            
            let final_path = if selected_item == ".." {
                app.current_path.clone()
            } else {
                // Strip slash if it's a directory so rsync treats it consistently
                let clean_name = selected_item.trim_end_matches('/');
                
                if app.current_path == "/" {
                    format!("/{}", clean_name)
                } else {
                    format!("{}/{}", app.current_path, clean_name)
                }
            };

            match app.pending_sync_direction {
                SyncDirection::Push => {
                    // Push mode: local is source, need to collect remote info
                    app.pending_source = final_path;
                    app.saved_hosts = load_hosts();

                    if app.saved_hosts.is_empty() {
                        app.mode = AppMode::RemoteHostInput;
                        app.input_remote_host = String::new();
                        app.is_editing_host = false;
                    } else {
                        app.mode = AppMode::HostSelect;
                        app.host_list_idx = 0;
                    }
                }
                SyncDirection::Pull => {
                    // Pull mode: local is destination, remote is already selected
                    // Store local path as source (destination in Pull mode)
                    app.pending_source = final_path;
                    // Go directly to SyncModeSelect
                    app.mode = AppMode::SyncModeSelect;
                    app.sync_mode_selected_idx = 0;
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.mode = AppMode::CreateLocalDir;
            app.input_new_dir.clear();
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

// Helper to reduce code duplication
fn request_local_list(app: &mut App) {
    match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
        ServerResponse::DirList(d) => {
            app.dir_entries = d;
            app.dir_entries.insert(0, "..".to_string());
            app.selected_idx = 0;
            update_browser_scroll(app);
        }
        ServerResponse::Error(e) => {
            eprintln!("Error: {}", e);
        }
        _ => {}
    }
}

fn handle_host_select_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::LocalBrowser;
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.host_list_idx < app.saved_hosts.len().saturating_sub(1) {
                app.host_list_idx += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.host_list_idx > 0 {
                app.host_list_idx -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            if !app.saved_hosts.is_empty() {
                app.pending_remote_host = app.saved_hosts[app.host_list_idx].clone();
                app.mode = AppMode::PasswordInput;
                app.input_password.clear();
                app.show_password = false;
            }
            HandlerResult::Continue
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            app.mode = AppMode::RemoteHostInput;
            app.input_remote_host = String::new();
            app.input_cursor_pos = 0;
            app.is_editing_host = false;
            HandlerResult::Continue
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if !app.saved_hosts.is_empty() {
                app.mode = AppMode::RemoteHostInput;
                app.input_remote_host = app.saved_hosts[app.host_list_idx].clone();
                app.input_cursor_pos = app.input_remote_host.len();
                app.is_editing_host = true;
            }
            HandlerResult::Continue
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            if !app.saved_hosts.is_empty() {
                app.saved_hosts.remove(app.host_list_idx);
                save_hosts(&app.saved_hosts);
                if app.host_list_idx >= app.saved_hosts.len() && !app.saved_hosts.is_empty() {
                    app.host_list_idx = app.saved_hosts.len() - 1;
                }
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_remote_host_input_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            if !app.saved_hosts.is_empty() {
                app.mode = AppMode::HostSelect;
            } else {
                // Go back based on direction
                match app.pending_sync_direction {
                    SyncDirection::Push => app.mode = AppMode::LocalBrowser,
                    SyncDirection::Pull => app.mode = AppMode::Dashboard,
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            let input = app.input_remote_host.trim();
            
            // Logic to handle "user@host:port" format
            // We split by the last ':' and check if the remainder is a number
            let (host, port_str) = if let Some(idx) = input.rfind(':') {
                let (h, p_str) = input.split_at(idx);
                let potential_port = &p_str[1..]; // Skip the ':'
                
                // Only treat as port if it parses successfully (avoids breaking IPv6)
                if potential_port.parse::<u16>().is_ok() {
                    (h, Some(potential_port))
                } else {
                    (input, None)
                }
            } else {
                (input, None)
            };

            app.pending_remote_host = host.to_string();
            
            // Setup next step: Port Input
            app.mode = AppMode::RemotePortInput; 
            
            if let Some(p) = port_str {
                app.input_remote_port = p.to_string(); // Use typed port
            } else {
                app.input_remote_port = "22".to_string(); // Default
            }
            
            app.input_cursor_pos = app.input_remote_port.len(); 
            HandlerResult::Continue
        }
        KeyCode::Left => {
            if app.input_cursor_pos > 0 {
                app.input_cursor_pos -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Right => {
            if app.input_cursor_pos < app.input_remote_host.len() {
                app.input_cursor_pos += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Backspace => {
            if app.input_cursor_pos > 0 {
                app.input_remote_host.remove(app.input_cursor_pos - 1);
                app.input_cursor_pos -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Delete => {
            if app.input_cursor_pos < app.input_remote_host.len() {
                app.input_remote_host.remove(app.input_cursor_pos);
            }
            HandlerResult::Continue
        }
        KeyCode::Home => {
            app.input_cursor_pos = 0;
            HandlerResult::Continue
        }
        KeyCode::End => {
            app.input_cursor_pos = app.input_remote_host.len();
            HandlerResult::Continue
        }
        KeyCode::Char(c) => {
            app.input_remote_host.insert(app.input_cursor_pos, c);
            app.input_cursor_pos += 1;
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_remote_port_input_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::RemoteHostInput;
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            if let Ok(p) = app.input_remote_port.trim().parse::<u16>() {
                app.pending_remote_port = Some(p);
            } else {
                app.pending_remote_port = Some(22); // Fallback if invalid
            }
            app.mode = AppMode::PasswordInput; // Next step
            app.input_password.clear();
            app.show_password = false;
            HandlerResult::Continue
        }
        KeyCode::Backspace => {
            if !app.input_remote_port.is_empty() {
                app.input_remote_port.pop();
                if app.input_cursor_pos > 0 {
                    app.input_cursor_pos -= 1;
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Left => {
            if app.input_cursor_pos > 0 {
                app.input_cursor_pos -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Right => {
            if app.input_cursor_pos < app.input_remote_port.len() {
                app.input_cursor_pos += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Home => {
            app.input_cursor_pos = 0;
            HandlerResult::Continue
        }
        KeyCode::End => {
            app.input_cursor_pos = app.input_remote_port.len();
            HandlerResult::Continue
        }
        KeyCode::Char(c) => {
            if c.is_ascii_digit() { // Only allow numbers
                app.input_remote_port.insert(app.input_cursor_pos, c);
                app.input_cursor_pos += 1;
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_password_input_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::RemotePortInput;
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            if app.input_password.trim().is_empty() {
                app.pending_password = None;
            } else {
                app.pending_password = Some(app.input_password.clone());
            }
            
            match app.pending_sync_direction {
                SyncDirection::Push => {
                    // Push mode: go to SyncModeSelect (after local source is selected)
                    app.mode = AppMode::SyncModeSelect;
                    app.sync_mode_selected_idx = 0;
                }
                SyncDirection::Pull => {
                    // Pull mode: go to RemoteBrowser to select remote source
                    app.mode = AppMode::RemoteBrowser;
                    match send_req(ClientRequest::GetRemoteHome(
                        app.pending_remote_host.clone(),
                        app.pending_remote_port,
                        app.pending_password.clone()
                    )) {
                        ServerResponse::RemoteHome(path) => {
                            app.remote_current_path = path;
                        }
                        _ => {
                            app.remote_current_path = "/".to_string();
                        }
                    }
                    
                    match send_req(ClientRequest::ListRemoteDirs(
                        app.pending_remote_host.clone(),
                        app.pending_remote_port,
                        app.remote_current_path.clone(),
                        app.pending_password.clone()
                    )) {
                        ServerResponse::DirList(d) => {
                            app.dir_entries = d;
                            app.dir_entries.insert(0, "..".to_string());
                            app.selected_idx = 0;
                            update_browser_scroll(app);
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Error listing remote dirs: {}", e);
                            app.mode = AppMode::PasswordInput;
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Tab => {
            app.show_password = !app.show_password;
            HandlerResult::Continue
        }
        KeyCode::Backspace => {
            app.input_password.pop();
            HandlerResult::Continue
        }
        KeyCode::Char(c) => {
            app.input_password.push(c);
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_sync_mode_select_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            // Go back based on direction
            match app.pending_sync_direction {
                SyncDirection::Push => {
                    app.mode = AppMode::PasswordInput;
                }
                SyncDirection::Pull => {
                    app.mode = AppMode::LocalBrowser;
                }
            }
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.sync_mode_selected_idx < 3 {
                app.sync_mode_selected_idx += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.sync_mode_selected_idx > 0 {
                app.sync_mode_selected_idx -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Char(' ') => {
            app.pending_compress = !app.pending_compress;
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            app.pending_sync_mode = match app.sync_mode_selected_idx {
                0 => SyncMode::Mirror,
                1 => SyncMode::AddOnly,
                2 => SyncMode::SafeSync,
                3 => SyncMode::Update,
                _ => SyncMode::Mirror,
            };
            
            match app.pending_sync_direction {
                SyncDirection::Push => {
                    // Push mode: go to RemoteBrowser to select remote destination
                    app.mode = AppMode::RemoteBrowser;
                    match send_req(ClientRequest::GetRemoteHome(
                        app.pending_remote_host.clone(),
                        app.pending_remote_port,
                        app.pending_password.clone()
                    )) {
                        ServerResponse::RemoteHome(path) => {
                            app.remote_current_path = path;
                        }
                        _ => {
                            app.remote_current_path = "/".to_string();
                        }
                    }
                    
                    match send_req(ClientRequest::ListRemoteDirs(
                        app.pending_remote_host.clone(),
                        app.pending_remote_port,
                        app.remote_current_path.clone(),
                        app.pending_password.clone()
                    )) {
                        ServerResponse::DirList(d) => {
                            app.dir_entries = d;
                            app.dir_entries.insert(0, "..".to_string());
                            app.selected_idx = 0;
                            update_browser_scroll(app);
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Error listing remote dirs: {}", e);
                            app.mode = AppMode::SyncModeSelect;
                        }
                        _ => {}
                    }
                }
                SyncDirection::Pull => {
                    // Pull mode: both source (remote) and destination (local) are already selected
                    // Create task directly
                    // In Pull mode: source = local (destination), remote_path = remote (source)
                    let task_id = format!("task_{}", app.tasks.len() + 1);
                    
                    let new_task = SyncTask {
                        id: task_id,
                        source: app.pending_source.clone(), // Local destination
                        remote_host: app.pending_remote_host.clone(),
                        remote_port: app.pending_remote_port,
                        remote_path: app.remote_current_path.clone(), // Remote source (stored when remote was selected)
                        status: "STARTING".to_string(),
                        last_log: "Created".to_string(),
                        poll_interval: 5,
                        sync_mode: app.pending_sync_mode.clone(),
                        compress: app.pending_compress,
                        sync_direction: app.pending_sync_direction.clone(),
                    };
                    
                    match send_req(ClientRequest::StartTask(new_task, app.pending_password.clone())) {
                        ServerResponse::Ack => {
                            app.mode = AppMode::Dashboard;
                            app.pending_source.clear();
                            app.remote_current_path.clear();
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Error starting task: {}", e);
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_remote_browser_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Dashboard;
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.selected_idx < app.dir_entries.len().saturating_sub(1) {
                app.selected_idx += 1;
                update_browser_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.selected_idx > 0 {
                app.selected_idx -= 1;
                update_browser_scroll(app);
            }
            HandlerResult::Continue
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.mode = AppMode::CreateRemoteDir;
            app.input_new_dir.clear();
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            let selected = &app.dir_entries[app.selected_idx];
            let new_path = if selected == ".." {
                let p = std::path::Path::new(&app.remote_current_path);
                match p.parent() {
                    Some(parent) => {
                        let s = parent.to_string_lossy().to_string();
                        if s.is_empty() {
                            "/".to_string()
                        } else {
                            s
                        }
                    }
                    None => app.remote_current_path.clone(),
                }
            } else {
                if app.remote_current_path == "/" {
                    format!("/{}", selected)
                } else {
                    format!("{}/{}", app.remote_current_path, selected)
                }
            };
            
            app.remote_current_path = new_path.replace("//", "/");
            
            match send_req(ClientRequest::ListRemoteDirs(
                app.pending_remote_host.clone(),
                app.pending_remote_port,
                app.remote_current_path.clone(),
                app.pending_password.clone()
            )) {
                ServerResponse::DirList(d) => {
                    app.dir_entries = d;
                    app.dir_entries.insert(0, "..".to_string());
                    app.selected_idx = 0;
                    update_browser_scroll(app);
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error navigating remote: {}", e);
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        KeyCode::Char(' ') => {
            let selected_item = &app.dir_entries[app.selected_idx];
            let final_path = if selected_item == ".." {
                app.remote_current_path.clone()
            } else {
                if app.remote_current_path == "/" {
                    format!("/{}", selected_item)
                } else {
                    format!("{}/{}", app.remote_current_path, selected_item)
                }
            };
            let final_path = final_path.replace("//", "/");
            
            match app.pending_sync_direction {
                SyncDirection::Push => {
                    // Push mode: create task (local source already selected, remote destination just selected)
                    let task_id = format!("task_{}", app.tasks.len() + 1);
                    
                    let new_task = SyncTask {
                        id: task_id,
                        source: app.pending_source.clone(),
                        remote_host: app.pending_remote_host.clone(),
                        remote_port: app.pending_remote_port,
                        remote_path: final_path,
                        status: "STARTING".to_string(),
                        last_log: "Created".to_string(),
                        poll_interval: 5,
                        sync_mode: app.pending_sync_mode.clone(),
                        compress: app.pending_compress,
                        sync_direction: app.pending_sync_direction.clone(),
                    };
                    
                    match send_req(ClientRequest::StartTask(new_task, app.pending_password.clone())) {
                        ServerResponse::Ack => {
                            app.mode = AppMode::Dashboard;
                            app.pending_source.clear();
                            app.remote_current_path.clear();
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Error starting task: {}", e);
                        }
                        _ => {}
                    }
                }
                SyncDirection::Pull => {
                    // Pull mode: store remote path (will be remote_path in task), then go to local browser
                    // Store the selected remote path in remote_current_path (we'll use it when creating task)
                    // remote_current_path will hold the selected remote source path
                    // pending_source will be overwritten with local destination path later
                    let selected_remote_path = final_path.clone();
                    app.remote_current_path = selected_remote_path; // Store selected remote path
                    // Now go to local browser to select local destination
                    app.mode = AppMode::LocalBrowser;
                    match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                        ServerResponse::DirList(d) => {
                            app.dir_entries = d;
                            app.dir_entries.insert(0, "..".to_string());
                            app.selected_idx = 0;
                            update_browser_scroll(app);
                        }
                        ServerResponse::Error(e) => {
                            eprintln!("Error listing dirs: {}", e);
                            app.mode = AppMode::Dashboard;
                        }
                        _ => {}
                    }
                }
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_dry_run_view_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Dashboard;
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.dry_run_scroll < app.dry_run_results.len().saturating_sub(1) {
                app.dry_run_scroll += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.dry_run_scroll > 0 {
                app.dry_run_scroll -= 1;
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_log_view_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.mode = AppMode::Dashboard;
            HandlerResult::Continue
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.view_log_scroll = app.view_log_scroll.saturating_add(1);
            HandlerResult::Continue
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.view_log_scroll = app.view_log_scroll.saturating_sub(1);
            HandlerResult::Continue
        }
        KeyCode::PageDown => {
            app.view_log_scroll = app.view_log_scroll.saturating_add(10);
            HandlerResult::Continue
        }
        KeyCode::PageUp => {
            app.view_log_scroll = app.view_log_scroll.saturating_sub(10);
            HandlerResult::Continue
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            // Manual refresh
            let tid = app.view_log_task_id.clone();
            match send_req(ClientRequest::GetTaskLog(tid.clone())) {
                ServerResponse::TaskLog(_, content) => {
                    app.view_task_log = content;
                    app.view_log_last_fetch = std::time::Instant::now();
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_local_mkdir_input_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::LocalBrowser; // Cancel
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            if !app.input_new_dir.trim().is_empty() {
                // Construct path: current_path/new_dir_name
                let new_path = if app.current_path == "/" {
                    format!("/{}", app.input_new_dir.trim())
                } else {
                    format!("{}/{}", app.current_path, app.input_new_dir.trim())
                };
                match send_req(ClientRequest::CreateLocalDir(new_path)) {
                    ServerResponse::Ack => {
                        app.mode = AppMode::LocalBrowser;
                        match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                            ServerResponse::DirList(d) => {
                                app.dir_entries = d;
                                app.dir_entries.insert(0, "..".to_string());
                                app.selected_idx = 0;
                                update_browser_scroll(app);
                            }
                            _ => {}
                        }
                    }
                    ServerResponse::Error(e) => {
                        app.input_new_dir = format!("Error: {}", e);
                    }
                    _ => { app.mode = AppMode::LocalBrowser; }
                } 
            } else {
                    app.mode = AppMode::LocalBrowser;
                }
                HandlerResult::Continue
        }
        KeyCode::Backspace => {
            app.input_new_dir.pop();
            HandlerResult::Continue
        }
        KeyCode::Char(c) => {
            app.input_new_dir.push(c);
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn handle_remote_mkdir_input_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::RemoteBrowser; // Cancel
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            if !app.input_new_dir.trim().is_empty() {
                // Construct path: current_path/new_dir_name
                let new_path = if app.remote_current_path == "/" {
                    format!("/{}", app.input_new_dir.trim())
                } else {
                    format!("{}/{}", app.remote_current_path, app.input_new_dir.trim())
                };

                // Send Create Request
                match send_req(ClientRequest::CreateRemoteDir(
                    app.pending_remote_host.clone(),
                    app.pending_remote_port,
                    new_path,
                    app.pending_password.clone()
                )) {
                    ServerResponse::Ack => {
                        // Success: Go back to browser
                        app.mode = AppMode::RemoteBrowser;
                        
                        // REFRESH the list immediately so we see the new folder
                        match send_req(ClientRequest::ListRemoteDirs(
                            app.pending_remote_host.clone(),
                            app.pending_remote_port,
                            app.remote_current_path.clone(),
                            app.pending_password.clone()
                        )) {
                            ServerResponse::DirList(d) => {
                                app.dir_entries = d;
                                app.dir_entries.insert(0, "..".to_string());
                                app.selected_idx = 0;
                                update_browser_scroll(app);
                                // Optional: Move selection to the new folder? 
                                // For now, just reset or keep 0
                            }
                            _ => {}
                        }
                    }
                    ServerResponse::Error(e) => {
                        // INSTEAD OF: eprintln!("Failed to create directory: {}", e);
                        // DO THIS: Show the error in the input field so the user sees it
                        app.input_new_dir = format!("Error: {}", e);
                        // Optional: Force a redraw or keep mode to let user read it
                        // app.mode stays AppMode::CreateRemoteDir
                    }
                    _ => { app.mode = AppMode::RemoteBrowser; }
                }
            } else {
                app.mode = AppMode::RemoteBrowser; // Empty name = cancel
            }
            HandlerResult::Continue
        }
        KeyCode::Backspace => {
            app.input_new_dir.pop();
            HandlerResult::Continue
        }
        KeyCode::Char(c) => {
            app.input_new_dir.push(c);
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
    }
}

fn update_dashboard_scroll(app: &mut App) {
    app.dashboard_list_state.select(Some(app.dashboard_selected_idx));
}

fn update_browser_scroll(app: &mut App) {
    app.browser_list_state.select(Some(app.selected_idx));
}