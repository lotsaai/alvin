# AGENTS.md - alvin

alvin is a desktop client for the `omp` coding agent. A gateway owns one `omp --mode rpc` process per live session and reads omp's session files for history; every UI reaches the gateway through one typed client whose transport is chosen by endpoint.

## Crate Roles & Boundaries

### Terminology

- **Face**: an entrypoint binary. `alvin-app` (desktop) and `alvind` (standalone gateway, binary target of `alvin-gateway`). Faces parse arguments and config, wire crates together, and render. They own no protocol or agent semantics.
- **Protocol**: the wire contract (`alvin-protocol`). jsonrpsee `#[rpc]` traits and serde types shared by gateway and client. Builds for `wasm32-unknown-unknown`.
- **Gateway**: the application boundary (`alvin-gateway`). Implements the protocol server traits, validates requests once, and owns live sessions and their in-memory event logs.
- **Agent host**: process supervision (`alvin-omp-host`). The only code that talks to omp.
- **UI**: `alvin-conversation` (pure state) and `alvin-conversation-ui` (gpui-kit views). They see the gateway only through `alvin-client`.

| Crate | Role | Dependencies Allowed |
|---|---|---|
| `alvin-protocol` | **Wire contract**: `#[rpc]` traits (client half by default, server half behind the `server` feature), request/response/event types, UUIDv7 id newtypes, error codes. No IO, no async runtime. Builds for wasm32. | `jsonrpsee` (`client-core`, `macros`; `server-core` only behind `server`), `serde`, `uuid`, `jiff`, `thiserror` |
| `alvin-session-index` | **Module**: read-only scan of omp JSONL session files; groups sessions by repository with git worktrees and jj workspaces as sub-groups. | `alvin-protocol`, `code-workspace`, `serde_json`, async/runtime utilities, `alvin-workspace-hack` |
| `alvin-omp-host` | **Infrastructure**: spawns and supervises one `omp --mode rpc` process per live session through `omp-rpc`, bridges its blocking client into async, turns omp frames into protocol events. Windows Job Object teardown. | `alvin-protocol`, `omp-rpc`, `tokio`, platform crates for process control, `alvin-workspace-hack` |
| `alvin-gateway` | **Application boundary**: implements the protocol server traits over `alvin-session-index` and `alvin-omp-host`; semantic validation; live-session registry and in-memory event logs; mounts the same methods on the channel transport and a WebSocket listener; exports telemetry over OTLP. Binary target `alvind` (face). | `alvin-protocol` (`server`), `alvin-session-index`, `alvin-omp-host`, `jsonrpsee` server, `tokio`, OpenTelemetry and `tracing` crates (`init-tracing-opentelemetry`, `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-appender-tracing`, `tracing-opentelemetry`, `tracing-subscriber`), for the `alvind` target `clap`, `confique`, `dirs`, `eyre`, `alvin-workspace-hack` |
| `alvin-client` | **Typed client**: one client type; the transport (channel, `ws://`, `wss://`) is an implementation detail picked by endpoint. Builds for wasm32. | `alvin-protocol`, `jsonrpsee` client transports |
| `alvin-conversation` | **UI state**: projects, sessions, live turns, queue, history tree, folded from protocol events. No IO. | `alvin-protocol`, `alvin-workspace-hack` |
| `alvin-conversation-ui` | **UI views**: gpui-kit components rendering `alvin-conversation` state and issuing commands through `alvin-client`. | `alvin-conversation`, `alvin-client`, `gpui-kit`, `alvin-workspace-hack` |
| `alvin-app` | **Face**: desktop binary. Owns config and argument parsing; embeds the gateway over the channel transport or connects to a `ws(s)://` URL. | `alvin-conversation-ui`, `alvin-client`, `alvin-gateway`, `gpui-kit`, `eyre`, `alvin-workspace-hack` |
| `alvin-workspace-hack` | **Build-only**: `cargo-hakari` feature unification. | Managed by hakari only. |

### Dependency Rules (STRICT)

