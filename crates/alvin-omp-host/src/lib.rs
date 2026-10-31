//! Supervises one `omp --mode rpc-ui` process per live session. The only crate that talks to omp.

mod bridge;
mod error;
mod host;
mod session;
#[cfg(test)]
mod test_support;
mod translate;

pub use error::{Error, Result};
pub use host::{Host, HostConfig, StartRequest};
pub use session::Session;
pub use translate::{Translation, Untranslated, translate};
