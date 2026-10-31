//! The typed configuration faces build the gateway from.

use std::path::PathBuf;

use alvin_omp_host::HostConfig;
use typed_builder::TypedBuilder;

/// What the gateway needs to run. Faces build it; the gateway reads no environment and chooses no defaults.
#[derive(Debug, Clone, TypedBuilder)]
pub struct GatewayConfig {
  /// omp's sessions directory, resolved by the face.
  #[builder(setter(into))]
  pub sessions_dir: PathBuf,
  /// How omp is launched: executable, extra arguments, environment.
  pub omp: HostConfig,
  /// The model new and resumed sessions start with when the request names none, as `provider/id`. Absent: omp's own
  /// choice.
  #[builder(default)]
  pub default_model: Option<String>,
}
