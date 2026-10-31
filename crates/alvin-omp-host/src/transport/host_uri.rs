//! Host-owned URI schemes the agent reads and writes (`set_host_uri_schemes`, `host_uri_request`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use omp_rpc::{HostUriOperation, HostUriResultContentType, HostUriSchemeDefinition};

use super::client::Error;
use super::host_tool::HostToolError;

/// Error a handler returns; its text becomes the request's error result.
pub type HostUriError = HostToolError;

type ReadFuture = Pin<Box<dyn Future<Output = Result<HostUriRead, HostUriError>> + Send>>;
type WriteFuture = Pin<Box<dyn Future<Output = Result<(), HostUriError>> + Send>>;

type ReadHandler = dyn Fn(HostUriContext) -> ReadFuture + Send + Sync;
type WriteHandler = dyn Fn(HostUriContext, String) -> WriteFuture + Send + Sync;

/// A `<scheme>://` namespace served by the host.
#[derive(Clone)]
pub struct HostUri {
  scheme: String,
  description: Option<String>,
  immutable: bool,
  read: Arc<ReadHandler>,
  write: Option<Arc<WriteHandler>>,
}

impl HostUri {
  /// `scheme` is trimmed and lowercased and must not be empty. The read handler runs as a task of its own per request
  /// and gets the request's context, which carries the full URL.
  pub fn new<F, Fut>(scheme: &str, read: F) -> Result<Self, Error>
  where
    F: Fn(HostUriContext) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<HostUriRead, HostUriError>> + Send + 'static,
  {
    let scheme = scheme.trim().to_lowercase();
    if scheme.is_empty() {
      return Err(Error::InvalidArgument("host URI scheme must be a non-empty string".to_owned()));
    }
    Ok(Self {
      scheme,
      description: None,
      immutable: false,
      read: Arc::new(move |context| Box::pin(read(context))),
      write: None,
    })
  }

  /// Makes the scheme writable; the handler gets the request's context and the content to write.
  pub fn write<F, Fut>(mut self, handler: F) -> Self
  where
    F: Fn(HostUriContext, String) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<(), HostUriError>> + Send + 'static,
  {
    self.write = Some(Arc::new(move |context, content| Box::pin(handler(context, content))));
    self
  }

  /// Describes the scheme to the model.
  pub fn description(mut self, description: impl Into<String>) -> Self {
    self.description = Some(description.into());
    self
  }

  /// Marks the scheme's content as immutable.
  pub fn immutable(mut self, immutable: bool) -> Self {
    self.immutable = immutable;
    self
  }

  /// The scheme, lowercase.
  pub fn scheme(&self) -> &str {
    &self.scheme
  }

  /// Whether the scheme has a write handler.
  pub fn writable(&self) -> bool {
    self.write.is_some()
  }

  /// The entry sent with `set_host_uri_schemes`.
  pub fn definition(&self) -> HostUriSchemeDefinition {
    HostUriSchemeDefinition {
      scheme: self.scheme.clone(),
      description: self.description.clone(),
      writable: Some(self.writable()),
      immutable: Some(self.immutable),
    }
  }

  pub(crate) async fn run_read(&self, context: HostUriContext) -> Result<HostUriRead, HostUriError> {
    (self.read)(context).await
  }

  /// `None` when the scheme has no write handler.
  pub(crate) async fn run_write(&self, context: HostUriContext, content: String) -> Option<Result<(), HostUriError>> {
    match &self.write {
      Some(write) => Some(write(context, content).await),
      None => None,
    }
  }
}

impl fmt::Debug for HostUri {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("HostUri")
      .field("scheme", &self.scheme)
      .field("description", &self.description)
      .field("immutable", &self.immutable)
      .field("writable", &self.writable())
      .finish_non_exhaustive()
  }
}

/// A read handler's result; `From<String>` and `From<&str>` give the text shortcut.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HostUriRead {
  /// The content.
  pub content: String,
  /// Its content type.
  pub content_type: Option<HostUriResultContentType>,
  /// Notes for the model.
  pub notes: Option<Vec<String>>,
  /// Whether this content is immutable.
  pub immutable: Option<bool>,
}

impl From<String> for HostUriRead {
  fn from(content: String) -> Self {
    Self {
      content,
      ..Self::default()
    }
  }
}

impl From<&str> for HostUriRead {
  fn from(content: &str) -> Self {
    Self::from(content.to_owned())
  }
}

/// Per-request context handed to a handler.
pub struct HostUriContext {
  pub(crate) url: String,
  pub(crate) operation: HostUriOperation,
  pub(crate) cancelled: Arc<AtomicBool>,
}

impl HostUriContext {
  /// The full URL of the request.
  pub fn url(&self) -> &str {
    &self.url
  }

  /// Whether this is a read or a write.
  pub fn operation(&self) -> HostUriOperation {
    self.operation
  }

  /// Whether the server cancelled the request or the client closed. A handler checks this at its own pace; nothing
  /// interrupts it.
  pub fn is_cancelled(&self) -> bool {
    self.cancelled.load(Ordering::SeqCst)
  }
}

/// The URL's scheme, lowercased; empty when the URL has none.
pub(crate) fn uri_scheme(url: &str) -> String {
  url
    .split_once(':')
    .map(|(scheme, _)| scheme)
    .filter(|scheme| {
      let mut chars = scheme.chars();
      chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|char| char.is_ascii_alphanumeric() || matches!(char, '+' | '-' | '.'))
    })
    .map_or_else(String::new, str::to_ascii_lowercase)
}
