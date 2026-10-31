//! The in-process transport: the server half of a string channel pair.
//!
//! The client wraps the pair it gets from [`Gateway::connect_channel`] in its channel transport.

use jsonrpsee::server::Methods;
use tokio::sync::mpsc;

use crate::Gateway;

/// What a server answers to a frame that is not JSON-RPC at all.
const PARSE_ERROR: &str = r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"Parse error"},"id":null}"#;

impl Gateway {
  /// Mints an in-process connection and returns the client's ends: a sender for request frames and a receiver
  /// for response and subscription frames. `capacity` bounds both channels and each subscription's buffer.
  ///
  /// Must be called inside a tokio runtime.
  pub fn connect_channel(&self, capacity: usize) -> (mpsc::Sender<String>, mpsc::Receiver<String>) {
    let (to_server, from_client) = mpsc::channel(capacity);
    let (to_client, from_server) = mpsc::channel(capacity);
    tokio::spawn(serve(self.methods(), from_client, to_client, capacity));
    (to_server, from_server)
  }
}

/// Answers each request frame in its own task, so a slow call never holds up others on the connection. A send
/// failing means the client dropped its receiver, so there is nobody left to answer.
async fn serve(methods: Methods, mut from_client: mpsc::Receiver<String>, to_client: mpsc::Sender<String>, capacity: usize) {
  while let Some(frame) = from_client.recv().await {
    let methods = methods.clone();
    let to_client = to_client.clone();
    tokio::spawn(async move {
      let Ok((response, mut subscription)) = methods.raw_json_request(&frame, capacity).await else {
        // Dropping the send result is right: the receiver is gone only when the client is.
        drop(to_client.send(PARSE_ERROR.to_owned()).await);
        return;
      };
      if to_client.send(response.get().to_owned()).await.is_err() {
        return;
      }
      while let Some(item) = subscription.recv().await {
        if to_client.send(item.get().to_owned()).await.is_err() {
          return;
        }
      }
    });
  }
}