1. **Faces own wiring only.** `alvin-app` and the `alvind` target compose crates and parse config; no protocol, session, or agent logic lives in them.
2. **`alvin-protocol` is a leaf.** It depends on no alvin crate, has no IO and no async runtime, and must build for `wasm32-unknown-unknown`. Server-only jsonrpsee features stay behind its `server` feature.
3. **`alvin-gateway` is the only consumer of `alvin-omp-host` and `alvin-session-index`.** No UI crate, client, or face reaches them directly (the app reaches them only through an embedded `alvin-gateway`).
4. **`alvin-omp-host` is the only crate that depends on `omp-rpc` or spawns omp.** One omp process per live session; omp holds one active session per process.
5. **`alvin-session-index` is the only crate that reads omp session files and the only consumer of `code-workspace`.** It opens them read-only and never writes, renames, or deletes them.
6. **`alvin-omp-host` and `alvin-session-index` do not depend on each other** or on `alvin-gateway`.
7. **UI crates see the gateway only through `alvin-client`.** `alvin-conversation` and `alvin-conversation-ui` never depend on `alvin-gateway`, `alvin-omp-host`, or `alvin-session-index`. One exception: `alvin-conversation-ui` takes `alvin-gateway` as a dev-dependency so its in-module UI tests run against an embedded gateway; production code never sees it.
8. **`alvin-client` never depends on `alvin-gateway`.** The in-process channel transport is handed to the client by the embedding side; the client stays wasm-compatible.
9. **No ACP.** The gateway API is our own `#[rpc]` design; do not add ACP crates or shape the API after ACP.
10. **No new crates** beyond this table without approval.
11. **The workspace-hack crate is a build-only exception.** Members depend on it only because hakari manages it; never import from it or add logic to it.

## Non-negotiable Rules

### No Optionality / No Fallbacks

- **Never introduce optionality to "make it pass/work".** Do not add silent defaults, degrade required behavior to `Option`, return early with `Ok(())`, swallow errors, or otherwise mask missing configuration/state.
- **Never weaken tests to go green.** Do not "skip" by returning success early, loosen assertions, add broad retries/timeouts, or ignore errors unless explicitly instructed.
- **Never add gating/ignores without explicit instruction.** No `#[ignore]`, feature flags, env-var gates, or conditional compilation to hide failing tests or behavior changes unless the user explicitly asks for it.

### No Panics in Production Code

- **A panic is a programmer error.** Forbidden in any non-test path: `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `assert!*`, `.unwrap()`, `.expect()`, indexing/slicing that can go out of bounds or split a char boundary (use `get`/`get_mut`), arithmetic that can overflow or divide by zero (use `checked_*`/`saturating_*`).
- **Instead**: return `Result` with a typed error. The dangerous `.expect()` is the one justified by an invariant enforced somewhere else — a comment, not a guarantee. If an invariant is real, encode it in a type. If it cannot be encoded, handle the failure.
- **Narrow exceptions**: test code, benchmarks, build scripts, `examples/`; poisoned-lock recovery via `.unwrap_or_else(PoisonError::into_inner)`; `const` evaluation that fails at compile time. Anything else: ask first.

### Ordered Ids, No Sequence Numbers

- Every entity and event id the gateway mints is a UUIDv7 newtype. Ordering and resume come from the ids: event subscriptions take an optional "after event id". Never add a `seq` field or a counter.
- Live event logs live in gateway memory only. History after a restart comes from omp's session file.

### omp Boundary

- omp session files are read-only to alvin. Never write, rename, or delete them.
- Support only what omp-rpc supports. `!cmd` maps to the `bash` command; `prompt` expands `/` commands, skills, templates, and `^model` on the omp side. Do not invent composer conventions.
- `wire.rs` in `omp-rpc` is generated; never fork or hand-edit `omp-rpc`.
- No access controls: no tokens, no auth, no protective defaults. Users are competent.

### Wire Errors Keep Their Codes

- Gateway errors map to JSON-RPC error objects with a stable code from `alvin-protocol`. The client surfaces the code and message as a typed error; never flatten a wire error to a `String`.

### Terminology

Use standard software engineering vocabulary. Architectural metaphors are banned: never write "load-bearing", "spine", "seam", "pillar", "streak", or similar. Say the technical thing directly: "X is required by Y", "Z depends on W", "the boundary between A and B", "integration point", "failure mode".

Banned domain vocabulary with required replacements: "birth"/"born" → "join"/"membership".

### Mise Environment

> **CRITICAL: Do not ever try to fix `mise` environment related issues. Escalate to the user immediately.**

### Linting & Code Quality

- **Fix, Don't Suppress**: address all lint errors and warnings by fixing the code.
- **No `allow` Attributes**: **NEVER** use `#![allow(...)]` or `#[allow(...)]` to suppress warnings.
- **Strict Mode**: the project compiles with `-D warnings`. If it doesn't compile cleanly, it's broken.
- **Read the Tool's Output, Not the Wrapper's Exit Code**: `mise format` runs a report pass then a `--fix` pass; the aggregate exit reflects the fix pass and can mask report-pass findings `--fix` cannot resolve. A zero exit does **not** mean clean — read each step.

