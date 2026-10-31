//! The WebSocket transport: accepts TCP connections and serves the gateway's methods on each.

use std::net::SocketAddr;

use jsonrpsee::server::{Methods, Server, ServerHandle, StopHandle, serve_with_graceful_shutdown, stop_channel};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::{Error, Gateway, Result};

/// A running WebSocket listener.
#[derive(Debug)]
pub struct WsServer {
  local_addr: SocketAddr,
  handle: ServerHandle,
}

impl WsServer {
  /// The address the listener bound, which is how callers learn the port after binding port 0.
  pub fn local_addr(&self) -> SocketAddr {
    self.local_addr
  }

  /// Stops accepting connections, lets in-flight requests finish, and waits for every connection to close.
  pub async fn shutdown(self) -> Result<()> {
    self.handle.stop()?;
    self.handle.stopped().await;
    Ok(())
  }
}

impl Gateway {
  /// Binds `addr` and serves the gateway's methods over WebSocket until [`WsServer::shutdown`].
  pub async fn serve_ws(&self, addr: SocketAddr) -> Result<WsServer> {
    let listener = TcpListener::bind(addr).await.map_err(Error::Bind)?;
    let local_addr = listener.local_addr().map_err(Error::Bind)?;
    let (stop, handle) = stop_channel();
    tokio::spawn(accept(self.methods()?, listener, stop));
    Ok(WsServer { local_addr, handle })
  }
}

async fn accept(methods: Methods, listener: TcpListener, stop: StopHandle) {
  loop {
    let stream = tokio::select! {
      accepted = listener.accept() => match accepted {
        Ok((stream, _peer)) => stream,
        Err(error) => {
          tracing::warn!(%error, "accepting a WebSocket connection failed");
          continue;
        }
      },
      () = stop.clone().shutdown() => return,
    };
    tokio::spawn(serve_connection(stream, methods.clone(), stop.clone()));
  }
}

/// Serves one connection of any transport stream: a TLS acceptor wraps the stream before it gets here.
async fn serve_connection<I>(io: I, methods: Methods, stop: StopHandle)
where
  I: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
  let service = Server::builder().to_service_builder().build(methods, stop.clone());
  if let Err(error) = serve_with_graceful_shutdown(io, service, stop.shutdown()).await {
    tracing::debug!(%error, "WebSocket connection ended with an error");
  }
}
