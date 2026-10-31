# Spec: conversation

Module id `conversation`, crate `alvin-conversation`. Depends on `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

Pure UI state. Folds protocol responses and events into the model the views render: projects, sessions, live sessions with their transcript, streaming turn, tool calls, queue, dialogs, and history tree. No IO, no async, no client calls. Builds for `wasm32-unknown-unknown`.

Success:

- Every `EventBody` variant has a defined effect on state, tested.
- Replaying the events of a real recorded omp run produces the same final transcript as `live/messages` returns for that session afterwards.
- A `Resync` from the client resets the live session's transcript to a reload-needed state; nothing stale stays on screen.

## Model

Shapes follow omp's wire types (generated into `alvin-protocol`, read from omp-rpc `wire.rs`):

- `Workspace`: projects with checkouts (`projects/list`), and the paged session list per checkout (`sessions/list`).
- `LiveSessions`: `LiveSessionId` to `LiveView`.
- `LiveView`:
  - `transcript`: committed messages (`AgentMessage`: user, assistant, tool result, bash execution, branch and compaction summaries, and the other variants omp defines), loaded newest page first from `live/messages` (`MessagesPage { messages, total_messages, next_cursor }`) and extended by `message_end`.
  - `streaming`: the assistant message being built, keyed by omp's `message_id`, from `message_update` deltas (`AssistantMessageEvent`: text, thinking, tool call start/delta/end, done, error). `message_end` replaces it with the committed message.
  - `tools`: running tool executions keyed by `tool_call_id`, from `tool_execution_start`/`update`/`stream_update`/`end`. Args and results stay `serde_json::Value`, as omp's schema defines them; the view renders them per tool name.
  - `queue`: `steering` and `follow_up` text lists plus `live_steered` (leading steering entries already sent, which `queue/remove` cannot reach), replaced whole by each `queue_update`.
  - `run`: idle, running, or settled, from `agent_start`, `agent_end` (`yielded`, `awaiting_async_work`), `prompt_result`, `session_settled`. Plus compaction and retry progress.
  - `model` and `thinking`: from `live/state`, updated by `model_changed`, `thinking_level_changed`, `config_update`.
  - `dialogs`: pending extension UI requests by request id, removed when answered or cancelled (`cancel` with `targetId`).
  - `commands`: the slash-command catalog from `available_commands_update`.
  - `notices`: `notice`, `command_output`, `extension_error`, untranslated-frame and dropped-frame events, in arrival order.
  - `title`: from the session list and `session_info_update`.
  - `exited`: set by the process-exited event.
  - `tree`: the history tree (`HistoryTree`: roots, children, labels, leaf) from `live/tree`, kept current from `live/entries { since }`. The leaf marks the active branch; the transcript is that branch. omp sends no tree-change event, so `turn_end`, `agent_end`, compaction end, and the replies to `live/navigateTree` and `live/label` set `tree_stale`; the UI then fetches entries since `tree_cursor`. `live/branch` and `live/fork` move the session onto a new file (`docs/rpc.md:235-247`), so after either the UI fetches the whole tree and the first transcript page again.
- `ConnectionView`: the client's connection state (`Connecting`, `Connected`, `Reconnecting`, `Lost`), set by the UI from `Client::state()`.

## Operations

All are plain methods on owned state; none fail except on decoding, and decoding is done by the protocol types before values reach here.

- `apply(&mut self, &Event)`: the fold. Events arrive in `EventId` order with no duplicates (the client guarantees it); this crate does not re-check order.
- `load_page(live, MessagesPage)`, `set_state(live, LiveState)`, `set_projects(..)`, `set_sessions(checkout, page)`.
- `resync(live, reason)`: drops `transcript`, `streaming`, `tools`, and `tree` for that session and marks it reload-needed.
- `set_tree(live, HistoryTree)`: replaces the tree; used on open, after a fork, and when `live/entries` answers omp's `unknown_since`.
- `apply_entries(live, SessionEntries)`: inserts entries newer than the tree's newest under their `parentId` (a missing parent makes a root, as omp does), applies `label` entries to their `targetId` (empty label clears), and sets `leaf`. Same rules as omp's `tree()` and label index (`session-manager.ts:590-591,694-723`). New entries are newest, so appending keeps children in omp's timestamp order.
- `tree_cursor(live)`: the newest entry id, the `since` for the next `live/entries`.
- `close(live)`.

Commands the user issues (prompt, abort, queue edits, model change, dialog answers) are sent by `alvin-conversation-ui` through `alvin-client`; their effect arrives back as events.

## Dependencies

`alvin-protocol`, `alvin-workspace-hack`. Nothing else (`AGENTS.md` table).

## Project Structure

```
crates/alvin-conversation/src/lib.rs        thin
crates/alvin-conversation/src/error.rs      only if a fallible operation appears
crates/alvin-conversation/src/workspace.rs  projects, checkouts, session lists
crates/alvin-conversation/src/live.rs       LiveView and the event fold
crates/alvin-conversation/src/streaming.rs  assistant message assembly
crates/alvin-conversation/src/tools.rs      tool execution tracking
crates/alvin-conversation/src/queue.rs
crates/alvin-conversation/src/dialogs.rs
crates/alvin-conversation/fixtures/         events recorded from real omp runs
```

Gupi splits the same concerns into `crates/gupi-conversation/src/conversation/{messages,queue,execution,model_change,compaction,shell,navigation}.rs` (file list read from the code graph; contents not read).

## Testing Strategy

- Plain `#[test]` in `#[cfg(test)] mod tests` inside each module; no `tests/` (`AGENTS.md`, Testing).
- Fixtures: event sequences and `live/messages` pages recorded from real omp runs (`--mode rpc-ui`, `zai/glm-5.3-flash`) through the gateway, stored as JSON. Recorded real data, not hand-written frames. A mise task re-records them; tests never start omp.
- Fold tests: a plain turn, a turn with tool calls, abort, queue steering and promotion, compaction, an extension dialog answered and one cancelled, process exit.
- Tree tests over recorded `get_tree` and `get_entries` output: applying entries since the last known id to the tree from before gives the same tree a fresh `live/tree` returns afterwards (labels and leaf included); after a branch, a navigate with summary, a label set and cleared, and a fork.
- Equivalence: folding a recorded run's events gives the transcript the same session's `live/messages` returned after the run.
- Resync clears the live parts and keeps the session listed.

## Boundaries

- Always: follow `AGENTS.md`; keep it pure.
- Ask first: any dependency.
- Never: IO, async, depending on `alvin-client` or `gpui-kit`, re-validating event order, parsing omp text.

## Open Questions

None. Approved: typed history tree with open entry bodies (`SPEC-protocol.md`, History Tree); fixture recording task `mise run generate:conversation-fixtures`.
