//! The WebSocket transport: jsonrpsee's WebSocket client on native targets, its browser client on wasm32.

use jsonrpsee::core::client::Client;
use url::Url;

use crate::Result;

/// Connects to the gateway at `url`.
#[cfg(not(target_arch = "wasm32"))]
pub async fn connect(url: &Url) -> Result<Client> {
  Ok(jsonrpsee::ws_client::WsClientBuilder::default().build(url).await?)
}

/// Connects to the gateway at `url`.
#[cfg(target_arch = "wasm32")]
pub async fn connect(url: &Url) -> Result<Client> {
  Ok(jsonrpsee::wasm_client::WasmClientBuilder::default().build(url).await?)
}
