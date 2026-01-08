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
};
use server_sync::protocol::{ClientRequest, ServerResponse, SyncMode};
use server_sync::client::state::{App, AppMode};
use server_sync::client::network::send_req;
use server_sync::client::ui::draw;
use server_sync::client::handler::{handle_key_event, HandlerResult};
use server_sync::client::config::load_hosts;

// Global flag for Ctrl+C
static CTRL_C_PRESSED: AtomicBool = AtomicBool::new(false);

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
        current_path: start_path,
        dir_entries: vec![],
        selected_idx: 0,
        pending_source: String::new(),
        remote_current_path: String::new(),
        pending_remote_host: String::new(),
        input_remote_host: String::new(),
        input_cursor_pos: 0,
        pending_password: None,
        input_password: String::new(),
        show_password: false,
        pending_sync_mode: SyncMode::Mirror,
        pending_compress: true,
        sync_mode_selected_idx: 0,
        dry_run_results: vec![],
        dry_run_task_id: String::new(),
        dry_run_scroll: 0,
        saved_hosts: load_hosts(),
        host_list_idx: 0,
        is_editing_host: false,
    };

    // Fetch remote host from server on startup
    match send_req(ClientRequest::GetRemoteHost) {
        ServerResponse::RemoteHost(host) => {
            app.pending_remote_host = host;
        }
        ServerResponse::Error(e) => {
            eprintln!("Warning: Could not fetch remote host from server: {}", e);
            app.pending_remote_host = "user@remote".to_string();
        }
        _ => {
            eprintln!("Warning: Unexpected response when fetching remote host");
            app.pending_remote_host = "user@remote".to_string();
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
                    if e.contains("not running") {
                        app.tasks = vec![];
                    }
                }
                _ => {}
            }
        }

        // 2. RENDER
        terminal.draw(|f| {
            draw(f, &app);
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
