# Spec: protocol

Module id `protocol`, crate `alvin-protocol`. Depends on `scaffold`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

The wire contract between gateway and every UI. jsonrpsee `#[rpc]` traits plus serde types. Leaf crate: no alvin dependency, no IO, no async runtime, builds for `wasm32-unknown-unknown`.

Success:

- Client half builds by default; server half only behind feature `server`.
- `cargo build --target wasm32-unknown-unknown -p alvin-protocol` succeeds without `server`, run through a mise task.
- Gateway-minted ids are UUIDv7 newtypes. No `seq` field exists anywhere.
- Every gateway error has a stable code defined here.
- Types cover each feature in the capability list below, and nothing omp-rpc cannot do.

## Dependencies

`jsonrpsee` (`client-core`, `macros`; `server-core` behind `server`), `serde`, `serde_json`, `uuid` (v7, serde), `jiff` (serde), `thiserror`, `typed-builder`, `validator`, `schemars`, `alvin-workspace-hack`. Nothing else.

## Ids

Newtypes, `serde(transparent)`, `Ord`:

- Minted by the gateway, fresh UUIDv7: `LiveSessionId` (one omp process), `EventId`.
- Minted by omp: `SessionId`, parsed from the session header. Current omp mints it with `Bun.randomUUIDv7()` (`session-manager.ts:144-145`); the header field is a plain string (`session-entries.ts`, `SessionHeader.id`), so a header whose id does not parse as a UUID is a per-file error.
- Derived by the gateway: `ProjectId`, `CheckoutId`, the blake3 hash of the canonical root path, serialized as hex. They carry no time and are not UUIDs.
- No queue entry id: omp addresses queued messages by text and queue.

Ordering and resume use `EventId` only: `after: Option<EventId>`.

## API Surface

Method names `<area>/<camelCaseVerb>`. One trait per area, each `#[rpc(client)]`, plus `server` behind the feature.

| Trait | Methods | omp-rpc source |
|---|---|---|
| `ProjectsApi` | `projects/list` (repository, then checkouts: git worktrees, jj workspaces). A `cwd` whose repository can no longer be resolved belongs to no project and its sessions are not listed | session files via `code-workspace` |
| `SessionsApi` | `sessions/list` (per project/checkout, paged, newest first), from session file headers | `session-index` |
| `LiveApi` | `live/open` (new session in a checkout, or open a listed session file; optional model), `live/close`, `live/list`, `live/state`, `live/messages` (paged, opaque cursor), `live/tree`, `live/entries` (since an entry id), `live/branchMessages`, `live/prompt`, `live/steer`, `live/followUp`, `live/abort`, `live/abortAndPrompt`, `live/bash`, `live/abortBash`, `live/compact`, `live/fork`, `live/branch`, `live/navigateTree`, `live/abortBranchSummary`, `live/label`, `live/rename`, `live/stats`, `live/commands`, `live/exportHtml` | `new_session`, `switch_session`, `open_session`, `get_state`, `get_messages_page`, `get_tree`, `get_entries`, `get_branch_messages`, `prompt`, `steer`, `follow_up`, `abort`, `abort_and_prompt`, `bash`, `abort_bash`, `compact`, `fork`, `branch`, `navigate_tree`, `abort_branch_summary`, `set_label`, `set_session_name`, `get_session_stats`, `get_available_commands`, `export_html` (`docs/rpc.md:125-258`) |
| `QueueApi` | `queue/remove` (message text and queue: steering or follow-up), `queue/promote` (message text), `queue/restore` (abort and return the withdrawn messages). omp identifies queued messages by their text, not by id; the current queue arrives as the `queue_update` event and in `live/state` | `remove_queued_message`, `promote_queued_message`, `abort_and_restore_queue`, `queue_update`, `get_state.queuedMessages` (`docs/rpc.md:133-137,396-447,511-514`) |
| `ModelApi` | `model/list`, `model/set`, `model/cycle`, `thinking/levels`, `thinking/set`, `thinking/cycle` | `get_available_models`, `set_model`, `cycle_model`, `get_available_thinking_levels`, `set_thinking_level`, `cycle_thinking_level` |
| `DialogApi` | `dialog/respond` (answers an extension UI request) | extension UI responses |
| `EventsApi` | subscription `events/subscribe` → `events/event`, params `{ live: LiveSessionId, after: Option<EventId> }` | all frames |

