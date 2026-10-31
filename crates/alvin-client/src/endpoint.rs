//! Where the gateway is: an in-process channel pair or a WebSocket URL.

use std::str::FromStr;

use url::Url;

use crate::Error;

/// The client's ends of an in-process connection, as handed out by the embedded gateway.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct ChannelPair {
  pub(crate) sender: tokio::sync::mpsc::Sender<String>,
  pub(crate) receiver: tokio::sync::mpsc::Receiver<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl From<(tokio::sync::mpsc::Sender<String>, tokio::sync::mpsc::Receiver<String>)> for ChannelPair {
  fn from((sender, receiver): (tokio::sync::mpsc::Sender<String>, tokio::sync::mpsc::Receiver<String>)) -> Self {
    Self { sender, receiver }
  }
}

/// How to reach a gateway.
#[derive(Debug)]
pub enum Endpoint {
  /// In-process: the string channel pair of an embedded gateway. Not available in the browser, which has no
  /// embedded gateway.
  #[cfg(not(target_arch = "wasm32"))]
  Channel(ChannelPair),
  /// A `ws://` or `wss://` URL.
  WebSocket(Url),
}

impl FromStr for Endpoint {
  type Err = Error;

  fn from_str(value: &str) -> Result<Self, Error> {
    let url = Url::parse(value)?;
    match url.scheme() {
      "ws" | "wss" => Ok(Self::WebSocket(url)),
      scheme => Err(Error::UnsupportedScheme { scheme: scheme.to_owned() }),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::Endpoint;
  use crate::Error;

  #[test]
  fn ws_and_wss_urls_parse_to_websocket_endpoints() {
    for url in ["ws://127.0.0.1:7000", "wss://alvin.example/rpc"] {
      let endpoint = url.parse::<Endpoint>().unwrap();
      assert!(matches!(&endpoint, Endpoint::WebSocket(parsed) if parsed.as_str().starts_with(url.trim_end_matches('/'))));
    }
  }

  #[test]
  fn other_schemes_are_rejected_with_their_name() {
    for scheme in ["http", "https", "tcp", "stdio"] {
      let error = format!("{scheme}://127.0.0.1:7000").parse::<Endpoint>().unwrap_err();
      assert!(
        matches!(&error, Error::UnsupportedScheme { scheme: rejected } if rejected == scheme),
        "{error:?}"
      );
    }
  }

  #[test]
  fn a_string_that_is_not_a_url_is_an_invalid_url() {
    let error = "not a url".parse::<Endpoint>().unwrap_err();
    assert!(matches!(error, Error::InvalidUrl(_)), "{error:?}");
  }
}
