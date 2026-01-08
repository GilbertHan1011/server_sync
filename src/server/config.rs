use serde::Deserialize;
use std::fs;
use clap::Parser;

// --- CLI ARGUMENTS ---
#[derive(Parser)]
#[command(name = "server_sync")]
#[command(about = "File synchronization daemon server", long_about = None)]
pub struct ServerArgs {
    /// Path to configuration file
    #[arg(short, long, default_value = "server_config.yaml")]
    pub config: String,
    
    /// Path to log file (stdout if not provided)
    #[arg(short, long)]
    pub log: Option<String>,
    
    /// Run in foreground instead of daemonizing
    #[arg(short, long)]
    pub foreground: bool,
}

// --- SERVER CONFIG ---
#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub remote_host: String,
}

pub fn load_server_config(config_path: &str) -> ServerConfig {
    match fs::read_to_string(config_path) {
        Ok(content) => {
            match serde_yaml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!("Failed to parse {}: {}. Using default config.", config_path, e);
                    ServerConfig {
                        remote_host: "user@remote".to_string(),
                    }
                }
            }
        }
        Err(_) => {
            eprintln!("Config file {} not found. Using default config.", config_path);
            ServerConfig {
                remote_host: "user@remote".to_string(),
            }
        }
    }
}