## Data Flow & Validation Strategy

> **CRITICAL: Validate ONCE. Do not repeat validation logic across layers.**

### 1. Faces (`alvin-app`, `alvind`)
- **Responsibility**: parse arguments and config files into typed settings; render.
- **Validation**: syntactic only (does it parse).
- **Normalization**: none.
- **❌ SIN**: checking whether a session exists, whether a path is a repository, or whether a model is valid.

### 2. Wire (`alvin-protocol`, `alvin-client`)
- **Responsibility**: encode and decode.
- **Validation**: serde decoding into typed requests and id newtypes.
- **Normalization**: none.
- **❌ SIN**: business rules in the client or in protocol types.

### 3. Gateway (`alvin-gateway`)
- **Responsibility**: the boundary where wire requests become domain operations.
- **Validation**: semantic, exactly once: the live session exists, the project root resolves, the command is allowed in the session's current state.
- **Normalization**: canonical paths and project grouping keys, once.
- **❌ SIN**: returning `Option` for a failure; re-validating in lower crates.

### 4. Lower crates (`alvin-omp-host`, `alvin-session-index`)
- **Responsibility**: process supervision and file reading.
- **Validation**: none beyond decoding omp's own data (omp frames and JSONL lines are untrusted input and are parsed into typed values at the point of reading).
- **Normalization**: none.
- **❌ SIN**: repairing omp data, defaulting missing fields, or re-checking gateway invariants.

## Configuration

- Config file/env loading, merge precedence, and `clap` mechanics belong to the faces: `alvin-app` and the `alvind` target.
- `alvin-gateway` consumes a typed config (omp executable, model selection, listen address for `alvind`). Lower crates receive only the values they need as constructor inputs.
- New entrypoints become subcommands of an existing face, not new binaries with their own loaders.

## Coding Standards

- **Error Handling**:
  - Each crate that defines a local error boundary must expose exactly one `Error` type and one `Result<T>` alias at the crate root (`lib.rs` or the binary root).
  - Do **not** define module-local `Result` aliases or duplicate error enums in child modules.
  - Child modules must import the crate-root `Error` and `Result`.
  - Errors live in a root `error.rs`.
  - Libraries use `thiserror`; `main`/faces use `eyre`/`color-eyre` at the edge only.
  - `#[from]` for transparent conversions, `#[source]` for contextual variants. Variants carry typed context from the boundary that knows the entity (session id, cwd, session file path). No stringly wrappers (`Other(String)` and kin fail review); never `to_string()` a source. No swallowed errors (`map_err(|_| ..)`, `.ok()`, `let _ =` on a `Result`).
  - `omp_rpc::Error` stays the `#[source]` of `alvin-omp-host` variants; gateway errors carry the protocol error code they map to.
