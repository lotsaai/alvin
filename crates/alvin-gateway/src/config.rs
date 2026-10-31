//! The typed configuration faces build the gateway from.

use std::path::PathBuf;

use typed_builder::TypedBuilder;

/// What the gateway needs to run. Faces build it; the gateway reads no environment and chooses no defaults.
#[derive(Debug, Clone, TypedBuilder)]
pub struct GatewayConfig {
  /// omp's sessions directory, resolved by the face.
  #[builder(setter(into))]
  pub sessions_dir: PathBuf,
}
