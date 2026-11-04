# Spec: client

Module id `client`, crate `alvin-client`. Depends on `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

The one typed client every UI uses to reach the gateway. Desktop production connections use `ws://` or `wss://`. The library also retains channel support for transport acceptance tests and explicit non-desktop consumers. Builds for `wasm32-unknown-unknown`.

Success:

- A caller holding a `Client` calls every protocol method the same way regardless of transport.
- Wire errors arrive as typed errors carrying the protocol error code and `ErrorData`; nothing is flattened to a string.
- `cargo build --target wasm32-unknown-unknown` of this crate succeeds, through a mise task.
- The crate depends on neither `alvin-gateway` nor jsonrpsee server crates.

## Endpoint

```rust
pub enum Endpoint {
  /// In-process: the string channel pair handed over by an embedded gateway.
  Channel(ChannelPair),
  /// `ws://` or `wss://`, parsed from a URL.
  WebSocket(Url),
}
```

- `ChannelPair` holds a `tokio::sync::mpsc::Sender<String>` to the server and a `Receiver<String>` from it. An explicit embedding consumer gets the pair from `Gateway::connect_channel`. The desktop does not offer this mode. `alvin-client` knows only the channel types, not who made them.
- `Endpoint::from_str` parses URLs and accepts only `ws` and `wss`. Other schemes (`stdio`, `tcp`, `tcp+tls`, `http(s)`, iroh) are rejected with a typed unsupported-scheme error until a later transport adds them.
- Native only: `Channel`. The web build has no embedded gateway (the gateway spawns processes), so `Channel` is `cfg(not(target_arch = "wasm32"))`.

## Client

- `Client::connect(endpoint) -> Result<Client>`.
  - Channel: wraps the pair in jsonrpsee `TransportSenderT`/`TransportReceiverT` impls and builds with `ClientBuilder::build_with_tokio` (`jsonrpsee_core::client::async_client::ClientBuilder`, read on docs.rs). Pattern from tin `crates/tin-harnessd/src/rpc/memory.rs:30-52`.
  - `ws(s)://` native: jsonrpsee `WsClientBuilder` with feature `client-ws-transport-tls` (rustls with the platform verifier, jsonrpsee 0.26.1 features read on crates.io).
  - `ws(s)://` wasm: `WasmClientBuilder::build(url)` (`jsonrpsee-wasm-client`, read on docs.rs).
