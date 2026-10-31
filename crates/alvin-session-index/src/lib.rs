//! Read-only view of omp's session files, grouped by project.

mod error;
mod grouping;
mod header;
mod listing;
mod scan;
mod status;

#[cfg(test)]
mod test_support;

pub use error::{Error, Result};
pub use grouping::{group, select};
pub use header::SessionHead;
pub use listing::list_page;
pub use scan::scan;
