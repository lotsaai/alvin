# Spec: gateway

Module id `gateway`, crate `alvin-gateway` (library plus the `alvind` binary target). Depends on `session-index`, `omp-host`, `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

The application boundary. Implements every `alvin-protocol` server trait over `alvin-session-index` and `alvin-omp-host`, validates each request once, owns the live sessions and their in-memory event logs, and serves the same methods over the channel transport and a WebSocket listener.

Success:

- Every protocol method works over the channel transport and over `ws://` on `127.0.0.1:0`, round-tripped through `alvin-client`.
- A test pins the declared method list; adding a method without updating it fails.
- Opening a listed session starts one omp on it; opening it again returns the same live session.
- A subscriber that reconnects with its last event id receives every later event once, in order, with no gap.
- Shutting the gateway down closes every omp process.

## Configuration

`GatewayConfig` (typed-builder), built by the faces: omp executable path, omp sessions directory (resolved by the face, see `SPEC-session-index.md`), optional default model (`provider/id`, passed to omp as `--model`). `alvind` adds a listen address and optional TLS certificate and key paths. No environment reads, no defaults chosen here.

## Live Sessions

- Registry: `LiveSessionId` (fresh UUIDv7) to a live entry holding the `alvin-omp-host` session, its session file path, and its event log. Second index: session file path to `LiveSessionId`.
- `live/open` with a listed session: if that session file is already live, return the existing `LiveSessionId` (one omp per session file; two omp processes on one file make omp fork a continuation, `session-entries.ts` `continuationOf`). Otherwise start omp in the session's `cwd` and send `switch_session`.
- `live/open` new: the requested directory must exist; the gateway canonicalizes it and starts omp there with `new_session`.
- `live/branch` and `live/fork` move the omp process onto a new session file (`docs/rpc.md:245-247`). After a non-cancelled reply the gateway reads the new `sessionFile` with `get_state` and moves the session-file index entry to it under the registry lock, so "one omp per session file" still holds. The `LiveSessionId` stays the same.
- `live/close`: closes that session's omp through `alvin-omp-host` (teardown includes omp's children) and removes both index entries. The gateway never closes a session by itself; `session_settled` is reported to the UI, not acted on.
- omp exiting on its own appends a process-exited event, then later commands on that live session fail with the process-exited error code. The entry stays until `live/close`, so the UI can show what happened.
- Gateway shutdown closes every live session.

## Event Log

- A pump task per live session reads the host's event stream, stamps each body with a fresh `EventId` (`Uuid::now_v7`; the uuid crate guarantees ids from one process are ordered by creation) and the `LiveSessionId`, appends it to the log, then publishes it on a `tokio::sync::broadcast` channel.
- Retention: the log holds events since the last `session_settled`. On `session_settled` the log is cleared after that event is published; everything before it is in omp's transcript and is read with `live/messages`.
- `events/subscribe { live, after }`: under the log's lock, take the events with id greater than `after` and register the broadcast receiver, then release the lock, so nothing falls between replay and live. `after` older than the oldest retained event (and not the last cleared settle) fails with the stale-cursor error code; the client reloads with `live/messages` and subscribes without `after`.
- A subscriber that falls behind the broadcast buffer gets its subscription ended with the stale-cursor code and resubscribes with its last id; the log covers the gap if the events are still retained. Slow subscribers never slow omp.

## Validation and Errors

Semantic validation happens here, once: the live session exists, the session file is one the index lists, the directory for a new session exists, the command fits the session's state (no `live/prompt` after the process exited). Lower crates are trusted.

One `Error` in `error.rs`; variants wrap `alvin_session_index::Error` and `alvin_omp_host::Error` as `#[source]` and carry the protocol error code they map to. `From<Error> for ErrorObjectOwned` sets that code and typed `ErrorData`.

## Transports

- Methods: every trait's `into_rpc()` merged into one `RpcModule`; its `Methods` serve both transports.
- Channel: the gateway provides the server half. `Gateway::connect_channel(capacity)` mints a connection, spawns a task that answers each request with `Methods::raw_json_request` and forwards subscription items, and returns a pair of `tokio::sync::mpsc` string channels. `alvin-client` wraps that pair in its channel transport; it does not depend on `alvin-gateway` or on jsonrpsee server crates. Pattern taken from tin `crates/tin-harnessd/src/rpc/memory.rs:38-98`.
- WebSocket: `alvind` accepts TCP connections and serves the `Methods` with `jsonrpsee_server::utils::serve_with_graceful_shutdown`, which takes any `AsyncRead + AsyncWrite`. For `wss://` each stream is wrapped by a `tokio-rustls` acceptor built from the configured certificate and key. Binding to `127.0.0.1:0` reports the bound address for tests.

## Telemetry

OpenTelemetry for logs, traces, and metrics, exported over OTLP. Ported from `~/wagyu/slipstream/bootstrap` (`src/logging.rs`, `src/lib.rs`), with alvin's own defaults. Not a dependency on that crate.

