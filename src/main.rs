use std::{io, time::Duration, sync::{Arc, Mutex}, process::Command, fs};
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
use notify::{Config, PollWatcher, RecursiveMode, Watcher};
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
        if self.logs.len() > 50 { self.logs.remove(0); }
    }
}

// --- RSYNC LOGIC ---
async fn run_rsync(state: Arc<Mutex<AppState>>) {
    let config = {
        let mut s = state.lock().unwrap();
        s.status = "SYNCING...".to_string();
        s.config.clone()
    };

    let mut cmd = Command::new("rsync");
    cmd.arg("-avz").arg("--delete");
    for exc in &config.excludes {
        cmd.arg(format!("--exclude={}", exc));
    }
    cmd.arg(format!("{}/", config.source_dir)); 
    cmd.arg(format!("{}:{}", config.remote_host, config.remote_dir));

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
    // 1. Load Config
    let config_content = fs::read_to_string("config.yaml").expect("Failed to read config.yaml");
    let config: AppConfig = serde_yaml::from_str(&config_content).expect("Invalid YAML");

    // 2. Validate Path
    let path_to_watch = std::fs::canonicalize(&config.source_dir)
        .expect("❌ source_dir does not exist or is inaccessible!");

    // 3. Setup Terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // 4. Shared State
    let app_state = Arc::new(Mutex::new(AppState::new(config.clone())));
    
    // 5. Setup Channels
    let (tx, mut rx) = mpsc::channel(100);
    
    // --- STRATEGY 2: INITIAL SYNC ---
    {
        let state_clone = app_state.clone();
        tokio::spawn(async move {
            run_rsync(state_clone).await;
        });
    }

    // --- STRATEGY 2: FAST LOOP (Root Metadata) ---
    // Checks only the root folder every 1 second
    let tx_fast = tx.clone();
    let root_path = path_to_watch.clone();
    
    std::thread::spawn(move || {
        let mut last_modified = None;
        loop {
            // Check mtime of the root folder
            if let Ok(metadata) = std::fs::metadata(&root_path) {
                if let Ok(modified) = metadata.modified() {
                    // If mtime changed since last check, trigger sync
                    if let Some(last) = last_modified {
                        if modified != last {
                             let _ = tx_fast.blocking_send(());
                        }
                    }
                    last_modified = Some(modified);
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });

    // --- STRATEGY 2: SLOW LOOP (Deep Poll) ---
    // Scans EVERYTHING every 30 seconds
    let tx_slow = tx.clone();
    let deep_path = path_to_watch.clone();
    
    std::thread::spawn(move || {
        let (wt_tx, wt_rx) = std::sync::mpsc::channel();
        
        // 30 SECOND INTERVAL
        let config = Config::default()
            .with_poll_interval(Duration::from_secs(30)); 
            
        let mut watcher = match PollWatcher::new(wt_tx, config) {
            Ok(w) => w,
            Err(e) => { eprintln!("SlowWatcher Error: {:?}", e); return; }
        };
        if let Err(e) = watcher.watch(&deep_path, RecursiveMode::Recursive) {
             eprintln!("SlowWatcher Watch Error: {:?}", e);
             return;
        }
        for res in wt_rx {
            match res {
                Ok(_) => { let _ = tx_slow.blocking_send(()); },
                Err(_) => {}, // Ignore errors in slow loop to avoid log spam
            }
        }
    });

    // 6. Main TUI Loop
    let mut debounce_timer: Option<tokio::time::Instant> = None;
    
    loop {
        // Draw UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(f.area());
            let state = app_state.lock().unwrap();
            let status_block = Paragraph::new(format!(
                "Status: {}\nFiles Synced: {}\nStrategy: Hybrid (1s/30s)\n\n[Q] Quit  [F] Force", 
                state.status, state.sync_count))
                .block(Block::default().title("Sync Commander").borders(Borders::ALL));
            f.render_widget(status_block, chunks[0]);

            let logs: Vec<ListItem> = state.logs.iter().rev().map(|s| ListItem::new(s.as_str())).collect();
            let log_list = List::new(logs)
                .block(Block::default().title("Logs").borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(log_list, chunks[1]);
        })?;

        // Input Handling
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

        // Handle Events from Fast or Slow Loops
        while let Ok(_) = rx.try_recv() {
            // Debounce: Wait 2 seconds of silence before syncing
            debounce_timer = Some(tokio::time::Instant::now() + Duration::from_secs(2));

            let mut s = app_state.lock().unwrap();
            if s.status != "SYNCING..." {
                s.status = "PENDING...".to_string();
            }
        }

        if let Some(time) = debounce_timer {
            if tokio::time::Instant::now() >= time {
                debounce_timer = None;
                let s = app_state.clone();
                tokio::spawn(async move { run_rsync(s).await });
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    Ok(())
}