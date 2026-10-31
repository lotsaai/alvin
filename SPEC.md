# Spec: alvin

Status: capability map and open questions resolved (2026-10-30). Module specs (`SPEC-<module-id>.md`) follow in build order.

## Objective

alvin is a desktop client for the `omp` coding agent. A gateway owns the agent processes. Every UI reaches the gateway through one typed client.

Users are developers. Others install it later. Assume competent users: no protective defaults, no tokens, no auth.

Success means:

- Projects list grouped by repository, with git worktrees and jj workspaces as sub-groups.
- Session history per project, read from omp's session files.
- Several sessions live at once, each driven by its own `omp --mode rpc-ui` process.
- Chat features at least on par with Gupi, limited to what omp-rpc supports.

## Capability Map

| Module id | Crate | Responsibility | Depends on |
|---|---|---|---|
| `scaffold` | workspace, `alvin-workspace-hack` | Cargo workspace, `mise` tasks pruned of heyday leftovers, `hk`, hakari | none |
| `protocol` | `alvin-protocol` | jsonrpsee `#[rpc]` traits and wire types: projects, session history, live sessions, commands, events, error codes. UUIDv7 id newtypes. Client half by default, `server` feature for the server half. Builds for wasm32 | `scaffold` |
| `session-index` | `alvin-session-index` | Read-only scan of omp JSONL session files. Groups by repository and worktree/workspace through `code-workspace` | `protocol` |
| `omp-host` | `alvin-omp-host` | One supervised `omp-rpc` client per live session, bridged into async. Translates omp frames into protocol events. Windows Job Object for teardown | `protocol` |
| `gateway` | `alvin-gateway` | Implements the protocol server traits over `session-index` and `omp-host`. Mounts the same `Methods` on the channel transport and on a WebSocket listener. Ships the `alvind` binary serving `ws(s)://` | `session-index`, `omp-host` |
| `client` | `alvin-client` | The one typed client. Transport is an implementation detail chosen by endpoint | `protocol` |
| `conversation` | `alvin-conversation` | Pure UI state: projects, sessions, live turns, queue, history tree | `protocol` |
| `conversation-ui` | `alvin-conversation-ui` | gpui-kit views | `conversation`, `client` |
| `app` | `alvin-app` | Desktop face. Parses config and arguments, embeds the gateway and connects over the channel transport, or connects to a ws(s) URL | `conversation-ui`, `client`, `gateway` |

Build order: `scaffold → protocol → session-index, omp-host → gateway, client, conversation → conversation-ui → app`.

## Key Decisions

- **Agent protocol:** omp-rpc over `omp --mode rpc-ui` (plain `rpc` emits no dialog requests), consumed as a git dependency at the omp release tag. No ACP.
- **Process model:** one omp process per live session. omp holds one active session per process.
- **Session history:** read omp's JSONL session files. alvin never writes, renames, or deletes them.
- **Project grouping:** `code-workspace` (crates.io, latest stable; jj and worktree linking arrived in 0.2.0) finds roots, including jj, and links git worktrees and jj workspaces to their repository.
- **Gateway API:** our own jsonrpsee `#[rpc]` traits, in the tin and heyday style. Not shaped like ACP.
- **Transports:** one typed client. v1 implements the channel transport (in-memory, jsonrpsee `TransportSenderT`/`TransportReceiverT` over mpsc, as in tin `rpc/memory.rs`) and `ws://`/`wss://`. Later: `stdio://`, `tcp://`, `tcp+tls://`, `http(s)://`, iroh.
- **Ids and resume:** ids the gateway mints (live sessions, events) are UUIDv7. Session ids come from omp (UUIDv7 in current omp). Project and checkout ids are blake3 hashes of the canonical root path. Event subscriptions take an optional "after event id" and replay from there. No sequence counters.
- **Event log:** each live session's event log lives in gateway memory only. Opening a session starts omp on it; its transcript comes from that omp (`get_messages_page`), during a run and after a restart alike. alvin never parses session content itself.
- **Binaries:** `alvin-app` (desktop) and `alvind` (standalone gateway serving `ws(s)://`).
- **Errors:** one `Error` and `Result` per crate in `error.rs`. Gateway errors map to JSON-RPC errors with a stable code from `alvin-protocol`; the client surfaces code and message as a typed error and never flattens them to strings.
- **Composer:** support only what omp-rpc supports. `!cmd` maps to the `bash` command. `prompt` expands `/` commands, skills, templates, and `^model` server-side. No invented conventions.
- **Method names:** `<area>/<camelCaseVerb>`, for example `sessions/list`, `live/prompt`. New gateway methods go in the `#[rpc]` trait, the gateway, the pinned method list, and a round-trip test over both transports.
- **Validation:** the gateway validates semantically, once. Faces parse only. Lower crates parse omp data into typed values and do not repair it.
- **Configuration:** faces (`alvin-app`, `alvind`) load config and parse arguments into typed settings. The gateway takes a typed config (omp executable, model selection, listen address). New entrypoints are subcommands of an existing face.
- **Platforms:** macOS, Windows, Linux desktop. `alvin-protocol` and `alvin-client` build for `wasm32-unknown-unknown`.

