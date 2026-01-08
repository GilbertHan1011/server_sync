use std::path::Path;
use crossterm::event::{KeyCode, KeyEvent};
use crate::client::state::{App, AppMode};
use crate::client::network::send_req;
use crate::client::config::{load_hosts, save_hosts};
use crate::protocol::{ClientRequest, ServerResponse, SyncMode, SyncTask};

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
    }
}

fn handle_dashboard_keys(key: KeyEvent, app: &mut App) -> HandlerResult {
    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => HandlerResult::Quit,
        KeyCode::Char('a') | KeyCode::Char('A') => {
            // Enter LocalBrowser Mode
            app.mode = AppMode::LocalBrowser;
            match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                ServerResponse::DirList(d) => {
                    app.dir_entries = d;
                    app.dir_entries.insert(0, "..".to_string());
                    app.selected_idx = 0;
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error listing dirs: {}", e);
                    app.mode = AppMode::Dashboard;
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        KeyCode::Char('d') | KeyCode::Char('D') => {
            // Delete first task
            if let Some(t) = app.tasks.first() {
                send_req(ClientRequest::StopTask(t.id.clone()));
            }
            HandlerResult::Continue
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            // Dry run first task
            if let Some(first_task) = app.tasks.first() {
                match send_req(ClientRequest::DryRun(first_task.id.clone())) {
                    ServerResponse::DryRunResult(changes) => {
                        app.dry_run_results = changes;
                        app.dry_run_task_id = first_task.id.clone();
                        app.dry_run_scroll = 0;
                        app.mode = AppMode::DryRunView;
                    }
                    ServerResponse::Error(e) => {
                        eprintln!("Dry run error: {}", e);
                    }
                    _ => {}
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
            app.mode = AppMode::Dashboard;
            HandlerResult::Continue
        }
        KeyCode::Down => {
            if app.selected_idx < app.dir_entries.len().saturating_sub(1) {
                app.selected_idx += 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.selected_idx > 0 {
                app.selected_idx -= 1;
            }
            HandlerResult::Continue
        }
        KeyCode::Enter => {
            // Navigate into directory
            let selected = &app.dir_entries[app.selected_idx];
            let new_path = if selected == ".." {
                Path::new(&app.current_path)
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .display()
                    .to_string()
            } else {
                format!("{}/{}", app.current_path, selected)
            };

            app.current_path = new_path.replace("//", "/");
            match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                ServerResponse::DirList(d) => {
                    app.dir_entries = d;
                    app.dir_entries.insert(0, "..".to_string());
                    app.selected_idx = 0;
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error navigating: {}", e);
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        KeyCode::Char(' ') => {
            let selected_item = &app.dir_entries[app.selected_idx];
            let final_path = if selected_item == ".." {
                app.current_path.clone()
            } else {
                if app.current_path == "/" {
                    format!("/{}", selected_item)
                } else {
                    format!("{}/{}", app.current_path, selected_item)
                }
            };
            // STEP 1 COMPLETE: Source Selected
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
            HandlerResult::Continue
        }
        _ => HandlerResult::Continue,
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
                app.mode = AppMode::LocalBrowser;
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
            app.mode = AppMode::SyncModeSelect;
            app.sync_mode_selected_idx = 0;
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
            app.mode = AppMode::PasswordInput;
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
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error listing remote dirs: {}", e);
                    app.mode = AppMode::SyncModeSelect;
                }
                _ => {}
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
            }
            HandlerResult::Continue
        }
        KeyCode::Up => {
            if app.selected_idx > 0 {
                app.selected_idx -= 1;
            }
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
            
            app.remote_current_path = new_path;
            
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
                }
                ServerResponse::Error(e) => {
                    eprintln!("Error navigating remote: {}", e);
                }
                _ => {}
            }
            HandlerResult::Continue
        }
        KeyCode::Char(' ') => {
            let task_id = format!("task_{}", app.tasks.len() + 1);
            
            let new_task = SyncTask {
                id: task_id,
                source: app.pending_source.clone(),
                remote_host: app.pending_remote_host.clone(),
                remote_port: app.pending_remote_port,
                remote_path: app.remote_current_path.clone(),
                status: "STARTING".to_string(),
                last_log: "Created".to_string(),
                poll_interval: 5,
                sync_mode: app.pending_sync_mode.clone(),
                compress: app.pending_compress,
                password: app.pending_password.clone(),
            };
            
            match send_req(ClientRequest::StartTask(new_task)) {
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

