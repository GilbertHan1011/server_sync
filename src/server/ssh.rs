use std::fs;
use std::os::unix::fs::PermissionsExt;
use tokio::process::Command;
use crate::common::utils::is_valid_host;

// --- REMOTE DIRECTORY LISTING ---
pub async fn list_remote_dirs_ssh(remote_host: &str, port: Option<u16>, path: &str, password: &Option<String>) -> Vec<String> {
    // SECURITY: Validate host before using in SSH command
    if !is_valid_host(remote_host) {
        return vec!["Error: Invalid remote host format".to_string()];
    }

    // Default to current directory if empty
    let target_path = if path.is_empty() { "." } else { path };
    let p = port.unwrap_or(22).to_string();
    let mut cmd = Command::new("ssh");

    // --- ASKPASS LOGIC ---
    if let Some(pass) = password {
        // 1. Create the helper script
        if let Ok(script_path) = setup_askpass_script() {
            // 2. Set Env Vars for SSH to pick up
            cmd.env("SSH_ASKPASS", &script_path);
            cmd.env("SSH_ASKPASS_REQUIRE", "force"); // Force askpass even if no TTY
            cmd.env("SERVER_SYNC_PW", pass);         // The password itself
            cmd.env("DISPLAY", ":0");                // Dummy display to trick old SSH versions
        }
    }
    // ---------------------

    // Run: ssh user@host "ls -1F --group-directories-first /path"
    // OPTIMIZED SSH: Uses ControlMaster and AES-GCM cipher for speed
    let output = cmd
        .arg("-p").arg(&p)
        .arg("-T")  // Disable pseudo-tty (faster)
        .arg("-c").arg("aes128-gcm@openssh.com")  // Fastest hardware cipher
        .arg("-o").arg("Compression=no")  // Don't compress directory listings
        .arg("-o").arg("StrictHostKeyChecking=no") // Auto-accept host keys
        .arg("-o").arg("ControlMaster=auto")
        .arg("-o").arg("ControlPath=~/.ssh/sockets/%r@%h-%p")
        .arg("-o").arg("ControlPersist=600")
        .arg(remote_host)
        .arg(format!("ls -1F --group-directories-first {}", target_path))
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let raw = String::from_utf8_lossy(&o.stdout);
            raw.lines()
                .filter(|line| line.ends_with('/')) // Only show directories
                .map(|line| line.replace("/", ""))   // Remove the trailing slash for display
                .collect()
        }
        Ok(o) => vec![format!("SSH Error: {}", String::from_utf8_lossy(&o.stderr))],
        Err(e) => vec![format!("Exec Error: {}", e)],
    }
}

pub async fn get_remote_home_ssh(remote_host: &str, port: Option<u16>, password: &Option<String>) -> String {
    if !is_valid_host(remote_host) {
        return "/".to_string();
    }

    let p = port.unwrap_or(22).to_string();
    let mut cmd = Command::new("ssh");
    if let Some(pass) = password {
        if let Ok(script_path) = setup_askpass_script() {
            cmd.env("SSH_ASKPASS", &script_path);
            cmd.env("SSH_ASKPASS_REQUIRE", "force");
            cmd.env("SERVER_SYNC_PW", pass);
            cmd.env("DISPLAY", ":0");
        }
    }

    let output = cmd
        .arg("-p").arg(&p)
        .arg("-T")
        .arg("-c").arg("aes128-gcm@openssh.com")
        .arg("-o").arg("StrictHostKeyChecking=no")
        .arg("-o").arg("ControlMaster=auto")
        .arg("-o").arg("ControlPath=~/.ssh/sockets/%r@%h-%p")
        .arg("-o").arg("ControlPersist=600")
        .arg(remote_host)
        .arg("pwd") // <--- The command to run
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => "/".to_string(),
    }
}

pub fn setup_askpass_script() -> std::io::Result<String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        // Ensure directory exists
        let dir = format!("{}/.ssh/sockets", home);
        fs::create_dir_all(&dir)?;
        
        let script_path = format!("{}/askpass_wrapper.sh", dir);
        
        // Simple script: output the env var content
        let content = "#!/bin/sh\necho \"$SERVER_SYNC_PW\"";
        
        // Write synchronously (fast enough for small file)
        fs::write(&script_path, content)?;
        
        // Set executable permission (chmod 700)
        let mut perms = fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&script_path, perms)?;
        
        Ok(script_path)
    }