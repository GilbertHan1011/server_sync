use std::panic;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use simplelog::*;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
    widgets::ListState,
};
use crate::protocol::{ClientRequest, ServerResponse, SyncMode, SyncDirection};
use crate::client::state::{App, AppMode};
use crate::client::network::send_req;
use crate::client::ui::draw;
use crate::client::handler::{handle_key_event, HandlerResult};
use crate::client::config::load_hosts;
use crate::common::daemon;

// Global flag for Ctrl+C
static CTRL_C_PRESSED: AtomicBool = AtomicBool::new(false);

/// Main client logic - runs the TUI
pub async fn run_client() -> anyhow::Result<()> {
    // 0. Initialize Logging (MOVED to hidden dir)
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let log_dir = format!("{}/.sync_daemon_logs", home);
    
    // Ensure directory exists
    std::fs::create_dir_all(&log_dir)?;
    
    let log_path = format!("{}/client.log", log_dir);

    WriteLogger::init(
        LevelFilter::Info,
        Config::default(),
        File::create(log_path)?, // Truncates on every start, preventing infinite growth
    )?;
    
    log::info!("Client starting...");

    // 0.5. Setup Ctrl+C handler (BEFORE terminal setup)
    ctrlc::set_handler(|| {
        CTRL_C_PRESSED.store(true, Ordering::Relaxed);
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen, crossterm::event::DisableMouseCapture);
    })?;

    // 1. Install Panic Hook (The Safety Net)
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
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
        dashboard_selected_idx: 0,
        dashboard_list_state: ListState::default(),
        current_path: start_path,
        dir_entries: vec![],
        selected_idx: 0,
        browser_list_state: ListState::default(),
        pending_source: String::new(),
        remote_current_path: String::new(),
        pending_remote_host: String::new(),
        pending_remote_port: Some(22),
        input_remote_host: String::new(),
        input_remote_port: String::from("22"),
        input_cursor_pos: 0,
        pending_password: None,
        input_password: String::new(),
        show_password: false,
        pending_sync_mode: SyncMode::Mirror,
        pending_sync_direction: SyncDirection::Push,
        sync_direction_selected_idx: 0,
        pending_compress: true,
        sync_mode_selected_idx: 0,
        dry_run_results: vec![],
        dry_run_task_id: String::new(),
        dry_run_scroll: 0,
        view_task_log: String::new(),
        view_log_scroll: 0,
        view_log_task_id: String::new(),
        view_log_last_fetch: std::time::Instant::now(),
        saved_hosts: load_hosts(),
        host_list_idx: 0,
        is_editing_host: false,
        input_new_dir: String::new(),
        server_status: None,
        server_status_last_check: std::time::Instant::now(),
    };


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
                    app.server_status = Some(true);
                }
                ServerResponse::Error(e) => {
                    if e.contains("not running") {
                        app.tasks = vec![];
                        app.server_status = Some(false);
                    }
                }
                _ => {}
            }

            // Periodically check server status (every 3 seconds)
            let now = std::time::Instant::now();
            if now.duration_since(app.server_status_last_check).as_secs() >= 3 {
                app.server_status = Some(daemon::is_server_running());
                app.server_status_last_check = now;
            }
        }

        // Auto-refresh logs when in LogView mode
        if matches!(app.mode, AppMode::LogView) {
            let now = std::time::Instant::now();
            if now.duration_since(app.view_log_last_fetch).as_secs() >= 2 {
                let tid = app.view_log_task_id.clone();
                match send_req(ClientRequest::GetTaskLog(tid.clone())) {
                    ServerResponse::TaskLog(_, content) => {
                        app.view_task_log = content;
                        app.view_log_last_fetch = now;
                        // Maintain scroll position (don't reset to bottom on auto-refresh)
                    }
                    _ => {}
                }
            }
        }

        // 2. RENDER
        terminal.draw(|f| {
            draw(f, &mut app);
        }).map_err(|_e| anyhow::anyhow!("Terminal draw error"))?;

        // 3. INPUT - Reduced timeout for responsive typing
        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Handle Ctrl+C explicitly (backup to signal handler)
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    log::info!("Ctrl+C pressed, exiting...");
                    break;
                }
                
                // Handle key event using handler module
                match handle_key_event(key, &mut app) {
                    HandlerResult::Quit => break,
                    HandlerResult::Continue => {}
                }
            }
        }
    }

    Ok(())
}
