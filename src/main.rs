use std::{io, time::Duration, sync::{Arc, Mutex}, process::Command, fs};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::mpsc;

// --- CONFIGURATION ---
#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    source_dir: String,
    remote_host: String,
    remote_dir: String,
    #[serde(default)]
    excludes: Vec<String>,
}

// --- STATE MANAGEMENT ---
struct AppState {
    logs: Vec<String>,
    status: String,
    sync_count: u32,
    config: AppConfig,
}

impl AppState {
    fn new(config: AppConfig) -> Self {
        Self {
            logs: vec![format!("Monitoring: {}", config.source_dir)],
            status: "IDLE".to_string(),
            sync_count: 0,
            config,
        }
    }

    fn add_log(&mut self, msg: String) {
        self.logs.push(format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), msg));
        if self.logs.len() > 20 { self.logs.remove(0); } // Keep log short
    }
}

// --- RSYNC LOGIC ---
async fn run_rsync(state: Arc<Mutex<AppState>>) {
    let config = {
        let mut s = state.lock().unwrap();
        s.status = "SYNCING...".to_string();
        s.config.clone()
    };

    // Construct command
    let mut cmd = Command::new("rsync");
    cmd.arg("-avz").arg("--delete");
    for exc in &config.excludes {
        cmd.arg(format!("--exclude={}", exc));
    }
    // Add trailing slash for rsync contents behavior
    cmd.arg(format!("{}/", config.source_dir)); 
    cmd.arg(format!("{}:{}", config.remote_host, config.remote_dir));

    // Run command (blocking in thread, but we are in tokio spawn)
    let output = cmd.output();

    let mut s = state.lock().unwrap();
    match output {
        Ok(out) if out.status.success() => {
            s.add_log("✅ Sync Successful".to_string());
            s.sync_count += 1;
            s.status = "IDLE".to_string();
        }
        Ok(out) => {
            let err_msg = String::from_utf8_lossy(&out.stderr);
            s.add_log(format!("❌ Sync Failed: {}", err_msg));
            s.status = "ERROR".to_string();
        }
        Err(e) => {
            s.add_log(format!("❌ Exec Error: {}", e));
            s.status = "ERROR".to_string();
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load Config
    let config_content = fs::read_to_string("config.yaml").expect("Failed to read config.yaml");
    let config: AppConfig = serde_yaml::from_str(&config_content).expect("Invalid YAML");

    // Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Shared State
    let app_state = Arc::new(Mutex::new(AppState::new(config.clone())));
    
    // Setup File Watcher Channel
    let (tx, mut rx) = mpsc::channel(100);
    
    // File Watcher Thread
    let path_to_watch = config.source_dir.clone();
    std::thread::spawn(move || {
        let (wt_tx, wt_rx) = std::sync::mpsc::channel();
        let mut watcher = RecommendedWatcher::new(wt_tx, Config::default()).unwrap();
        watcher.watch(std::path::Path::new(&path_to_watch), RecursiveMode::Recursive).unwrap();

        for res in wt_rx {
            match res {
                Ok(_) => { let _ = tx.blocking_send(()); }, // Signal change
                Err(e) => println!("watch error: {:?}", e),
            }
        }
    });

    // Main Loop
    let mut debounce_timer: Option<tokio::time::Instant> = None;
    
    loop {
        // 1. Draw UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(f.size());

            let state = app_state.lock().unwrap();

            // Left Panel (Status)
            let status_block = Paragraph::new(format!(
                "Status: {}\nFiles Synced: {}\n\n[Q] to Quit\n[F] Force Sync", 
                state.status, state.sync_count))
                .block(Block::default().title("Control").borders(Borders::ALL));
            f.render_widget(status_block, chunks[0]);

            // Right Panel (Logs)
            let logs: Vec<ListItem> = state.logs.iter().map(|s| ListItem::new(s.as_str())).collect();
            let log_list = List::new(logs)
                .block(Block::default().title("Logs").borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(log_list, chunks[1]);
        })?;

        // 2. Event Handling (Input & File Watch)
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('f') => {
                        let s = app_state.clone();
                        tokio::spawn(async move { run_rsync(s).await });
                    }
                    _ => {}
                }
            }
        }

        // 3. Check for File Changes (Debouncing)
        while let Ok(_) = rx.try_recv() {
            // Reset timer on every event
            debounce_timer = Some(tokio::time::Instant::now() + Duration::from_secs(2));
            let mut s = app_state.lock().unwrap();
            s.status = "PENDING...".to_string();
        }

        // 4. Trigger Sync if timer expired
        if let Some(time) = debounce_timer {
            if tokio::time::Instant::now() >= time {
                debounce_timer = None;
                let s = app_state.clone();
                tokio::spawn(async move { run_rsync(s).await });
            }
        }
    }

    // Restore Terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}