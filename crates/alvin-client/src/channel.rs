//! The channel transport: jsonrpsee's string transport over an in-process channel pair.

use jsonrpsee::core::client::{Client, ClientBuilder, ReceivedMessage, TransportReceiverT, TransportSenderT};
use tokio::sync::mpsc;

use crate::endpoint::ChannelPair;

/// The gateway's half of the channel went away.
#[derive(Debug, thiserror::Error)]
#[error("in-process channel closed")]
pub struct Closed;

struct Sender(mpsc::Sender<String>);

struct Receiver(mpsc::Receiver<String>);

impl TransportSenderT for Sender {
  type Error = Closed;

  async fn send(&mut self, message: String) -> Result<(), Closed> {
    self.0.send(message).await.map_err(|_unsent| Closed)
  }
}

impl TransportReceiverT for Receiver {
  type Error = Closed;

  async fn receive(&mut self) -> Result<ReceivedMessage, Closed> {
    self.0.recv().await.map(ReceivedMessage::Text).ok_or(Closed)
  }
}

/// Starts a JSON-RPC client over the pair. Must be called inside a tokio runtime.
pub fn connect(pair: ChannelPair) -> Client {
  ClientBuilder::default().build_with_tokio(Sender(pair.sender), Receiver(pair.receiver))
}
