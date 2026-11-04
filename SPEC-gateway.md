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

`GatewayConfig` (typed-builder), built by the `alvind` face: omp launch settings (`alvin_omp_host::HostConfig`), the omp sessions directory, and an optional default model. `alvind` owns `--listen`, `--omp`, `--model`, and `--sessions-dir`. The desktop owns none of these server settings.

## Live Sessions

- Registry: `LiveSessionId` (fresh UUIDv7) to a live entry holding the `alvin-omp-host` session, its session file path, and its event log. Second index: session file path to `LiveSessionId`.
- `live/open` with a listed session: if that session file is already live, return the existing `LiveSessionId` (one omp per session file; two omp processes on one file make omp fork a continuation, `session-entries.ts` `continuationOf`). Otherwise start omp in the session's recorded `cwd` and send `switch_session`. The `cwd` is passed exactly as the session header records it: omp refuses the switch when its own spelling of the working directory differs from the recorded one (macOS `/var` against `/private/var`). Opens are serialized, so concurrent opens of one file start one process. Registry keys are session file paths with their directory canonicalized.
- `live/open` new: the checkout's directory must exist; the gateway starts omp there on a new session and reads the session file path with `get_state`.
- `live/branch` and `live/fork` move the omp process onto a new session file (`docs/rpc.md:245-247`). After a non-cancelled reply the gateway reads the new `sessionFile` with `get_state` and moves the session-file index entry to it under the registry lock, so "one omp per session file" still holds. The `LiveSessionId` stays the same.
- `live/close`: closes that session's omp through `alvin-omp-host` (teardown includes omp's children) and removes both index entries. The gateway never closes a session by itself; `session_settled` is reported to the UI, not acted on.
- omp exiting on its own appends a process-exited event, then later commands on that live session fail with the process-exited error code. The entry stays until `live/close`, so the UI can show what happened.
- Gateway shutdown closes every live session.

## Interactive terminals

The gateway resolves the requested checkout on its own host and starts an independent shell in a PTY. A terminal has a UUIDv7 identity, an ordered output log and serialized input and resize operations. Output is retained before it is read by subscribers; accepted-event acknowledgements release retained bytes and stop the PTY reader when the bounded log is full. Output precedes the exit event. Explicit close and gateway shutdown terminate the process and await cleanup. Windows teardown is deferred and unverified: the shell is assigned to a Job Object after it starts, so descendants created before assignment may escape the job.

## Event Log

- A pump task per live session reads the host's event stream, stamps each body with a fresh `EventId` (`Uuid::now_v7`; the uuid crate guarantees ids from one process are ordered by creation) and the `LiveSessionId`, appends it to the log, then publishes it on a `tokio::sync::broadcast` channel.
- Retention: the log holds events since the last settle. omp signals it two ways, both of which clear the log after that event is published: a `session_settled` event, and a `prompt_result` with `sessionSettled: true`. omp may send both for one turn. Everything before the settle is in omp's transcript and is read with `live/messages`.
- `events/subscribe { live, after }`: under the log's lock, take the events with id greater than `after` and register the broadcast receiver, then release the lock, so nothing falls between replay and live. `after` older than the last clearing event fails with the stale-cursor error code; the client reloads with `live/messages` and subscribes without `after`. An unknown live session fails with the unknown-live-session code.
- A subscriber that falls behind the broadcast buffer has its subscription ended (JSON-RPC cannot attach an error code to a subscription that is already accepted) and resubscribes with its last id; the log covers the gap if the events are still retained. Slow subscribers never slow omp.

## Validation and Errors

Semantic validation happens here, once: the live session exists, the session file is one the index lists, the directory for a new session exists, the command fits the session's state (no `live/prompt` after the process exited). Lower crates are trusted.

One `Error` in `error.rs`; variants wrap `alvin_session_index::Error` and `alvin_omp_host::Error` as `#[source]` and carry the protocol error code they map to. `From<Error> for ErrorObjectOwned` sets that code and typed `ErrorData`.

## Transports

- Methods: every trait's `into_rpc()` merged into one `RpcModule`; its `Methods` serve both transports.
- Channel: the gateway provides the server half. `Gateway::connect_channel(capacity)` mints a connection, spawns a task that answers each request with `Methods::raw_json_request` and forwards subscription items, and returns a pair of `tokio::sync::mpsc` string channels. `alvin-client` wraps that pair in its channel transport; it does not depend on `alvin-gateway` or on jsonrpsee server crates. Pattern taken from tin `crates/tin-harnessd/src/rpc/memory.rs:38-98`.
- WebSocket: `alvind` accepts TCP connections and serves the `Methods` with `jsonrpsee_server::utils::serve_with_graceful_shutdown`, which takes any `AsyncRead + AsyncWrite`. There is no TLS in the gateway: `wss://` is a reverse proxy's job. Binding to `127.0.0.1:0` reports the bound address for tests.

