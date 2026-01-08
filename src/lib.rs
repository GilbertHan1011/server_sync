pub mod protocol;

pub mod common {
    pub mod utils;
}

pub mod server {
    pub mod state;
    pub mod ssh;
    pub mod worker;
    pub mod handler;
}

pub mod client {
    pub mod config;
    pub mod state;
    pub mod network;
    pub mod ui;
    pub mod handler;
}