- **Builders**: use `typed-builder` for non-trivial request/configuration types. No long constructor arg lists.
- **Dependencies**: always the latest stable release when added or updated; never pin to an old version to match another repository or dodge a breaking change. A new dependency needs approval.
- **Configuration**: faces load config with `confique` (schema with `#[config(layer_attr(derive(clap::Args)))]`, CLI layer preloaded, then env, then files), as tin and crumbs do.
- **Simplicity**: boring over clever; deletion over addition. No fake abstractions, no hexagonal/ports-and-adapters layering, no `Deref` to expose a wrapped client.
- **Types**: UUIDv7 id newtypes; `jiff` for instants and durations; `Path`/`PathBuf` for paths, never `String`.
- **Modules**: `foo.rs` beside `foo/`, never `mod.rs`; thin `main.rs` and `lib.rs`; one module per feature; `use` type names at the module top.
- **Naming**: `UpperCamelCase` types and variants, `snake_case` fns/modules/variables, `SCREAMING_SNAKE_CASE` consts. Crates are `alvin-<role>`. JSON-RPC methods are `<area>/<camelCaseVerb>` (for example `sessions/list`, `live/prompt`).
- **Testing**:
  - Real services, no mocks, fakes, or stubs. Tests that need omp start real `omp --mode rpc-ui` with model `zai/glm-5.3-flash`, using the machine's existing omp configuration. Never look for, read, or print secrets.
  - `alvin-session-index` tests run over real session files that omp writes in a temporary project, plus real git worktree and jj workspace layouts.
  - Every gateway method round-trips through `alvin-client` over both the channel transport and a WebSocket on `127.0.0.1:0`. A test pins the declared method list.
  - UI tests use `#[gpui_kit::test]` headless with real views against an embedded gateway; `#[tokio::test]` stacked above `#[gpui_kit::test]` supplies the tokio context. Assert structure and state, not pixels. Never `use gpui_kit::*` or `use super::*` in a module containing `#[gpui_kit::test]`.
  - **Test kinds and where they live.** Unit tests (pure logic) and integration tests (a module working with real services: omp, files, git, jj, an embedded gateway) both live in `#[cfg(test)] mod tests` inside the module they test, so they reach private items without widening visibility. Acceptance tests are black-box tests of a crate's public contract and are the only tests in a crate's `tests/`; today that is the gateway's wire contract exercised through `alvin-client`. Never make an item `pub` for a test. Test fixtures live in `crates/<crate>/fixtures/`. Tests clean up what they create.

## Development Workflow (Mise)

**ALWAYS** use `mise` tasks for development. Only run direct toolchain commands if no `mise` wrapper exists.

**NEVER** run "targeted" tests, the cost is not the test it's the compilation of the modules.
**NEVER** run `cargo test`, it is not a win, it has the opposite outcome.

| Task | Description |
|---|---|
| `mise format` | Checks for this codebase (check, clippy `-D warnings`, rustfmt, cargo-sort, toml/whitespace guards). |
| `mise test` | All tests (Rust `nextest`). |
| `mise run --force test` | Force a fresh full test run (bypass cache). |
| `mise hakari` | Regenerate the workspace-hack crate after dependency changes. |

**IMPORTANT**: After changes, **ALWAYS** run:
1. `mise format`
2. If Rust code was modified: `mise run --force test`

Use jj for every repository mutation; never git mutations.

## Agent Guidelines

- **Do not** put protocol, session, or agent logic in `alvin-app` or the `alvind` target.
- **Do not** give `alvin-protocol` IO, an async runtime, or a dependency that breaks the wasm32 build.
- **Do not** call `alvin-omp-host` or `alvin-session-index` from anything but `alvin-gateway`.
- **Do not** depend on `omp-rpc` outside `alvin-omp-host`, or on `code-workspace` outside `alvin-session-index`.
- **Do not** let UI crates depend on gateway internals; go through `alvin-client`.
- **Do not** write to omp session files, add auth, or add sequence numbers.
- **Do** keep transports interchangeable: a method that works over the channel transport must work over `ws(s)://` unchanged.
- **Do** emit every live-session event with a fresh UUIDv7 id from the gateway.
- **When adding a gateway method**: add it to the `#[rpc]` trait in `alvin-protocol`, implement it in `alvin-gateway`, add it to the pinned method list, and round-trip it over both transports.
- **When adding a transport**: implement it in `alvin-client` (and the matching listener in `alvin-gateway`) behind endpoint selection; ask first.

## Before Committing Checklist

- [ ] `mise format` passes (each step read, not just the exit code)
- [ ] `mise run --force test` passes (if Rust changed)
- [ ] **No `allow` attributes**: all lint warnings fixed, not suppressed
- [ ] No `.unwrap()` or `.expect()` in production paths
- [ ] **Error boundary**: one `Error` + `Result` at root per boundary crate, children import from root, `eyre` only at the edge
- [ ] **Dependency Check**: the table above holds; `alvin-protocol` and `alvin-client` still build for wasm32
- [ ] **omp Boundary**: no writes to omp session files; `omp-rpc` untouched
- [ ] **Ids**: new events and entities use UUIDv7 newtypes; no `seq` fields
- [ ] **Transports**: new gateway methods are in the pinned list and round-trip over channel and WebSocket
- [ ] All public items have doc comments; no debug `println!` or `dbg!`
- [ ] No hardcoded credentials; no spec ids in code comments or error strings
- [ ] Banned vocabulary absent (`seam`, `spine`, `streak`, `pillar`, `load-bearing`, `birth`, `born`)
