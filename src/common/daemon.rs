use std::fs;
use std::path::Path;
use std::process::Command;
use sysinfo::{Pid, System};
use anyhow::Result;

/// Get the path to the PID file
pub fn get_pid_file() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.sync_daemon.pid", home)
}

/// Get the path to the log file
pub fn get_log_file() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    format!("{}/.sync_daemon_logs/daemon.log", home)
}

/// Get the server PID from the PID file
pub fn get_server_pid() -> Option<u32> {
    let pid_file = get_pid_file();
    if let Ok(content) = fs::read_to_string(&pid_file) {
        return content.trim().parse::<u32>().ok();
    }
    None
}

/// Check if the server is currently running
pub fn is_server_running() -> bool {
    if let Some(pid) = get_server_pid() {
        let mut sys = System::new_all();
        sys.refresh_all();
        if sys.process(Pid::from(pid as usize)).is_some() {
            return true;
        }
        // Process not found, clean up stale PID file
        let _ = fs::remove_file(get_pid_file());
    }
    false
}

/// Spawn the server process in the background
pub fn spawn_server() -> Result<()> {
    let exe = std::env::current_exe()?;
    Command::new(exe)
        .arg("server")
        .spawn()?;
    Ok(())
}

/// Kill the server process
pub fn kill_server() -> Result<()> {
    if let Some(pid) = get_server_pid() {
        let mut sys = System::new_all();
        sys.refresh_all();
        if let Some(process) = sys.process(Pid::from(pid as usize)) {
            process.kill();
            println!("Stopped server (PID: {})", pid);
            // Clean up PID file
            let _ = fs::remove_file(get_pid_file());
            Ok(())
        } else {
            println!("Process not found (stale PID file?). Cleaning up.");
            let _ = fs::remove_file(get_pid_file());
            Ok(())
        }
    } else {
        println!("No server running.");
        Ok(())
    }
}

/// Ensure the log directory exists
pub fn ensure_log_directory() -> Result<()> {
    let log_file = get_log_file();
    if let Some(parent) = Path::new(&log_file).parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}
