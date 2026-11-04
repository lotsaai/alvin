//! OpenTelemetry setup and explicit shutdown for alvin binaries.

mod error;
mod telemetry;

pub use error::{Error, Result};
pub use telemetry::{Service, Telemetry};
