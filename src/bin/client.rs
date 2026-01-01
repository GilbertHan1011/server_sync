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
    Browser,
    InputRemote,  // New mode for inputting remote
}

struct App {
    mode: AppMode,
    tasks: Vec<SyncTask>,
    // Browser State
    current_path: String,
    dir_entries: Vec<String>,
    selected_idx: usize,
    // New Task Input
    input_remote: String, // We only browser local, remote is typed
    input_cursor_pos: usize,  // Cursor position for editing
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
        input_remote: String::new(),  // Start empty
        input_cursor_pos: 0,
    };

    loop {
        // 1. DATA FETCH
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

                ListItem::new(format!(
                    "ID: {} | {} -> {}\n   [{}] {}",
                    t.id, t.source, t.remote, t.status, t.last_log
                ))
                .style(Style::default().fg(color))
            })
            .collect();

            let list = List::new(items)
                .block(Block::default().title("Active Sync Tasks").borders(Borders::ALL));
            f.render_widget(list, chunks[0]);

            let help = Paragraph::new(
                "Controls:\n[A] Add New Task (Browse)\n[D] Delete Task (First ID)\n[Q] Quit"
            )
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(help, chunks[1]);

            // --- BROWSER POPUP ---
            if let AppMode::Browser = app.mode {
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

                let b_block = Block::default()
                    .title(format!("Browsing: {}", app.current_path))
                    .borders(Borders::ALL);
                f.render_widget(List::new(dirs).block(b_block), browser_chunks[0]);

                let instructions = Paragraph::new(
                    "[Enter] Enter Dir  [Space] Select as Source  [Esc] Cancel"
                )
                .block(Block::default().borders(Borders::ALL));
                f.render_widget(instructions, browser_chunks[1]);
            }

            // --- INPUT REMOTE POPUP ---
            if let AppMode::InputRemote = app.mode {
                let area = centered_rect(70, 20, size);
                f.render_widget(Clear, area);
                
                let input_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Length(3)])
                    .split(area);
                
                // Show the input field with cursor
                let display_text = if app.input_cursor_pos <= app.input_remote.len() {
                    if app.input_cursor_pos < app.input_remote.len() {
                        format!("{}_{}", 
                            &app.input_remote[..app.input_cursor_pos],
                            &app.input_remote[app.input_cursor_pos..])
                    } else {
                        format!("{}_", app.input_remote)
                    }
                } else {
                    format!("{}_", app.input_remote)
                };
                
                let input_block = Paragraph::new(display_text)
                    .block(Block::default()
                        .title("Enter Remote (user@host:/path)")
                        .borders(Borders::ALL));
                f.render_widget(input_block, input_chunks[0]);
                
                let instructions = Paragraph::new("[Enter] Confirm  [Esc] Cancel")
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(instructions, input_chunks[1]);
            }
        })?;

        // 3. INPUT
        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match app.mode {
                    AppMode::Dashboard => match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => break,
                        KeyCode::Char('a') | KeyCode::Char('A') => {
                            // Enter Browser Mode
                            app.mode = AppMode::Browser;
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
                    AppMode::Browser => match key.code {
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
                            // Switch to input mode to enter remote
                            app.mode = AppMode::InputRemote;
                            if app.input_remote.is_empty() {
                                app.input_remote = "user@host:/path/to/destination".to_string();
                                app.input_cursor_pos = app.input_remote.len();
                            } else {
                                app.input_cursor_pos = app.input_remote.len();
                            }
                        }
                        _ => {}
                    },
                    AppMode::InputRemote => match key.code {
                        KeyCode::Esc => {
                            app.mode = AppMode::Browser;  // Cancel, go back to browser
                        }
                        KeyCode::Enter => {
                            // Confirm and create task
                            if !app.input_remote.trim().is_empty() {
                                let task_id = format!("task_{}", app.tasks.len() + 1);
                                let new_task = SyncTask {
                                    id: task_id,
                                    source: app.current_path.clone(),
                                    remote: app.input_remote.trim().to_string(),
                                    status: "STARTING".to_string(),
                                    last_log: "Created".to_string(),
                                    poll_interval: 30,
                                };
                                
                                match send_req(ClientRequest::StartTask(new_task)) {
                                    ServerResponse::Ack => {
                                        app.mode = AppMode::Dashboard;
                                        app.input_remote.clear();
                                        app.input_cursor_pos = 0;
                                    }
                                    ServerResponse::Error(e) => {
                                        eprintln!("Error starting task: {}", e);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        KeyCode::Left => {
                            if app.input_cursor_pos > 0 {
                                app.input_cursor_pos -= 1;
                            }
                        }
                        KeyCode::Right => {
                            if app.input_cursor_pos < app.input_remote.len() {
                                app.input_cursor_pos += 1;
                            }
                        }
                        KeyCode::Backspace => {
                            if app.input_cursor_pos > 0 {
                                app.input_remote.remove(app.input_cursor_pos - 1);
                                app.input_cursor_pos -= 1;
                            }
                        }
                        KeyCode::Delete => {
                            if app.input_cursor_pos < app.input_remote.len() {
                                app.input_remote.remove(app.input_cursor_pos);
                            }
                        }
                        KeyCode::Home => {
                            app.input_cursor_pos = 0;
                        }
                        KeyCode::End => {
                            app.input_cursor_pos = app.input_remote.len();
                        }
                        KeyCode::Char(c) => {
                            // Insert character at cursor
                            app.input_remote.insert(app.input_cursor_pos, c);
                            app.input_cursor_pos += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        // Small delay to avoid excessive CPU usage
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}
