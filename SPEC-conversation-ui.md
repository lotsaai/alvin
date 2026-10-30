# Spec: conversation-ui

Module id `conversation-ui`, crate `alvin-conversation-ui`. Depends on `conversation`, `client`. Parent: `SPEC.md`. Law: `AGENTS.md`. GPUI Kit rules: `.agents/skills/gpui-kit/references/coding-guides.md`.

## Objective

The gpui-kit views of alvin: browse projects and sessions, open several live sessions, chat with streaming text and tool output, steer and queue, pick model and thinking level, answer extension dialogs, and work the history tree. The views render `alvin-conversation` state and issue commands through `alvin-client`. Gupi parity, limited to what omp-rpc supports (`SPEC.md`).

Success:

- Every protocol method has a UI path that issues it, and every `EventBody` variant has a visible effect, both covered by tests.
- A live turn streams into the transcript without re-rendering earlier rows; the transcript and history tree stay responsive with thousands of entries.
- A reconnect, a `Resync`, an omp exit, and a wire error each have a defined, visible state; none leaves stale content on screen.
- `cargo build --target wasm32-unknown-unknown -p alvin-conversation-ui` succeeds, through `mise check`.

## Out of Scope

Gupi features that do not go through omp-rpc: file browser, git changes panel, workspace diff review, camera, quote capture, settings window, tray, temporary windows. They need filesystem or git access the UI does not have (the UI sees only the gateway). Adding them means new gateway methods; ask first.

## Layout

```
┌ TitleBar: live session tabs ─────────────────────────────────────────┐
│ Sidebar        │ Session view                         │ History tree │
│  projects      │  transcript (MessageScroller)        │ (toggle)     │
│   checkouts    │                                      │              │
│    sessions    │  widgets from extensions             │              │
│  unresolved    │  queue                               │              │
│                │  composer                            │              │
├ StatusBar: connection · model · thinking · context use · ext status ┤
└──────────────────────────────────────────────────────────────────────┘
```

