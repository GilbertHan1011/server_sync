use std::{io::{Read, Write}, os::unix::net::UnixStream, path::Path, panic, fs::File, sync::atomic::{AtomicBool, Ordering}, time::Duration};
use simplelog::*;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
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
use server_sync::protocol::{SyncTask, ClientRequest, ServerResponse, SyncMode};

// Global flag for Ctrl+C
static CTRL_C_PRESSED: AtomicBool = AtomicBool::new(false);

// --- UI STATE ---
enum AppMode {
    Dashboard,
    LocalBrowser,     // Browse local directories (source selection)
    RemoteHostInput,  // Edit remote host before browsing remote
    PasswordInput,    // Enter SSH password (optional)
    SyncModeSelect,   // Select sync mode (Mirror, AddOnly, SafeSync, Update)
    RemoteBrowser,    // Browse remote directories (destination selection)
    DryRunView,       // Display dry run results
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
    // Password Input State
    pending_password: Option<String>, // Stores the final confirmed password
    input_password: String,           // Buffer for typing password
    show_password: bool,              // Toggle to show/hide characters
    // Sync Mode Selection State
    pending_sync_mode: SyncMode,   // Selected sync mode
    pending_compress: bool,        // Compression enabled
    sync_mode_selected_idx: usize, // 0-3 for the 4 modes
    // Dry Run State
    dry_run_results: Vec<String>,  // Results from dry run
    dry_run_task_id: String,       // Which task was dry-run
    dry_run_scroll: usize,         // Scroll position in dry run view
}

fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

fn send_req(req: ClientRequest) -> ServerResponse {
    let socket_path = get_socket_path();
    log::info!("Sending request: {:?}", req); // Log before connecting

    match UnixStream::connect(&socket_path) {
        Ok(mut stream) => {
            // 1. Set Timeout (Fixes the hang)
            if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(3))) {
                log::error!("Failed to set timeout: {}", e);
            }
            if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(3))) {
                log::error!("Failed to set timeout: {}", e);
            }

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
                    log::info!("Received response ({} bytes)", n); // Log success
                    match serde_json::from_slice(&buf[..n]) {
                        Ok(resp) => resp,
                        Err(e) => {
                            ServerResponse::Error(format!("Parse error: {}", e))
                        }
                    }
                }
                Ok(_) => ServerResponse::Error("Empty response".to_string()),
                Err(e) => {
                    log::error!("Read error (Server timed out?): {}", e);
                    ServerResponse::Error(format!("Read error: {}", e))
                }
            }
        }
        Err(e) => {
            log::warn!("Could not connect to daemon: {}", e);
            ServerResponse::Error("Daemon not running!".to_string())
        }
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
    // 0. Initialize Logging
    WriteLogger::init(
        LevelFilter::Info,
        Config::default(),
        File::create("client_debug.log")?,
    )?;
    
    log::info!("Client starting...");

    // 0.5. Setup Ctrl+C handler (BEFORE terminal setup)
    ctrlc::set_handler(|| {
        CTRL_C_PRESSED.store(true, Ordering::Relaxed);
        // Try to restore terminal immediately
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen, crossterm::event::DisableMouseCapture);
    })?;

    // 1. Install Panic Hook (The Safety Net)
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        // Force-restore terminal before printing the error
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen, crossterm::event::DisableMouseCapture);
        original_hook(panic_info);
    }));

    // 2. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 3. Run App (Wrapped to catch errors)
    let res = run_app(&mut terminal);

    // 4. Cleanup (Always runs, even if error occurs)
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    // 5. Print Error if any
    if let Err(err) = res {
        eprintln!("❌ Application Error: {:?}", err);
    }

    Ok(())
}

