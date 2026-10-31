//! Application boundary: implements the alvin protocol server traits and serves them over an in-process channel
//! and over WebSocket.

mod channel;
mod config;
mod error;
mod gateway;
mod listener;
mod live;
mod omp_dirs;
mod rpc;
mod telemetry;

pub use alvin_omp_host::HostConfig;
pub use config::GatewayConfig;
pub use error::{Error, Result};
pub use gateway::Gateway;
pub use listener::WsServer;
pub use omp_dirs::sessions_dir;
pub use telemetry::{Service, Telemetry};
