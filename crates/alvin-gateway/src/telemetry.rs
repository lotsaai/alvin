//! Gateway metric instruments and their bounded attributes.

use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};

/// The gateway's metric instruments. Created from the global meter, so they export once telemetry is running and
/// record nothing before. Attributes come from fixed sets: ids, paths and message text never become attributes.
#[derive(Debug, Clone)]
pub(crate) struct Metrics {
  rpc_duration: Histogram<f64>,
  omp_command_duration: Histogram<f64>,
  live_sessions: UpDownCounter<i64>,
  events_published: Counter<u64>,
  subscriptions_stale: Counter<u64>,
}

impl Metrics {
  pub(crate) fn new() -> Self {
    let meter = opentelemetry::global::meter("alvin-gateway");
    Self {
      rpc_duration: meter
        .f64_histogram("alvin.rpc.duration")
        .with_unit("s")
        .with_description("Time to answer one protocol request")
        .build(),
      omp_command_duration: meter
        .f64_histogram("alvin.omp.command.duration")
        .with_unit("s")
        .with_description("Time omp took to answer one command")
        .build(),
      live_sessions: meter
        .i64_up_down_counter("alvin.live.sessions")
        .with_description("Live sessions, each one omp process")
        .build(),
      events_published: meter
        .u64_counter("alvin.events.published")
        .with_description("Events published to live-session logs")
        .build(),
      subscriptions_stale: meter
        .u64_counter("alvin.subscriptions.stale")
        .with_description("Subscriptions refused or ended with the stale-cursor code")
        .build(),
    }
  }

  /// Records one answered request. `outcome` is `ok` or the JSON-RPC error code, never free text.
  pub(crate) fn record_rpc(&self, method: &'static str, outcome: &str, elapsed: Duration) {
    self.rpc_duration.record(
      elapsed.as_secs_f64(),
      &[KeyValue::new("method", method), KeyValue::new("outcome", outcome.to_owned())],
    );
  }

  /// Records one command sent to omp. `outcome` is `ok` or `error`.
  pub(crate) fn record_omp_command(&self, command: &'static str, outcome: &'static str, elapsed: Duration) {
    self.omp_command_duration.record(
      elapsed.as_secs_f64(),
      &[KeyValue::new("command", command), KeyValue::new("outcome", outcome)],
    );
  }

  /// A live session started (`1`) or ended (`-1`).
  pub(crate) fn live_sessions(&self, change: i64) {
    self.live_sessions.add(change, &[]);
  }

  /// One event was appended to a live-session log.
  pub(crate) fn event_published(&self) {
    self.events_published.add(1, &[]);
  }

  /// A subscription was refused or ended with the stale-cursor code.
  pub(crate) fn subscription_stale(&self) {
    self.subscriptions_stale.add(1, &[]);
  }
}
