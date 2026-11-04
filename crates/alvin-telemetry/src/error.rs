//! Errors raised while initializing telemetry.

/// Errors raised while initializing telemetry.
#[derive(Debug, thiserror::Error)]
pub enum Error {
  /// Setting up telemetry failed.
  #[error("setting up telemetry")]
  Telemetry(#[from] init_tracing_opentelemetry::Error),

  /// The log filter in `ALVIN_LOG` or `RUST_LOG` does not parse.
  #[error("parsing the log filter")]
  LogFilter(#[from] tracing_subscriber::filter::ParseError),
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