omp's `cycle_thinking_level` can answer `auto`, a selector omp-rpc's generated `Effort` cannot decode, so that one step of the cycle fails with a gateway error. Setting a level and every other step work.

Reading a session's messages means opening it: `live/open` starts omp on that session file, and the UI pages its transcript with `live/messages` and reads its history tree with `live/tree`. No session content is read without a live omp; `session-index` reads headers and tail status only.

`prompt` text is sent unchanged. omp expands `/` commands, skills, templates, and `^model` itself. `!cmd` is `live/bash`; nothing else is parsed client-side. Out of scope: python, live voice, dictation, login, btw, goal, subagent control, host tools.

## Events

`Event { id: EventId, live: LiveSessionId, at: jiff::Timestamp, body: EventBody }`. `EventBody` is a tagged enum over omp's frames (`RpcNotification` and `RpcAgentEvent` in omp's wire module): agent start/end, turn start/end, message start/update/end (text and thinking deltas), tool execution start/update/stream update/end, queue update, prompt result, session settled, manual and auto compaction start/end, auto retry start/end, model changed, thinking level changed, notice, available commands update, command output, session info update, config update, extension UI request, extension error, process exited, frame dropped. Replay: a subscription with `after` first sends every retained event with a greater id, then streams live events, with no gap.

## Errors