// Extract main loop into separate function
fn run_app<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>) -> anyhow::Result<()> {
    let start_path = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| std::env::var("HOME").unwrap_or_else(|_| "/".to_string()));
    let mut app = App {
        mode: AppMode::Dashboard,
        tasks: vec![],
        current_path: start_path, // Start here
        dir_entries: vec![],
        selected_idx: 0,
        pending_source: String::new(),
        remote_current_path: String::new(),
        pending_remote_host: String::new(), // Will be fetched from server
        input_remote_host: String::new(),
        input_cursor_pos: 0,
        pending_password: None,
        input_password: String::new(),
        show_password: false,
        pending_sync_mode: SyncMode::Mirror, // Default: Mirror mode
        pending_compress: true,               // Default: Enable compression
        sync_mode_selected_idx: 0,           // Start at first option
        dry_run_results: vec![],
        dry_run_task_id: String::new(),
        dry_run_scroll: 0,
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
        // Check for Ctrl+C signal
        if CTRL_C_PRESSED.load(Ordering::Relaxed) {
            log::info!("Ctrl+C received, exiting...");
            break;
        }

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
                let mode_name = match t.sync_mode {
                    SyncMode::Mirror => "Mirror",
                    SyncMode::AddOnly => "AddOnly",
                    SyncMode::SafeSync => "SafeSync",
                    SyncMode::Update => "Update",
                };
                let compress_flag = if t.compress { "+Z" } else { "" };
                
                ListItem::new(format!(
                    "ID: {} | {} -> {}\n   [{}] Mode: {}{} | {}",
                    t.id, t.source, remote_display, t.status, mode_name, compress_flag, t.last_log
                ))
                .style(Style::default().fg(color))
            })
            .collect();

            let list = List::new(items)
                .block(Block::default().title("Active Sync Tasks").borders(Borders::ALL));
            f.render_widget(list, chunks[0]);

            let help = Paragraph::new(
                "Controls:\n[A] Add New Task (4-step wizard)\n[D] Delete Task (First ID)\n[R] Dry Run (First Task)\n[Q] Quit"
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
                        format!("Select Remote Destination: {} (Interface may freeze during SSH)", 
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

            // --- PASSWORD INPUT POPUP ---
            if let AppMode::PasswordInput = app.mode {
                let area = centered_rect(60, 25, size);
                f.render_widget(Clear, area);
                
                let pass_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(5), Constraint::Length(3)])
                    .split(area);

                let display_text = if app.show_password {
                    format!("{}|", app.input_password)
                } else {
                    format!("{}|", "*".repeat(app.input_password.len()))
                };

                let input = Paragraph::new(format!(
                    "SSH Password (leave empty for SSH keys):\n\n{}",
                    display_text
                ))
                    .block(Block::default()
                        .title("Step 3: Enter SSH Password")
                        .borders(Borders::ALL));
                f.render_widget(input, pass_chunks[0]);

                let help = Paragraph::new("[Enter] Confirm  [Tab] Show/Hide  [Esc] Back")
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(help, pass_chunks[1]);
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

            // --- SYNC MODE SELECT POPUP ---
            if let AppMode::SyncModeSelect = app.mode {
                let area = centered_rect(70, 50, size);
                f.render_widget(Clear, area);
                
                let sync_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(10), Constraint::Length(3)])
                    .split(area);
                
                // Mode descriptions
                let modes = vec![
                    ("Mirror", "Exact copy. Deletes files on remote if missing on local", Color::Yellow),
                    ("Add Only", "Uploads new/changed files. Never deletes on remote", Color::Green),
                    ("Safe Sync", "Mirrors but moves deleted files to .rsync-backup folder", Color::Cyan),
                    ("Update", "Only overwrites if local file is newer", Color::Blue),
                ];
                
                let mut mode_items: Vec<ListItem> = vec![];
                for (idx, (name, desc, color)) in modes.iter().enumerate() {
                    let prefix = if idx == app.sync_mode_selected_idx { "> " } else { "  " };
                    let text = format!("{}{}", prefix, name);
                    let item = if idx == app.sync_mode_selected_idx {
                        ListItem::new(vec![
                            ratatui::text::Line::from(text).style(Style::default().fg(*color).bg(Color::DarkGray)),
                            ratatui::text::Line::from(format!("  {}", desc)).style(Style::default().fg(Color::Gray)),
                        ])
                    } else {
                        ListItem::new(vec![
                            ratatui::text::Line::from(text).style(Style::default().fg(*color)),
                            ratatui::text::Line::from(format!("  {}", desc)).style(Style::default().fg(Color::DarkGray)),
                        ])
                    };
                    mode_items.push(item);
                }
                
                // Add compression toggle
                let compress_text = if app.pending_compress {
                    "[X] Enable Compression (-z)"
                } else {
                    "[ ] Enable Compression (-z)"
                };
                mode_items.push(ListItem::new(""));
                mode_items.push(ListItem::new(compress_text).style(Style::default().fg(Color::White)));
                
                let mode_list = List::new(mode_items)
                    .block(Block::default()
                        .title("Step 4: Select Sync Mode")
                        .borders(Borders::ALL));
                f.render_widget(mode_list, sync_chunks[0]);
                
                let instructions = Paragraph::new("[Enter] Continue  [↑↓] Navigate  [Space] Toggle Compress  [Esc] Back")
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(instructions, sync_chunks[1]);
            }

            // --- DRY RUN VIEW POPUP ---
            if let AppMode::DryRunView = app.mode {
                let area = centered_rect(80, 60, size);
                f.render_widget(Clear, area);
                
                let dry_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(10), Constraint::Length(3)])
                    .split(area);
                
                // Create list items from dry run results
                let items: Vec<ListItem> = app.dry_run_results
                    .iter()
                    .skip(app.dry_run_scroll)
                    .take(area.height as usize - 5) // Leave space for title and help
                    .map(|s| ListItem::new(s.as_str()))
                    .collect();
                
                let list = List::new(items)
                    .block(Block::default()
                        .title(format!("Dry Run Results: {}", app.dry_run_task_id))
                        .borders(Borders::ALL));
                f.render_widget(list, dry_chunks[0]);
                
                let help = Paragraph::new("[Esc] Close  [↑↓] Scroll")
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(help, dry_chunks[1]);
            }
        }).map_err(|_e| anyhow::anyhow!("Terminal draw error"))?;

        // 3. INPUT - Reduced timeout for responsive typing
        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Handle Ctrl+C explicitly (backup to signal handler)
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    log::info!("Ctrl+C pressed, exiting...");
                    break;
                }
                
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
                            
                            // Switch to Password Input
                            app.mode = AppMode::PasswordInput;
                            app.input_password.clear();
                            app.show_password = false;
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
                    AppMode::PasswordInput => match key.code {
                        KeyCode::Esc => {
                            app.mode = AppMode::RemoteHostInput;  // Go back
                        }
                        KeyCode::Enter => {
                            // STEP 3 COMPLETE: Password Confirmed
                            if app.input_password.trim().is_empty() {
                                app.pending_password = None; // Empty means "Use SSH Keys"
                            } else {
                                app.pending_password = Some(app.input_password.clone());
                            }
                            // Proceed to Sync Mode Select
                            app.mode = AppMode::SyncModeSelect;
                            app.sync_mode_selected_idx = 0; // Reset to first mode
                        }
                        KeyCode::Tab => {
                            app.show_password = !app.show_password;
                        }
                        KeyCode::Backspace => {
                            app.input_password.pop();
                        }
                        KeyCode::Char(c) => {
                            app.input_password.push(c);
                        }
                        _ => {}
                    },
                    AppMode::SyncModeSelect => match key.code {
                        KeyCode::Esc => {
                            app.mode = AppMode::PasswordInput;  // Go back to password input
                        }
                        KeyCode::Down => {
                            if app.sync_mode_selected_idx < 3 {  // 0-3 for 4 modes
                                app.sync_mode_selected_idx += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.sync_mode_selected_idx > 0 {
                                app.sync_mode_selected_idx -= 1;
                            }
                        }
                        KeyCode::Char(' ') => {
                            // Toggle compression
                            app.pending_compress = !app.pending_compress;
                        }
                        KeyCode::Enter => {
                            // STEP 4 COMPLETE: Sync Mode Selected
                            // Save the selected mode
                            app.pending_sync_mode = match app.sync_mode_selected_idx {
                                0 => SyncMode::Mirror,
                                1 => SyncMode::AddOnly,
                                2 => SyncMode::SafeSync,
                                3 => SyncMode::Update,
                                _ => SyncMode::Mirror,
                            };
                            
                            // Switch to Remote Browser
                            app.mode = AppMode::RemoteBrowser;
                            match send_req(ClientRequest::GetRemoteHome(
                                app.pending_remote_host.clone(),
                                app.pending_password.clone()
                            )) {
                                ServerResponse::RemoteHome(path) => {
                                    app.remote_current_path = path;
                                }
                                _ => {
                                    app.remote_current_path = "/".to_string(); // Fallback
                                }
                            }
                            
                            // Fetch Remote Dirs using the host from user input
                            match send_req(ClientRequest::ListRemoteDirs(
                                app.pending_remote_host.clone(),
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
                                let p = std::path::Path::new(&app.remote_current_path);
                                match p.parent() {
                                    Some(parent) => {
                                        let s = parent.to_string_lossy().to_string();
                                        if s.is_empty() {
                                            "/".to_string()
                                        } else {
                                            s
                                        }
                                    },
                                    None => {
                                        app.remote_current_path.clone()
                                    }
                                }
                            } else {
                                if app.remote_current_path == "/" {
                                    format!("/{}", selected)
                                } else {
                                    format!("{}/{}", app.remote_current_path, selected)
                                }
                            };
                            
                            app.remote_current_path = new_path;
                            
                            // Fetch New Remote List using the host from user input
                            match send_req(ClientRequest::ListRemoteDirs(
                                app.pending_remote_host.clone(),
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
                        }
                        KeyCode::Char(' ') => {
                            // STEP 5 COMPLETE: Remote Dest Selected
                            let task_id = format!("task_{}", app.tasks.len() + 1);
                            
                            let new_task = SyncTask {
                                id: task_id,
                                source: app.pending_source.clone(),
                                remote_host: app.pending_remote_host.clone(),
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
                        }
                        _ => {}
                    },
                    AppMode::DryRunView => match key.code {
                        KeyCode::Esc => app.mode = AppMode::Dashboard,
                        KeyCode::Down => {
                            if app.dry_run_scroll < app.dry_run_results.len().saturating_sub(1) {
                                app.dry_run_scroll += 1;
                            }
                        }
                        KeyCode::Up => {
                            if app.dry_run_scroll > 0 {
                                app.dry_run_scroll -= 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // No extra sleep needed - poll() provides timing control
    }

    Ok(())
}
