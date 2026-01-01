use serde::{Deserialize, Serialize};

/// Server state that is sent to clients
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerState {
    pub logs: Vec<String>,
    pub status: String,
    pub sync_count: u32,
}

impl Default for ServerState {
    fn default() -> Self {
        Self {
            logs: vec![],
            status: "DISCONNECTED".to_string(),
            sync_count: 0,
        }
    }
}

/// Commands that clients can send to the server
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Command {
    ForceSync,
    Ping,
}

