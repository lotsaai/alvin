//! OpenTelemetry traces, metrics and logs exported over OTLP.
//!
//! The three SDK providers are built here, not by `init-tracing-opentelemetry`'s `Guard`, because its drop discards
//! every flush error. [`Telemetry::shutdown`] flushes each provider and logs what fails.

use std::io;
use std::time::Duration;

use init_tracing_opentelemetry::Error;
use init_tracing_opentelemetry::otlp::{logs, metrics, traces};
use init_tracing_opentelemetry::resource::DetectResource;
use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::Result;

/// Which binary is exporting: the face passes its own name and version.
#[derive(Debug, Clone, Copy)]
pub struct Service {
  /// `service.name`, unless `OTEL_SERVICE_NAME` overrides it.
  pub name: &'static str,
  /// `service.version`.
  pub version: &'static str,
}

/// The running telemetry pipeline. Call [`Telemetry::shutdown`] before the process exits.
#[must_use = "telemetry is only flushed by calling shutdown"]
#[derive(Debug)]
pub struct Telemetry {
  tracer: SdkTracerProvider,
  meter: SdkMeterProvider,
  logger: SdkLoggerProvider,
}

impl Telemetry {
  /// Builds the exporters and installs the global `tracing` subscriber. The exporter endpoint, protocol and sampling come
  /// from the standard `OTEL_*` variables; the log filter from `ALVIN_LOG`, then `RUST_LOG`, else `info`. Call it outside
  /// an async runtime: the OTLP HTTP exporter builds a blocking client.
  pub fn init(service: &Service) -> Result<Self> {
    let resource = DetectResource::default()
      .with_fallback_service_name(service.name)
      .with_fallback_service_version(service.version)
      .build();
    let tracer = traces::init_tracerprovider(resource.clone(), traces::identity).map_err(Error::from)?;
    let meter = metrics::init_meterprovider(resource.clone(), metrics::identity).map_err(Error::from)?;
    let logger = logs::init_loggerprovider(resource, logs::identity).map_err(Error::from)?;
    init_tracing_opentelemetry::init_propagator()?;
    opentelemetry::global::set_tracer_provider(tracer.clone());
    opentelemetry::global::set_meter_provider(meter.clone());

    let spec = std::env::var("ALVIN_LOG")
      .or_else(|_| std::env::var("RUST_LOG"))
      .unwrap_or_else(|_| "info".to_owned());
    let filter = EnvFilter::try_new(spec)?;
    let text = if cfg!(debug_assertions) {
      tracing_subscriber::fmt::layer().pretty().with_writer(io::stderr).boxed()
    } else {
      tracing_subscriber::fmt::layer().json().with_writer(io::stderr).boxed()
    };
    let subscriber = tracing_subscriber::registry()
      .with(filter)
      .with(text)
      .with(
        tracing_opentelemetry::layer()
          .with_error_records_to_exceptions(true)
          .with_tracer(tracer.tracer(service.name)),
      )
      .with(OpenTelemetryTracingBridge::new(&logger));
    tracing::subscriber::set_global_default(subscriber).map_err(Error::from)?;

    Ok(Self { tracer, meter, logger })
  }

  /// Flushes and shuts down traces, metrics, then logs. A failure is logged and does not stop the remaining providers.
  pub fn shutdown(self) {
    report("traces", "flush", self.tracer.force_flush());
    report("traces", "shutdown", self.tracer.shutdown());
    report("metrics", "flush", self.meter.force_flush());
    report("metrics", "shutdown", self.meter.shutdown());
    // Logs go last so the failures above still reach the log exporter.
    report("logs", "flush", self.logger.force_flush());
    report("logs", "shutdown", self.logger.shutdown());
  }
}

fn report(signal: &'static str, step: &'static str, result: OTelSdkResult) {
  if let Err(error) = result {
    tracing::error!(signal, step, %error, "telemetry {step} failed");
  }
}

/// The gateway's metric instruments. Created from the global meter, so they export once a [`Telemetry`] is running and
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