## Telemetry

OpenTelemetry for logs, traces, and metrics, exported over OTLP. Ported from `~/wagyu/slipstream/bootstrap` (`src/logging.rs`, `src/lib.rs`), with alvin's own defaults. Not a dependency on that crate.

- `alvin-telemetry` builds the trace, metric, and log providers with `init-tracing-opentelemetry` and installs the global tracing subscriber. Debug output is pretty text; release output is JSON. The filter reads `ALVIN_LOG`, then `RUST_LOG`, then `info`.
- Service name and version come from the calling face (`alvind` or `alvin-app`), passed in by the face, not from the library's `CARGO_PKG_NAME`.
- Exporter endpoint, protocol, and sampling come from the standard `OTEL_*` environment variables, read by the OpenTelemetry SDK. alvin adds no knobs of its own.
- The filter is built from the environment without rewriting it: no `std::env::set_var`.
- The returned guard flushes and shuts down providers. `init-tracing-opentelemetry`'s `OtelGuard` ignores flush errors on drop, which `AGENTS.md` forbids; the module calls flush and shutdown explicitly at face shutdown and logs any error before dropping the guard.
- Called once, first thing in each face's `main`, before any runtime threads start.
- Shared provider setup lives in `alvin-telemetry`, called only by the faces. Gateway-specific metric instruments stay private in `alvin-gateway/src/telemetry.rs`. The desktop has no production dependency on the gateway.
- Instrumentation: spans per RPC method (method name, live session id, error code) and per omp command (command name); metrics:
  - `alvin.rpc.duration` histogram, attributes method and outcome (`ok` or error code).
  - `alvin.live.sessions` up-down counter.
  - `alvin.omp.command.duration` histogram, attributes command and outcome.
  - `alvin.events.published` counter; `alvin.subscriptions.stale` counter.
  Attributes come from fixed sets; ids, paths, and message text never become metric attributes. Prompt text and tool output are never logged.

## alvind

Binary target of this crate. `main.rs` loads `alvind.toml` and CLI settings, initializes shared telemetry before runtime threads, then serves the gateway. The listener defaults to `127.0.0.1:7410`. When absent from server configuration, the sessions directory comes from `alvin_gateway::sessions_dir`, following omp's rules. On an OS shutdown signal, `alvind` closes live sessions and flushes telemetry. Desktop connection loss does not shut down the gateway.

## Dependencies

`alvin-protocol` (`server`), `alvin-session-index`, `alvin-omp-host`, `jsonrpsee` (`server`), `tokio`, `jiff`, `tracing`, `opentelemetry` (metrics API), `thiserror`, `typed-builder`, the existing terminal platform dependencies, and `alvin-workspace-hack`. The `alvind` face also uses `alvin-telemetry`, `clap`, `confique`, `dirs`, and `eyre`. Provider/exporter dependencies belong to `alvin-telemetry`.

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
crates/alvin-gateway/src/listener.rs     ws listener
crates/alvin-gateway/src/telemetry.rs    gateway metric instruments
crates/alvin-gateway/src/main.rs         alvind
crates/alvin-gateway/tests/              acceptance tests: the wire contract through alvin-client
```

## Testing Strategy

- Every method round-trips through `alvin-client` over both the channel transport and `ws://127.0.0.1:0`, against real omp (`--mode rpc-ui`, model `openrouter/inclusionai/ling-3.0-flash`) and real session files in a temporary project.
- A test asserts the merged module's method names equal the pinned list.
- Resume: subscribe, drop the client mid-turn, reconnect with the last event id, assert the full ordered sequence with no duplicates.
- Stale cursor: subscribe with an `after` older than retention, assert the stale-cursor code.
- Idempotent open: two `live/open` calls on one session file return one `LiveSessionId` and one omp process.
- Process death: kill omp, assert the process-exited event and error code.
- Shutdown: after gateway shutdown no omp process remains.

## Boundaries

- Always: follow `AGENTS.md`; stamp every event with a fresh UUIDv7; keep both transports serving the same `Methods`.
- Ask first: new dependencies, new transports, auto-closing sessions.
- Never: write omp session files, add auth, add sequence numbers, re-validate in lower crates, let a slow subscriber block omp.

## Open Questions

None. Dependencies approved: `clap`, `init-tracing-opentelemetry`, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-appender-tracing`, `tracing-opentelemetry`. `wss://` on the server side is out of scope (decided with Task 6).
