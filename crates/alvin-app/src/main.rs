//! `alvin-app`: the desktop app. Parses settings, starts telemetry and runs the window until the user quits.

mod app;
mod config;
mod error;
mod project_preferences;
mod settings;
mod terminal_font;
#[cfg(test)]
mod test_support;

use alvin_gateway::{Service, Telemetry};
use clap::Parser as _;
use error::Error;

type Result<T, E = Error> = std::result::Result<T, E>;

fn main() -> eyre::Result<()> {
  let settings = config::Cli::parse().settings()?;
  // Telemetry is built before the runtime: the OTLP exporter creates a blocking HTTP client.
  let telemetry = Telemetry::init(&Service {
    name: "alvin-app",
    version: env!("CARGO_PKG_VERSION"),
  })?;
  let outcome = app::run(settings);
  telemetry.shutdown();
  outcome
}