Components (gpui-kit 0.8.1, import paths from the skill's catalog; exact signatures read from the component pages when implementing): `TitleBar`, `TabBar`, `Sidebar`, `Resizable`, `MessageScroller`, `Message`/`Bubble`, `TextView::markdown`, `ShimmerText`, `Collapsible`, `Diff`, `Textarea`, `Combobox`, `Select`, `Command`, `Dialog`/`AlertDialog`, `Notification`, `StatusBar`, `Spinner`, `Badge`, `v_virtual_list`.

## Views

One file per view; each is an `Entity<T>` with `Render` where it retains state, `RenderOnce` otherwise (coding guides, "Choose the right unit").

- `Workspace` (root view, one per window): owns the `alvin_client::Client`, the `Entity<Model>` (below), the sidebar, the open tabs, and the active session view. Placed directly under gpui-kit's `Root`.
- `ProjectSidebar`: projects → checkouts (main, git worktrees, jj workspaces) → sessions, newest first, paged on scroll (`sessions/list`); the unresolved groups with their reason; a live marker on sessions open in a tab. Actions: open (`live/open` with the session file), new session in a checkout (`live/open` new, model optional), refresh.
- `SessionView` (one per live session): transcript, widgets, queue, composer, and the session menu (rename, compact, stats, export HTML, close).
- `Transcript`: `MessageScroller` over the active branch. Rows are committed messages plus one streaming row. Row kinds follow omp's `AgentMessage` variants: user (text, images), assistant (markdown text, thinking in a `Collapsible`, tool calls), tool result, bash execution, branch summary, compaction summary, custom messages. Older pages load on scroll to the top (`live/messages` cursor); `MessageScroller` keeps the reader's position on insert.
- `ToolCard`: one per tool call, keyed by `tool_call_id`. Running state with streamed output (`tool_execution_update`/`stream_update`), then the result. Renderers by tool name for omp's built-in tools (bash: command and output; read; edit and write: `Diff` when the result carries a patch; grep, find, glob: match lists; task: subagent summary). Any other tool name renders its args and result as formatted JSON; tool args and results are open JSON in omp's schema, so a tool alvin does not know is a normal case, not an error. Which result fields carry patches is read from recorded fixtures in the first slice.
- `Composer`: `Textarea`. Enter sends, Shift+Enter inserts a newline. Text starting with `!` goes to `live/bash` with the rest as the command; everything else goes to `live/prompt` unchanged, and omp expands `/` commands, skills, templates, and `^model` (`SPEC.md`). While a run is active, Enter sends with omp's `streamingBehavior` (steer by default, follow-up with a modifier). Escape aborts (`live/abort`; `live/abortBash` while a bash command runs). Images pasted or dropped become the prompt's `images` (`docs/rpc.md:130`). `/` opens completion from the session's command catalog (`available_commands_update`); choosing one inserts text, it does not run anything. `set_editor_text` requests replace the composer text.
- `QueueView`: steering and follow-up entries from `queue_update`, with remove and promote (`queue/remove`, `queue/promote`). Entries counted in `live_steered` show as sent and have no actions. After an abort, `queue/restore` returns withdrawn text to the composer.
- `ModelPicker` and `ThinkingPicker`: `Combobox` over `model/list`, `Select` over `thinking/levels`; set and cycle via `model/*` and `thinking/*`. Current values from `live/state` and the model and thinking events.
- `HistoryTreeView`: the session's tree (`alvin-conversation` `tree`), rendered as rows in a `v_virtual_list`: a chain of single children stays at one indent, a branch point indents its children, so a long linear session is a flat list. The active path (root to leaf) is highlighted; labels show inline. Actions on a row: navigate here (`live/navigateTree`, with optional summary and custom instructions), branch from a user message (`live/branch`, candidates from `live/branchMessages`), fork (`live/fork`), set or clear a label (`live/label`), cancel a running branch summary (`live/abortBranchSummary`). gpui-kit's `Tree` is not used: it nests one level per child, which for omp's chains means one level per message.
- `DialogHost`: extension UI requests by request id. `select`, `confirm`, `input`, `editor` open a `Dialog` (one at a time per session, queued in arrival order) and answer with `dialog/respond`; a `cancel` from omp for that id closes it. `notify` pushes a `Notification`. `setStatus` updates the status bar segment for that key. `setWidget` renders the widget lines above the composer.
- `ConnectionBanner` and `SessionBanner`: `Reconnecting { attempt }` and `Lost` from `Client::state()`; reload-needed after `Resync`; process exited with a close action.

## State and Async

- `Model` (`Entity`): wraps `alvin_conversation` state. It is the only owner of domain state; views read it and keep only transient view state (scroll, expanded cards, composer text) per the coding guides' "State ownership".
- One task per open live session, started with `cx.spawn` when the tab opens and dropped when it closes, reads `Client::events(live)` and calls `Model::apply` through a weak handle, notifying once per event. A `Resync` item calls `resync`, then reloads with `live/messages` and `live/tree` and starts a new stream.
- `tree_stale` triggers `live/entries { since: tree_cursor }`; one request in flight per session, later triggers coalesce into it.
- Commands run with `cx.spawn` and the client; their effects arrive as events. Each request carries the live session id it was issued for and the result is dropped if that session closed meanwhile (coding guides, "Async work").
- Errors: a typed `alvin_client::Error` shows as a `Notification` with the protocol code's name and the message; process-exited and stale-cursor codes route to the banners instead. Nothing is only logged.
- Runtime: this crate has no tokio dependency and creates no runtime. The face owns the runtime and builds the `Client` (`skill://gpui-tokio-client-wiring`); the views receive the client.

## Actions and Keys

`actions!` in `actions.rs`: `Submit`, `Newline`, `Abort`, `FocusComposer`, `ToggleSidebar`, `ToggleHistoryTree`, `NextTab`, `PreviousTab`, `CloseTab`, `CycleModel`, `CycleThinking`, `NewSession`. Bindings are registered by the crate's `init(cx)`, which the face calls after `gpui_kit::init`.

## Identity

`ElementId`s come from domain ids: `LiveSessionId` for tabs and session views, omp `message_id` and `EntryId` for transcript and tree rows, `tool_call_id` for tool cards, project and checkout ids in the sidebar, request id for dialogs. No list indexes.

## Platform

Image paste and drop, and clipboard copy, go through GPUI's platform APIs; where the web platform lacks one, the control is not shown on wasm32 (gpui-kit WebAssembly docs, "Web capabilities to plan for"). Everything else is shared. Fonts for the web build are the web face's concern.

## Dependencies

`alvin-conversation`, `alvin-client`, `alvin-protocol`, `gpui-kit`, `serde_json` (rendering open tool JSON), `alvin-workspace-hack`. Dev: `gpui-kit` with `test-support`, `tokio` (`rt`, `macros`, for `#[tokio::test]`), and `alvin-gateway` (`AGENTS.md` rule 7). Latest stable of each.

## Project Structure

```
crates/alvin-conversation-ui/src/lib.rs           thin: init(cx), re-exports Workspace
crates/alvin-conversation-ui/src/actions.rs
crates/alvin-conversation-ui/src/model.rs         Entity wrapper over alvin-conversation, event tasks
crates/alvin-conversation-ui/src/workspace.rs
crates/alvin-conversation-ui/src/sidebar.rs
crates/alvin-conversation-ui/src/session.rs
crates/alvin-conversation-ui/src/transcript.rs
crates/alvin-conversation-ui/src/tool_card.rs
crates/alvin-conversation-ui/src/composer.rs
crates/alvin-conversation-ui/src/queue.rs
crates/alvin-conversation-ui/src/pickers.rs
crates/alvin-conversation-ui/src/history_tree.rs
crates/alvin-conversation-ui/src/dialogs.rs
crates/alvin-conversation-ui/src/status.rs        status bar and banners
```

## Testing Strategy

- Tree row flattening (indent rule, active path) is pure logic: plain `#[test]` in its module.
- UI integration tests in `#[cfg(test)] mod tests` inside each view's module (`AGENTS.md`, Testing; `skill://gpui-tokio-client-wiring`), `#[tokio::test]` stacked over `#[gpui_kit::test]`, headless window with the real `Workspace`, an embedded gateway over the channel transport, real omp (`--mode rpc-ui`, `zai/glm-5.3-flash`) in a temporary project. Interaction through `TestWindowExt` (clicks, typed text, named keys); assertions on view and model state and on the accessibility tree, never pixels. Explicit imports, never `use gpui_kit::*` or `use super::*`. No `tests/`.
- Flows: open a listed session and page its transcript; new session, prompt, streamed reply ends settled; a tool call renders a card that completes; `!cmd` runs bash; abort mid-turn; steer while running and see the queue update, then remove an entry; change model and thinking; an `ask` tool call opens a dialog and the answer reaches omp, and a second test cancels it; history tree: branch from a user message, navigate back, label a node, fork and see the tab follow the new file; close a tab and confirm its omp is gone.
- Failure states: kill omp and see the exited banner; drop the gateway's channel and see `Lost`; a command rejected by the gateway shows its error code.

## Boundaries

- Always: follow `AGENTS.md` and the gpui-kit coding guides; read the component page before using a component; domain ids for identity.
- Ask first: new dependencies; features outside omp-rpc; a custom `Element`.
- Never: depend on `alvin-gateway` outside dev-dependencies, `alvin-omp-host`, or `alvin-session-index`; parse composer text beyond the leading `!`; create a tokio runtime; keep domain state in a view; assert pixels.

## Open Questions

None. Decided: `alvin-gateway` is a dev-dependency of this crate (`AGENTS.md` rule 7); the out-of-scope list stays out of v1.
