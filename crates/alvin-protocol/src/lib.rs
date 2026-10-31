//! Wire contract between the alvin gateway and its clients.

pub mod error_codes;
pub mod ids;
pub mod omp_wire;

mod error;

pub use error::{Error, Result};
pub use ids::{EventId, LiveSessionId};
