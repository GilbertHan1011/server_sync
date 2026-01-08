use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use crate::protocol::{ClientRequest, ServerResponse};
use crate::common::utils::get_socket_path;

pub fn send_req(req: ClientRequest) -> ServerResponse {
    let socket_path = get_socket_path();
    log::info!("Sending request: {:?}", req); // Log before connecting

    match UnixStream::connect(&socket_path) {
        Ok(mut stream) => {
            // 1. Set Timeout (Fixes the hang)
            if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(3))) {
                log::error!("Failed to set timeout: {}", e);
            }
            if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(3))) {
                log::error!("Failed to set timeout: {}", e);
            }

            let json = match serde_json::to_string(&req) {
                Ok(j) => j,
                Err(e) => {
                    return ServerResponse::Error(format!("Serialization error: {}", e));
                }
            };
            
            if stream.write_all(json.as_bytes()).is_err() {
                return ServerResponse::Error("Failed to send request".to_string());
            }
            
            let mut buf = vec![0; 65535];
            match stream.read(&mut buf) {
                Ok(n) if n > 0 => {
                    log::info!("Received response ({} bytes)", n); // Log success
                    match serde_json::from_slice(&buf[..n]) {
                        Ok(resp) => resp,
                        Err(e) => {
                            ServerResponse::Error(format!("Parse error: {}", e))
                        }
                    }
                }
                Ok(_) => ServerResponse::Error("Empty response".to_string()),
                Err(e) => {
                    log::error!("Read error (Server timed out?): {}", e);
                    ServerResponse::Error(format!("Read error: {}", e))
                }
            }
        }
        Err(e) => {
            log::warn!("Could not connect to daemon: {}", e);
            ServerResponse::Error("Daemon not running!".to_string())
        }
    }
}