- Built on `init-tracing-opentelemetry` 0.44 `TracingConfig` (read on docs.rs): `with_otel(true)`, `with_logs(true)` (log records exported over OTLP), `with_metrics(true)` with `MetricsExporter::Otlp`, `with_env_fallback("ALVIN_LOG")` then `RUST_LOG`, pretty output in debug builds and JSON in release builds, as `bootstrap` does.
- Service name and version come from the calling face (`alvind` or `alvin-app`), passed in by the face, not from the library's `CARGO_PKG_NAME`.
- Exporter endpoint, protocol, and sampling come from the standard `OTEL_*` environment variables, read by the OpenTelemetry SDK. alvin adds no knobs of its own.
- The filter is built from the environment without rewriting it: no `std::env::set_var`.
- The returned guard flushes and shuts down providers. `init-tracing-opentelemetry`'s `OtelGuard` ignores flush errors on drop, which `AGENTS.md` forbids; the module calls flush and shutdown explicitly at face shutdown and logs any error before dropping the guard.
- Panic hook from `bootstrap`: logs the panic message and location as an `error` event. Its `block_on(tracer_provider())` step only reads the global provider and flushes nothing, so it is dropped; flushing happens through the guard.
- Called once, first thing in each face's `main`, before any runtime threads start.
- The module lives in `alvin-gateway`'s library because both faces depend on it and `AGENTS.md` allows no new crates. Only the faces call it; library code only uses `tracing` macros and the `opentelemetry` metrics API.
- Instrumentation: spans per RPC method (method name, live session id, error code) and per omp command (command name); metrics:
  - `alvin.rpc.duration` histogram, attributes method and outcome (`ok` or error code).
  - `alvin.live.sessions` up-down counter.
  - `alvin.omp.command.duration` histogram, attributes command and outcome.
  - `alvin.events.published` counter; `alvin.subscriptions.stale` counter.
  Attributes come from fixed sets; ids, paths, and message text never become metric attributes. Prompt text and tool output are never logged.

## alvind

Binary target of this crate. `main.rs` is thin: telemetry setup, configuration loaded with `confique` and its clap layer into `GatewayConfig` plus listen settings (same sources and precedence as `alvin-app`, files under `dirs::config_dir()/alvin/alvind.toml`; `SPEC-app.md`, Configuration), `eyre` at the edge, run until a shutdown signal, then close all live sessions and flush telemetry.

## Dependencies

`alvin-protocol` (`server`), `alvin-session-index`, `alvin-omp-host`, `jsonrpsee` (`server`), `tokio`, `tokio-rustls`, `rustls`, `uuid` (v7), `jiff`, `blake3`, `tracing`, `opentelemetry` (metrics API), `init-tracing-opentelemetry` (`otlp`, `tracer`, `logs`, `metrics`), `opentelemetry_sdk`, `tracing-opentelemetry`, `tracing-subscriber`, `thiserror`, `typed-builder`, `alvin-workspace-hack`; for `alvind` only: `clap`, `confique` (`toml`), `dirs`, `eyre`. `alvin-omp-host` and `alvin-session-index` use `tracing` only. All approved. Every dependency at latest stable when added.

## Project Structure

```
crates/alvin-gateway/src/lib.rs          thin
crates/alvin-gateway/src/error.rs
crates/alvin-gateway/src/config.rs       GatewayConfig
crates/alvin-gateway/src/gateway.rs      Gateway: registry, shutdown
crates/alvin-gateway/src/live.rs         live session entry, pump, event log
crates/alvin-gateway/src/rpc.rs          server trait impls, merged module
crates/alvin-gateway/src/rpc/            one file per protocol area
crates/alvin-gateway/src/channel.rs      channel transport server half
crates/alvin-gateway/src/listener.rs     ws and wss listener
crates/alvin-gateway/src/telemetry.rs    logs, traces, metrics setup for both faces
crates/alvin-gateway/src/main.rs         alvind
crates/alvin-gateway/tests/              acceptance tests: the wire contract through alvin-client
```

## Testing Strategy

- Every method round-trips through `alvin-client` over both the channel transport and `ws://127.0.0.1:0`, against real omp (`--mode rpc-ui`, model `zai/glm-5.3-flash`) and real session files in a temporary project.
- A test asserts the merged module's method names equal the pinned list.
- Resume: subscribe, drop the client mid-turn, reconnect with the last event id, assert the full ordered sequence with no duplicates.
- Stale cursor: subscribe with an `after` older than retention, assert the stale-cursor code.
- Idempotent open: two `live/open` calls on one session file return one `LiveSessionId` and one omp process.
- Process death: kill omp, assert the process-exited event and error code.
- Shutdown: after gateway shutdown no omp process remains.
- `wss://`: one round trip over TLS with a certificate generated by the test.

## Boundaries

- Always: follow `AGENTS.md`; stamp every event with a fresh UUIDv7; keep both transports serving the same `Methods`.
- Ask first: new dependencies, new transports, auto-closing sessions.
- Never: write omp session files, add auth, add sequence numbers, re-validate in lower crates, let a slow subscriber block omp.

## Open Questions

None. Dependencies approved: `tokio-rustls`, `rustls`, `clap`, `init-tracing-opentelemetry`, `opentelemetry`, `opentelemetry_sdk`, `tracing-opentelemetry`.
