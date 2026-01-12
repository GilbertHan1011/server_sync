use clap::{Parser, Subcommand};
use std::fs::File;
use daemonize::Daemonize;
use server_sync::common::daemon::{self, ensure_log_directory};
use tokio::runtime::Runtime;

#[derive(Parser)]
#[command(name = "sync_app")]
#[command(about = "A unified sync tool with TUI and Daemon", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the TUI Dashboard (Default)
    Ui,
    /// Start the background sync daemon
    Start,
    /// Stop the background sync daemon
    Stop,
    /// Restart the daemon
    Restart,
    /// Check daemon status
    Status,
    /// (Internal) Run the server logic directly
    #[command(hide = true)]
    Server,
}

fn main() {
    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Commands::Ui);

    match command {
        Commands::Ui => {
            // Initialize async runtime for client
            let rt = Runtime::new().unwrap();
            // Auto-start server if not running
            if !daemon::is_server_running() {
                println!("Server not running. Starting it...");
                if let Err(e) = daemon::spawn_server() {
                    eprintln!("Failed to start server: {}", e);
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            if let Err(e) = rt.block_on(server_sync::client_main::run_client()) {
                eprintln!("Error running client: {}", e);
            }
        }
        Commands::Server => {
            // Ensure log directory exists
            if let Err(e) = ensure_log_directory() {
                eprintln!("Warning: Failed to create log directory: {}", e);
            }

            // 1. Daemonize (Detach from terminal)
            let log_file_path = daemon::get_log_file();
            let stdout = match File::create(&log_file_path) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to create log file {}: {}", log_file_path, e);
                    return;
                }
            };
            let stderr = match stdout.try_clone() {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Failed to clone log file: {}", e);
                    return;
                }
            };

            let pid_file = daemon::get_pid_file();
            let daemonize = Daemonize::new()
                .pid_file(&pid_file)
                .chown_pid_file(true)
                .working_directory(".")
                .stdout(stdout)
                .stderr(stderr);

            match daemonize.start() {
                Ok(_) => {
                    // We are now in the background! Create runtime AFTER daemonization
                    let rt = Runtime::new().unwrap();
                    if let Err(e) = rt.block_on(server_sync::server_main::run_server()) {
                        eprintln!("Server error: {}", e);
                    }
                }
                Err(e) => eprintln!("Error daemonizing: {}", e),
            }
        }
        Commands::Start => {
            if daemon::is_server_running() {
                println!("Server is already running.");
            } else {
                if let Err(e) = daemon::spawn_server() {
                    eprintln!("Failed to start server: {}", e);
                } else {
                    println!("Server started in background.");
                }
            }
        }
        Commands::Stop => {
            if let Err(e) = daemon::kill_server() {
                eprintln!("Error stopping server: {}", e);
            }
        }
        Commands::Restart => {
            if let Err(e) = daemon::kill_server() {
                eprintln!("Error stopping server: {}", e);
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Err(e) = daemon::spawn_server() {
                eprintln!("Failed to start server: {}", e);
            } else {
                println!("Server restarted.");
            }
        }
        Commands::Status => {
            if daemon::is_server_running() {
                if let Some(pid) = daemon::get_server_pid() {
                    println!("✅ Server is RUNNING (PID: {})", pid);
                } else {
                    println!("✅ Server is RUNNING");
                }
            } else {
                println!("🔴 Server is STOPPED");
            }
        }
    }
}
