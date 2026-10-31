//! Read-only view of omp's session files, grouped by project.

mod error;
mod grouping;

#[cfg(test)]
mod test_support;

pub use error::{Error, Result};
pub use grouping::group;
