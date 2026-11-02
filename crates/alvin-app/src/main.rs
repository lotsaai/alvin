//! `alvin-app`: the desktop app. Parses settings, starts telemetry and runs the window until the user quits.

mod app;
mod config;
mod settings;
#[cfg(test)]
mod test_support;

use alvin_gateway::{Service, Telemetry};
use clap::Parser as _;

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
