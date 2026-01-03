use std::{io::{Read, Write}, os::unix::net::UnixStream, path::Path};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Terminal,
};
use server_sync::protocol::{SyncTask, ClientRequest, ServerResponse};

// --- UI STATE ---
enum AppMode {
    Dashboard,
    LocalBrowser,     // Browse local directories (source selection)
    RemoteHostInput,  // Edit remote host before browsing remote
    RemoteBrowser,    // Browse remote directories (destination selection)
}

struct App {
    mode: AppMode,
    tasks: Vec<SyncTask>,
    // Browser State
    current_path: String,
    dir_entries: Vec<String>,
    selected_idx: usize,
    // Task Creation State
    pending_source: String,        // Selected local path before remote browsing
    remote_current_path: String,   // Current path in remote browser
    pending_remote_host: String,   // Remote host (e.g., "user@host")
    // Remote Host Input State
    input_remote_host: String,     // User's edited remote host
    input_cursor_pos: usize,       // Cursor position in input field
}

fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

fn send_req(req: ClientRequest) -> ServerResponse {
    let socket_path = get_socket_path();
    
    match UnixStream::connect(&socket_path) {
        Ok(mut stream) => {
            let json = match serde_json::to_string(&req) {
                Ok(j) => j,
                Err(e) => {
                    return ServerResponse::Error(format!("Serialization error: {}", e));
                }
            };
            
            if stream.write_all(json.as_bytes()).is_err() {
                return ServerResponse::Error("Failed to send request".to_string());
            }
            
            let mut buf = vec![0; 65535];
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => {
                    match serde_json::from_slice(&buf[..n]) {
                        Ok(resp) => resp,
                        Err(e) => ServerResponse::Error(format!("Parse error: {}", e)),
                    }
                }
                Ok(_) => ServerResponse::Error("Empty response".to_string()),
                Err(e) => ServerResponse::Error(format!("Read error: {}", e)),
            }
        }
        Err(_) => ServerResponse::Error("Daemon not running!".to_string()),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn main() -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App {
        mode: AppMode::Dashboard,
        tasks: vec![],
        current_path: "/storage/zhangkaiLab/hanlitian".to_string(), // Start here
        dir_entries: vec![],
        selected_idx: 0,
        pending_source: String::new(),
        remote_current_path: String::new(),
        pending_remote_host: String::new(), // Will be fetched from server
        input_remote_host: String::new(),
        input_cursor_pos: 0,
    };

    // Fetch remote host from server on startup
    match send_req(ClientRequest::GetRemoteHost) {
        ServerResponse::RemoteHost(host) => {
            app.pending_remote_host = host;
        }
        ServerResponse::Error(e) => {
            eprintln!("Warning: Could not fetch remote host from server: {}", e);
            app.pending_remote_host = "user@remote".to_string(); // Fallback
        }
        _ => {
            eprintln!("Warning: Unexpected response when fetching remote host");
            app.pending_remote_host = "user@remote".to_string(); // Fallback
        }
    }

    loop {
        // 1. DATA FETCH - Only update task state when viewing Dashboard
        if matches!(app.mode, AppMode::Dashboard) {
            match send_req(ClientRequest::GetState) {
                ServerResponse::State(t) => {
                    app.tasks = t;
                }
                ServerResponse::Error(e) => {
                    // If daemon not running, show error but don't crash
                    if e.contains("not running") {
                        app.tasks = vec![];
                    }
                }
                _ => {}
            }
        }

        // 2. RENDER
        terminal.draw(|f| {
            let size = f.area();

            // --- DASHBOARD ---
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
                .split(size);

                let items: Vec<ListItem> = app.tasks.iter().map(|t| {
                let color = match t.status.as_str() {
                    "IDLE" => Color::Green,
                    "ERROR" => Color::Red,
                    "SYNCING..." => Color::Yellow,
                    "PENDING..." => Color::Cyan,
                    _ => Color::Blue,
                };

                let remote_display = format!("{}:{}", t.remote_host, t.remote_path);
                ListItem::new(format!(
                    "ID: {} | {} -> {}\n   [{}] {}",
                    t.id, t.source, remote_display, t.status, t.last_log
                ))
                .style(Style::default().fg(color))
            })
            .collect();

            let list = List::new(items)
                .block(Block::default().title("Active Sync Tasks").borders(Borders::ALL));
            f.render_widget(list, chunks[0]);

            let help = Paragraph::new(
                "Controls:\n[A] Add New Task (3-step wizard)\n[D] Delete Task (First ID)\n[Q] Quit"
            )
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(help, chunks[1]);

            // --- BROWSER POPUP ---
            if let AppMode::LocalBrowser | AppMode::RemoteBrowser = app.mode {
                let area = centered_rect(60, 60, size);
                f.render_widget(Clear, area); // Clear background

                let browser_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(3), Constraint::Length(3)])
                    .split(area);

                // Dir List
                let dirs: Vec<ListItem> = app
                    .dir_entries
                    .iter()
                    .enumerate()
                    .map(|(i, d)| {
                        let style = if i == app.selected_idx {
                            Style::default().bg(Color::Blue)
                        } else {
                            Style::default()
                        };
                        ListItem::new(d.clone()).style(style)
                    })
                    .collect();

                // Different titles and instructions based on mode
                let (title, instructions_text) = match app.mode {
                    AppMode::LocalBrowser => (
                        format!("Select Source Folder: {}", app.current_path),
                        "[Enter] Enter Dir  [Space] Select as Source  [Esc] Cancel"
                    ),
                    AppMode::RemoteBrowser => (
                        format!("Select Remote Destination: {}", 
                            if app.remote_current_path.is_empty() { 
                                format!("{}:/", app.pending_remote_host) 
                            } else { 
                                format!("{}:{}", app.pending_remote_host, app.remote_current_path) 
                            }),
                        "[Enter] Enter Dir  [Space] Select as Destination  [Esc] Cancel"
                    ),
                    _ => unreachable!(),
                };

                let b_block = Block::default()
                    .title(title)
                    .borders(Borders::ALL);
                f.render_widget(List::new(dirs).block(b_block), browser_chunks[0]);

                let instructions = Paragraph::new(instructions_text)
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(instructions, browser_chunks[1]);
            }

            // --- REMOTE HOST INPUT POPUP ---
            if let AppMode::RemoteHostInput = app.mode {
                let area = centered_rect(70, 25, size);
                f.render_widget(Clear, area);
                
                let input_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(5), Constraint::Length(3)])
                    .split(area);
                
                // Show the input field with cursor
                let display_text = if app.input_cursor_pos <= app.input_remote_host.len() {
                    if app.input_cursor_pos < app.input_remote_host.len() {
                        format!("{}|{}", 
                            &app.input_remote_host[..app.input_cursor_pos],
                            &app.input_remote_host[app.input_cursor_pos..])
                    } else {
                        format!("{}|", app.input_remote_host)
                    }
                } else {
                    format!("{}|", app.input_remote_host)
                };
                
                let input_block = Paragraph::new(format!(
                    "Remote Host (user@hostname):\n\n{}",
                    display_text
                ))
                    .block(Block::default()
                        .title("Step 2: Confirm/Edit Remote Host")
                        .borders(Borders::ALL));
                f.render_widget(input_block, input_chunks[0]);
                
                let instructions = Paragraph::new("[Enter] Continue  [Esc] Back  [←→] Move  [Home/End] Jump")
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(instructions, input_chunks[1]);
            }
        })?;

        // 3. INPUT - Reduced timeout for responsive typing
        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match app.mode {
                    AppMode::Dashboard => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => break,
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            // Enter LocalBrowser Mode
                            app.mode = AppMode::LocalBrowser;
                            match send_req(ClientRequest::ListLocalDirs(app.current_path.clone())) {
                                ServerResponse::DirList(d) => {
                                    app.dir_entries = d;
                                    app.dir_entries.insert(0, "..".to_string()); // Add parent navigation
                                    app.selected_idx = 0;
                                }
                                ServerResponse::Error(e) => {
                                    eprintln!("Error listing dirs: {}", e);
                                    app.mode = AppMode::Dashboard;
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Char('D') => {
                            // Quick hack: delete first task
                            if let Some(t) = app.tasks.first() {
                                send_req(ClientRequest::StopTask(t.id.clone()));
                            }
                        }
                        _ => {}
                    },
                    AppMode::LocalBrowser => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Dashboard,
                        KeyCode::Down => {
                            if app.selected_idx < app.dir_entries.len().saturating_sub(1) {
                                app.selected_idx += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.selected_idx > 0 {
                                app.selected_idx -= 1;
                            }
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

                            app.current_path = new_path.replace("//", "/"); // Clean path
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
                        }
                        KeyCode::Char(' ') => {
                            // STEP 1 COMPLETE: Source Selected
                            app.pending_source = app.current_path.clone();
                            
                            // Switch to Remote Host Input
                            app.mode = AppMode::RemoteHostInput;
                            app.input_remote_host = app.pending_remote_host.clone(); // Pre-fill with default
                            app.input_cursor_pos = app.input_remote_host.len();
                        }
                        _ => {}
                    },
                    AppMode::RemoteHostInput => match key.code {
                        KeyCode::Esc => {
                            app.mode = AppMode::LocalBrowser;  // Go back to local browser
                        }
                        KeyCode::Enter => {
                            // STEP 2 COMPLETE: Remote Host Confirmed
                            // Update pending_remote_host with the edited value
                            app.pending_remote_host = app.input_remote_host.trim().to_string();
                            
                            // Switch to Remote Browser
                            app.mode = AppMode::RemoteBrowser;
                            app.remote_current_path = String::new(); // Start at remote HOME
                            
                            // Fetch Remote Dirs using the host from user input
                            match send_req(ClientRequest::ListRemoteDirs(
                                app.pending_remote_host.clone(),
                                app.remote_current_path.clone()
                            )) {
                                ServerResponse::DirList(d) => {
                                    app.dir_entries = d;
                                    app.dir_entries.insert(0, "..".to_string());
                                    app.selected_idx = 0;
                                }
                                ServerResponse::Error(e) => {
                                    eprintln!("Error listing remote dirs: {}", e);
                                    app.mode = AppMode::RemoteHostInput;
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Left => {
                            if app.input_cursor_pos > 0 {
                                app.input_cursor_pos -= 1;
                            }
                        }
                        KeyCode::Right => {
                            if app.input_cursor_pos < app.input_remote_host.len() {
                                app.input_cursor_pos += 1;
                            }
                        }
                        KeyCode::Backspace => {
                            if app.input_cursor_pos > 0 {
                                app.input_remote_host.remove(app.input_cursor_pos - 1);
                                app.input_cursor_pos -= 1;
                            }
                        }
                        KeyCode::Delete => {
                            if app.input_cursor_pos < app.input_remote_host.len() {
                                app.input_remote_host.remove(app.input_cursor_pos);
                            }
                        }
                        KeyCode::Home => {
                            app.input_cursor_pos = 0;
                        }
                        KeyCode::End => {
                            app.input_cursor_pos = app.input_remote_host.len();
                        }
                        KeyCode::Char(c) => {
                            // Insert character at cursor
                            app.input_remote_host.insert(app.input_cursor_pos, c);
                            app.input_cursor_pos += 1;
                        }
                        _ => {}
                    },
                    AppMode::RemoteBrowser => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Dashboard, // Cancel
                        KeyCode::Down => {
                            if app.selected_idx < app.dir_entries.len().saturating_sub(1) {
                                app.selected_idx += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.selected_idx > 0 {
                                app.selected_idx -= 1;
                            }
                        }
                        KeyCode::Enter => {
                            // Navigate Remote Dir
                            let selected = &app.dir_entries[app.selected_idx];
                            let new_path = if selected == ".." {
                                // Simple parent logic (string manipulation)
                                let p = std::path::Path::new(&app.remote_current_path);
                                p.parent().unwrap_or(std::path::Path::new("")).to_str().unwrap().to_string()
                            } else {
                                if app.remote_current_path.is_empty() {
                                    selected.clone()
                                } else {
                                    format!("{}/{}", app.remote_current_path, selected)
                                }
                            };
                            
                            app.remote_current_path = new_path;
                            
                            // Fetch New Remote List using the host from user input
                            match send_req(ClientRequest::ListRemoteDirs(
                                app.pending_remote_host.clone(),
                                app.remote_current_path.clone()
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
                        }
                        KeyCode::Char(' ') => {
                            // STEP 2 COMPLETE: Remote Dest Selected
                            let task_id = format!("task_{}", app.tasks.len() + 1);
                            
                            let new_task = SyncTask {
                                id: task_id,
                                source: app.pending_source.clone(),
                                remote_host: app.pending_remote_host.clone(),
                                remote_path: app.remote_current_path.clone(),
                                status: "STARTING".to_string(),
                                last_log: "Created".to_string(),
                                poll_interval: 5,
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
                        }
                        _ => {}
                    }
                }
            }
        }

        // No extra sleep needed - poll() provides timing control
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}
