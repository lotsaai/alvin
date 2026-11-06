//! Blocking local speech transcription independent of any UI or async runtime.

mod error;
mod model;
mod transcriber;

pub use error::{Error, Result};
pub use model::PreparationProgress;
pub use transcriber::{Stream, Transcriber};
