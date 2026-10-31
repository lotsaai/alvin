//! `alvind`: the standalone gateway. Parses settings, wires telemetry and the WebSocket listener, and runs until Ctrl-C.

mod settings;

use alvin_gateway::{Gateway, Service, Telemetry};
use clap::Parser as _;
use settings::{Cli, Settings};

fn main() -> eyre::Result<()> {
  let settings = Cli::parse().settings()?;
  // Telemetry is built before the runtime: the OTLP exporter creates a blocking HTTP client.
  let telemetry = Telemetry::init(&Service {
    name: "alvind",
    version: env!("CARGO_PKG_VERSION"),
  })?;
  let outcome = run(settings);
  telemetry.shutdown();
  outcome
}

fn run(settings: Settings) -> eyre::Result<()> {
  tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
    let gateway = Gateway::new(settings.gateway);
    let server = gateway.serve_ws(settings.listen).await?;
    tracing::info!(addr = %server.local_addr(), "alvind listening");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    server.shutdown().await?;
    Ok(())
  })
}
