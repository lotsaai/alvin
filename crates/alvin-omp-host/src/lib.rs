//! Supervises one `omp --mode rpc-ui` process per live session. The only crate that talks to omp.

mod bridge;
mod error;
mod history;
mod host;
mod session;
#[cfg(test)]
mod test_support;
mod translate;
mod transport;

pub use error::{Error, Result};
pub use host::{Host, HostConfig, StartRequest};
pub use session::Session;
pub use translate::{Untranslated, translate};
pub use transport::{
  Client, ClientOptions, Error as RpcError, Event, HostTool, HostToolContext, HostToolError, HostToolOutput, HostUri, HostUriContext,
  HostUriError, HostUriRead, PromptTurn, encode_command,
};
