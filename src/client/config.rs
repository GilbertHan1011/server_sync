use crate::common::utils::get_host_path;

pub fn load_hosts() -> Vec<String> {
    let path = get_host_path();
    if let Ok(content) = std::fs::read_to_string(path) {
        content.lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with("#"))
            .collect()
    } else {
        vec![]
    }
}

pub fn save_hosts(hosts: &Vec<String>) {
    let path = get_host_path();
    let content = hosts.join("\n");
    let _ = std::fs::write(path, content);
}
