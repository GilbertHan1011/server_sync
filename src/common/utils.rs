use regex::Regex;

pub fn get_socket_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_daemon.sock", home)
}

pub fn get_host_path() -> String {
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    format!("{}/.sync_hosts", home)
}

// --- SECURITY: INPUT VALIDATION ---
pub fn is_valid_host(host: &str) -> bool {
    // Only allow alphanumeric, dots, hyphens, and one '@'
    // This prevents SSH flag injection (e.g. -oProxyCommand)
    let re = Regex::new(r"^[a-zA-Z0-9.-]+(@[a-zA-Z0-9.-]+)?$").unwrap();
    re.is_match(host)
}