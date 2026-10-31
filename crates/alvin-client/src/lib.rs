//! The typed alvin client. One [`Client`] type serves every UI; the transport is picked by [`Endpoint`].

#[cfg(not(target_arch = "wasm32"))]
mod channel;
mod client;
mod endpoint;
mod error;
mod ws;

pub use client::Client;
#[cfg(not(target_arch = "wasm32"))]
pub use endpoint::ChannelPair;
pub use endpoint::Endpoint;
pub use error::{Error, Result};