`error_codes` module with named `i32` constants in the JSON-RPC server range, never reused once retired: unknown live session, unknown session, unknown project, session busy, stale cursor (an `events/subscribe` `after` id older than the retained log, or a subscriber that fell behind), command not allowed in current state, omp command failed (carries omp's `code`), omp process exited, invalid request. `ErrorData` carries typed context (ids, omp error code) as serde data. The client side maps `ErrorObject` back to a typed value; nothing becomes a `String`.

## History Tree

omp's wire schema leaves entries open (`$defs/SessionEntries.entries`: `{"type":"object","properties":{},"additionalProperties":{}}`), so the generator yields `Map<String, Value>`. The protocol types the fields every entry shares (`SessionEntryBase`, `session-entries.ts:92-96`) and keeps the rest open, as Gupi does (`crates/pi-rpc/src/protocol.rs:134-151`):

```rust
/// One omp session entry: shared fields typed, kind-specific fields kept as omp wrote them.
pub struct Entry {
    pub id: EntryId,                  // omp-minted string newtype, not a UUID
    pub parent_id: Option<EntryId>,
    pub timestamp: jiff::Timestamp,
    #[serde(rename = "type")]
    pub kind: EntryKind,              // string newtype: omp adds kinds across releases
    #[serde(flatten)]
    pub data: serde_json::Map<String, serde_json::Value>,
}

pub struct HistoryEntries { pub entries: Vec<Entry>, pub leaf: Option<EntryId> }
```

- The tree is built from `live/entries` (`get_entries`), not carried by `get_tree`. omp nests a tree node's children inside it, one JSON level per entry on a branch, and omp-rpc decodes a frame with serde_json's recursion limit: `get_tree` fails with "recursion limit exceeded" for a session of a few dozen entries on one branch (reproduced with a 300-entry session). The entries carry `parentId`, so the same tree follows, built by `alvin-conversation` with omp's rules (`session-manager.ts:694-723`).
- Shared fields are parsed once, in `alvin-omp-host`, where omp's output is read; an entry missing them is a typed error naming the entry id.
- `live/entries` takes an optional `since` (an entry id); omp answers an id it does not know with the command error code `unknown_since`, which reaches the client as `OmpCommandFailed` with that code.
- `live/branchMessages`, `live/navigateTree`, `live/branch`, `live/fork`, `live/label`, `live/abortBranchSummary` take `EntryId`.

## Omp Payload Types

omp-owned payloads (messages, entries, stats, model info, command results) are generated, not hand-written and not taken from `omp-rpc`.

- Version: one exact omp release (npm `@oh-my-pi/pi-coding-agent`, git tag `v<version>`). No commit pins. It is the version of the omp binary installed for tests, containers, and deployments.
- Root `package.json` declares `@oh-my-pi/pi-coding-agent` as a dev dependency at that exact version. Bun installs it; the lockfile is committed. The package ships `scripts/gen-rpc/` and `src/modes/rpc/wire/rpc-wire.schema.json`. Nothing refers to a local checkout or any path outside the repository.
- `mise run generate:omp-wire` (new task): runs omp's own `buildWireModel` and `emitRust` from `node_modules` over the schema from the same package, with our patch applied (below), and writes `crates/alvin-protocol/src/omp_wire.rs`. The generated file is committed and its header names the release.
- Patch: `crates/alvin-protocol/schema/rust.ts.patch`, applied to a temporary copy of `rust.ts`; the package in `node_modules` stays untouched. It adds our derives (`JsonSchema`, `Validate`) and per-field `#[validate(...)]` attributes from schema keywords, replaces the `#![allow(...)]` header with what `AGENTS.md` permits, and drops items we do not use. A patch that no longer applies fails the task.
- Updating: bump the version in `package.json`, run the task, commit the diff of lockfile and generated file. The `omp-rpc` git dependency in `alvin-omp-host` is set to `tag = "v<version>"` in the same change.
- `mise run check` fails when `package.json`, the `omp-rpc` tag, or the installed omp binary version disagree, or when the committed `omp_wire.rs` differs from what the task produces. Generation never runs from the build or from tests.
- The generated module builds for `wasm32-unknown-unknown` with the rest of the crate. It needs only `std`, `serde`, `serde_json`, plus the approved derive crates.
- Alvin types (ids, events, API traits, errors) reference generated types; `alvin-omp-host` converts `omp-rpc` values into them. The protocol crate never depends on `omp-rpc`.

Project structure additions: `package.json`, `bun.lock`, `crates/alvin-protocol/schema/rust.ts.patch`, `crates/alvin-protocol/src/omp_wire.rs`, `.mise/tasks/generate/omp-wire`.

```
crates/alvin-protocol/src/lib.rs          thin: modules and re-exports
crates/alvin-protocol/src/error.rs        Error, Result
crates/alvin-protocol/src/error_codes.rs  stable codes + ErrorData
crates/alvin-protocol/src/ids.rs
crates/alvin-protocol/src/projects.rs     types and trait
crates/alvin-protocol/src/sessions.rs
crates/alvin-protocol/src/live.rs
crates/alvin-protocol/src/queue.rs
crates/alvin-protocol/src/model.rs
crates/alvin-protocol/src/dialog.rs
crates/alvin-protocol/src/events.rs
```

## Code Style

Per `AGENTS.md`. Request types with more than two fields use `typed-builder`. Every public item has a doc comment; fallible items document `# Errors`.

```rust
#[cfg_attr(feature = "server", jsonrpsee::proc_macros::rpc(client, server))]
#[cfg_attr(not(feature = "server"), jsonrpsee::proc_macros::rpc(client))]
pub trait LiveApi {
  /// Sends a prompt to a live session; omp expands commands server-side.
  #[method(name = "live/prompt")]
  async fn prompt(&self, request: PromptRequest) -> RpcResult<()>;
}
```

## Testing Strategy

- Serde round trip of every request, response, and event variant, including unknown-field handling decided by the types.
- Ids: UUIDv7 version bit set; ordering follows mint order.
- Error codes: a test pins the full code table so a change is deliberate.
- Build checks: native with and without `server`, and `wasm32-unknown-unknown` without `server`, via mise tasks added by this module.
- The method list is pinned in `alvin-gateway`, not here.

## Boundaries

- Always: follow `AGENTS.md`; doc every public item.
- Ask first: new dependency, any IO or runtime in this crate.
- Never: depend on an alvin crate, on `omp-rpc`, on tokio, add `seq`, shape types after ACP, flatten errors to `String`.

## Open Questions

None.

Approved: Bun in `mise.toml`; protocol-level shape validation wording in `AGENTS.md`; `validator` and `schemars`; blake3 path ids.

First slice of this module: generate from the real release and report what compiles, what the patch must change, and which schema keywords exist for validation, before anything else depends on the output.
