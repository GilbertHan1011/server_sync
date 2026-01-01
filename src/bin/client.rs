use std::{io, time::Duration};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use server_sync::protocol::ServerState;
use std::os::unix::net::UnixStream;
use std::io::{Read, Write};

fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

fn fetch_state() -> ServerState {
    let socket_path = get_socket_path();
    
    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
        let mut buffer = vec![0; 65535];
        if let Ok(n) = stream.read(&mut buffer) {
            if n > 0 {
                // Parse JSON from the stream (server sends newline-terminated JSON)
                if let Ok(json_str) = String::from_utf8(buffer[..n].to_vec()) {
                    // Find the first complete JSON object (up to newline)
                    if let Some(line_end) = json_str.find('\n') {
                        let json_line = &json_str[..line_end];
                        if let Ok(state) = serde_json::from_str::<ServerState>(json_line) {
                            return state;
                        }
                    } else if let Ok(state) = serde_json::from_str::<ServerState>(&json_str) {
                        return state;
                    }
                }
            }
        }
    }
    
    ServerState::default()
}

fn send_command(cmd: &str) {
    let socket_path = get_socket_path();
    if let Ok(mut stream) = UnixStream::connect(&socket_path) {
        let _ = stream.write_all(cmd.as_bytes());
    }
}

fn main() -> anyhow::Result<()> {
    // Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        // 1. Fetch Data from Daemon
        let state = fetch_state();

        // 2. Draw UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(f.area());
            
            let status_color = if state.status == "ERROR" {
                Color::Red
            } else if state.status.contains("DISCONNECTED") {
                Color::DarkGray
            } else if state.status == "SYNCING..." {
                Color::Yellow
            } else {
                Color::Green
            };

            let status_block = Paragraph::new(format!(
                "Status: {}\nFiles Synced: {}\n\n[Q] Quit\n[F] Force Sync", 
                state.status, state.sync_count))
                .block(Block::default()
                    .title("Sync Commander")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(status_color)));
            f.render_widget(status_block, chunks[0]);

            let logs: Vec<ListItem> = state.logs.iter().rev().map(|s| ListItem::new(s.as_str())).collect();
            let log_list = List::new(logs)
                .block(Block::default().title("Logs").borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(log_list, chunks[1]);
        })?;

        // 3. Input Handling
        if crossterm::event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => break,
                    KeyCode::Char('f') | KeyCode::Char('F') => {
                        send_command("F");
                    }
                    _ => {}
                }
            }
        }

        // Small delay to avoid excessive CPU usage
        std::thread::sleep(Duration::from_millis(200));
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}