## Tech Stack

- Rust, edition 2024, latest stable toolchain from `mise.toml`.
- UI: `gpui-kit` 0.8.1 with `gpui-pre` 0.3.10, tracking latest.
- RPC: `jsonrpsee` (client-core and macros in `alvin-protocol`; server and ws client in their own crates).
- Agent: `omp-rpc` from the oh-my-pi git repository.
- Project roots: `code-workspace`.
- Versions: every dependency at its latest stable release (`AGENTS.md`, Dependencies).
- Ids: `uuid` with v7. Time: `jiff`.
- Async: `tokio` on native. Builders: `typed-builder`. Errors: `thiserror` in libraries, `eyre` only in faces.

## Commands

Use mise tasks only. Never `cargo test`, never targeted test runs.

```
Format:    mise format
Test:      mise test
Fresh run: mise run --force test
Hakari:    mise hakari
```

After changes run `mise format`, then `mise run --force test` if Rust changed. Read each format step's output, not the exit code. The `scaffold` module adds build and dev tasks if needed.

`.mise/tasks` pruning (heyday leftovers): `nsh/`, `generate/sql/heydayd`, `test/server.sh` and the NSH platform test tasks, `dev/api`, `dev/web`, `dev/ios`, `dev/android`, `dist/*`. Rename heyday binary outputs to alvin's. Remove the NSH excludes from `hk.pkl`.

## Project Structure

```
Cargo.toml                    → workspace
crates/alvin-protocol/        → wire types and rpc traits
crates/alvin-session-index/   → omp session file reader
crates/alvin-omp-host/        → omp process supervision
crates/alvin-gateway/         → server implementation and transports
crates/alvin-client/          → typed client
crates/alvin-conversation/    → UI state
crates/alvin-conversation-ui/ → gpui-kit views
crates/alvin-app/             → desktop binary
crates/alvin-workspace-hack/  → hakari, build-only
.mise/tasks/                  → task scripts
SPEC.md, SPEC-<module-id>.md  → specs
AGENTS.md                     → coding rules and crate boundaries
.agents/commands/rust-review.md → review command
```

Unit and integration tests live in `#[cfg(test)] mod tests` inside the module they test. A crate's `tests/` holds only acceptance tests of its public contract (today: the gateway's wire contract through `alvin-client`). See `AGENTS.md`, Testing.

## Code Style

`AGENTS.md` is the code-style and crate-boundary law; `.agents/commands/rust-review.md` reviews against it, then the `write-rust` skill. Summary: rustfmt 2-space indent, 140 columns, edition 2024. One `error.rs` per crate with one `Error` (thiserror) and a `Result` alias at the root; variants carry typed context. No `mod.rs`, no `#[allow]`, no `unwrap`/`expect`/panics outside tests.

```rust
/// Identifies one live agent session inside the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveSessionId(Uuid);

impl LiveSessionId {
  /// Mints a fresh, time-ordered id.
  pub fn mint() -> Self {
    Self(Uuid::now_v7())
  }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
  #[error("spawning omp in {}", cwd.display())]
  Spawn { cwd: PathBuf, #[source] source: omp_rpc::Error },
  #[error("unknown live session {id:?}")]
  UnknownSession { id: LiveSessionId },
}
```

## Testing Strategy

- Runner: `cargo nextest` via `mise run test`, whole workspace.
- Real services, no mocks or fakes. Integration tests start real `omp --mode rpc-ui` with model `openrouter/inclusionai/ling-3.0-flash`, using the machine's existing omp configuration.
- `session-index`: tests over real JSONL session files written by omp in a temporary project, plus git worktree and jj workspace layouts.
- `gateway` and `client`: every API method round-tripped over both the channel transport and a WebSocket on `127.0.0.1:0`. A test pins the declared method list.
- `conversation`: plain `#[test]` on the state model.
- `conversation-ui` and `app`: headless `#[gpui_kit::test]` with real views against an embedded gateway. Assert structure and state, not pixels. Never `use gpui_kit::*` in test modules.

## Boundaries

- **Always:** follow `AGENTS.md`. codebase-memory, ast-grep, ast-edit before shell tools. Run `mise format` and `mise run --force test` before a commit. jj for every repository mutation.
- **Ask first:** new dependencies, new crates, binaries or services, changes to `omp-rpc` or `code-workspace`, adding a transport.
- **Never:** write to omp session files, hand-edit `omp-rpc`'s `wire.rs`, read or search for secrets, add auth or protective defaults, mock services in tests, add sequence numbers.

## Open Questions

None.