- `Client` holds the jsonrpsee client and implements `ClientT` and `SubscriptionClientT` by forwarding to it, so every generated `*ApiClient` trait from `alvin-protocol` applies to `Client` directly. No `Deref` (`AGENTS.md`).
- Errors: one `Error` in `error.rs`. `From<jsonrpsee::core::client::Error>` maps `Call(ErrorObjectOwned)` to `Error::Rpc { code: ErrorCode, message, data: Option<ErrorData> }`, decoding `code` against `alvin_protocol::error_codes` and `data` as `ErrorData`. A code outside that table is `Error::UnknownCode { code, message }`, not a guess. `RestartNeeded` (jsonrpsee's "background task terminated", per its docs) maps to `Error::Disconnected`; `Transport`, `RequestTimeout`, `ParseError` map to their own variants with the source kept.
- Typed calls: UI code calls the generated trait methods and converts with `?` into `alvin_client::Error`.
- Terminal RPCs use the same generated typed client over either transport. The UI retains the emulator and its accepted event cursor across reconnects; it never repeats a shell creation or user input after an ambiguous failure.

## Reconnect

Neither tin nor heyday reconnects automatically, so there is nothing to port; both supply parts. tin: sessions outlive connections and each subscribe registers a fresh forwarder (`crates/tin-harnessd/src/rpc/tests.rs:249-287`); its client says "no reconnect/backoff" (`crates/tin-ui/src/client.rs`). heyday: a connection state machine with `Connected`, `Reconnecting`, `Lost`, driven by a watcher on `Client::on_disconnect` (`crates/heyday-app/src/app/connection.rs:12-84`); reconnect is user-triggered.

jsonrpsee 0.26.1 itself does not reconnect: none of its features add it, and `async_client::Client` reports a dropped connection through `on_disconnect()` and the `RestartNeeded` error ("the background task has been terminated"), both read on docs.rs. The companion crate `reconnecting-jsonrpsee-ws-client` 0.6.0 (jsonrpsee ^0.26, native and web features) does reconnect, with exponential backoff and a per-call `CallRetryPolicy` (`Drop`, `Retry`, `RetryAndResubscribe`). It is not used, because:

- Its `Client` implements neither `ClientT` nor `SubscriptionClientT` (docs.rs trait list); calls take a method `String` and return raw JSON, so the `*ApiClient` traits `alvin-protocol` generates do not work on it and every call loses its types.
- `RetryAndResubscribe` resubscribes with the original params, so `events/subscribe` would replay from the first `after` and repeat events; its docs state it "may lose subscription notifications when it reconnects".
- `Retry` resends calls after a reconnect, which would run `live/prompt` or `live/bash` twice.

What the crate would save is the backoff loop below; the resumable event stream would still be ours. The design below keeps the typed client and puts reconnect beside it.

- State: `Client::connection_state()` and `Client::watch_connection()` (a `tokio::sync::watch` receiver) report `Connected`, `Reconnecting { attempt }` or `Lost`. There is no `Connecting` state: the first connect is `Client::connect` itself, so a client that exists was connected once. The UI renders the state; it does not drive reconnects. `Client` is cheap to clone; clones share one connection.
- Detection: a task awaits `on_disconnect()` on the current jsonrpsee client (`jsonrpsee_core::client::async_client::Client::on_disconnect`, read on docs.rs).
- Reconnect: `ws(s)://` only. The client reconnects to the same URL with exponential backoff from 250 ms doubling to a 30 s limit, plus jitter, forever, until the `Client` is dropped. Constants are named and documented; no settings. Durations are `jiff::SignedDuration`. On success the new jsonrpsee client replaces the old one; later calls go to it.
- Channel: an in-process gateway does not drop a live connection; the channel closing means the gateway shut down, so the state goes `Lost` with no retry.
- Calls: a call in flight when the connection drops fails with `Error::Disconnected`. Calls are never retried by the client: `live/prompt`, `live/bash` and others are not idempotent, and a retry could run them twice. Calls made while `Reconnecting` fail at once with `Error::Disconnected`.
- Events: `Client::events(request)` returns an `EventStream` that survives reconnects. It records the last `EventId` it delivered; after a reconnect it calls `events/subscribe { live, after: last }`, and the gateway replays from there (`SPEC-gateway.md`), so no event is lost or repeated. If the gateway answers with the stale-cursor code, or the live session is unknown after a gateway restart or a `live/close`, the stream's last item is `Err(Error::Resync(reason))` (`ResyncReason::StaleCursor` or `UnknownLiveSession`) and it then ends; the UI reloads with `live/messages` and opens a new stream. The stream ends with `None` when the client is `Lost`.
- The same stream handles a subscription the gateway ended for falling behind: it resubscribes from its last id. A subscription that ends twice in a row while connected without delivering an event yields `Resync(StaleCursor)`, so a gateway that keeps ending subscriptions cannot make the stream spin.

## Dependencies

`alvin-protocol` (no `server`), `jsonrpsee` (`client-core`, `async-client`, `macros`; native: `client-ws-transport-tls`; wasm: `wasm-client`, `async-wasm-client`), `tokio` (`sync`, `macros`; `time` native only), `gloo-timers` (wasm only, `futures` feature, for the backoff delay where tokio's timer does not run; approved), `wasm-bindgen-futures` (wasm only, spawns the reconnect supervisor in the browser, where there is no tokio runtime), `url`, `jiff`, `thiserror`, `tracing`, `alvin-workspace-hack`.

## Project Structure

```
crates/alvin-client/src/lib.rs        thin
crates/alvin-client/src/error.rs
crates/alvin-client/src/endpoint.rs   Endpoint, scheme parsing
crates/alvin-client/src/client.rs     Client, ClientT and SubscriptionClientT forwarding to the current connection
crates/alvin-client/src/reconnect.rs  connection state, disconnect watcher, backoff
crates/alvin-client/src/events.rs     resumable event stream
crates/alvin-client/src/channel.rs    channel transport, native only
crates/alvin-client/src/ws.rs         ws(s) connect, native and wasm
```

## Testing Strategy

- Round trips over both transports live in `alvin-gateway`'s tests, against a real gateway and real omp; this crate has no copy of them.
- Reconnect, also in `alvin-gateway`'s tests over `ws://`: stop the listener mid-turn, restart it on the same address, assert the state goes `Reconnecting` then `Connected`, the event stream continues with no gap and no duplicate; restart with an empty log, assert one `Resync` item.
- Here: endpoint parsing (each supported scheme, each rejected scheme, malformed URLs); error mapping from real `ErrorObjectOwned` values built with the protocol's codes, including an unknown code; the backoff schedule.
- Build check: the wasm32 build runs in `mise check`.

## Boundaries

- Always: follow `AGENTS.md`; keep transports interchangeable.
- Ask first: a new transport or scheme; retrying calls automatically.
- Never: depend on `alvin-gateway`, `alvin-omp-host`, or `alvin-session-index`; add business rules; flatten wire errors to `String`; use `Deref` to expose the inner client.

## Open Questions

None. `url` approved.
