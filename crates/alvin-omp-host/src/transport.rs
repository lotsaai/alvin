//! The omp RPC transport: JSON lines over the stdio of a long-running `omp --mode rpc-ui` process. Adapted from
//! `omp-rpc`'s client (MIT), which supplies the wire types this module builds on.

mod client;
mod frame;
mod host_tool;
mod host_uri;
mod turn;

pub use client::{Client, ClientOptions, Error, Event, encode_command};
pub use host_tool::{HostTool, HostToolContext, HostToolError, HostToolOutput};
pub use host_uri::{HostUri, HostUriContext, HostUriError, HostUriRead};
pub use turn::PromptTurn;
