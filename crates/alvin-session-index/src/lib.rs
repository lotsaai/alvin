//! Read-only view of omp's session files, grouped by project.

mod error;
mod grouping;
mod header;
mod scan;

#[cfg(test)]
mod test_support;

pub use error::{Error, Result};
pub use grouping::group;
pub use header::SessionHead;
pub use scan::scan;
