//! `alvind`: the standalone gateway. Parses settings, wires telemetry and the WebSocket listener, and runs until the OS
//! asks it to stop.

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
    let listener = tokio::net::TcpListener::bind(settings.listen).await?;
    let server = gateway.serve_ws(listener).await?;
    tracing::info!(addr = %server.local_addr(), "alvind listening");
    shutdown_signal().await?;
    tracing::info!("shutting down");
    server.shutdown().await?;
    gateway.shutdown().await?;
    Ok(())
  })
}

/// Resolves when the OS asks the process to stop: SIGINT or SIGTERM (what `systemd` sends) on Unix.
#[cfg(unix)]
async fn shutdown_signal() -> std::io::Result<()> {
  use tokio::signal::unix::{SignalKind, signal};

  let mut interrupt = signal(SignalKind::interrupt())?;
  let mut terminate = signal(SignalKind::terminate())?;
  tokio::select! {
    _ = interrupt.recv() => {}
    _ = terminate.recv() => {}
  }
  Ok(())
}

/// Resolves when the OS asks the process to stop: Ctrl-C, Ctrl-Break, console close or system shutdown on Windows.
#[cfg(windows)]
async fn shutdown_signal() -> std::io::Result<()> {
  use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close, ctrl_shutdown};

  let mut interrupt = ctrl_c()?;
  let mut interrupt_break = ctrl_break()?;
  let mut close = ctrl_close()?;
  let mut shutdown = ctrl_shutdown()?;
  tokio::select! {
    _ = interrupt.recv() => {}
    _ = interrupt_break.recv() => {}
    _ = close.recv() => {}
    _ = shutdown.recv() => {}
  }
  Ok(())
}
