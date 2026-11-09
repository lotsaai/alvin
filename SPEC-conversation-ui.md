# Spec: conversation-ui

Module id `conversation-ui`, crate `alvin-conversation-ui`. Depends on `conversation`, `client`. Parent: `SPEC.md`. Law: `AGENTS.md`. GPUI Kit rules: `.agents/skills/gpui-kit/references/coding-guides.md`.

## Objective

The gpui-kit views of alvin: browse projects and sessions, open several live sessions, chat with streaming text and tool output, steer and queue, pick model and thinking level, answer extension dialogs, and work the history tree. The views render `alvin-conversation` state and issue commands through `alvin-client`. Gupi parity, limited to what omp-rpc supports (`SPEC.md`).

Success:

- Every protocol method has a UI path that issues it, and every `EventBody` variant has a visible effect, both covered by tests.
- A live turn streams into the transcript without re-rendering earlier rows; the transcript and history tree stay responsive with thousands of entries.
- A reconnect, a `Resync`, an omp exit, and a wire error each have a defined, visible state; none leaves stale content on screen.
- `cargo build --target wasm32-unknown-unknown -p alvin-conversation-ui` succeeds. **Not met today:** gpui-kit's browser backend (`wasm_thread`) needs a nightly compiler and the project builds on stable. The crate has no wasm-specific code, and `mise check` guards its dependencies instead.

The terminal uses the locally maintained `vendor/ghostty-vt` and `vendor/ghostty-vt-sys` crates, derived from version `0.8.0+ghostty.1.4.0-main.83fb956`. Both dependencies resolve by local path. The sys crate downloads the official Ghostty source pinned in `GHOSTTY.lock`, verifies its SHA-256, and builds it with Zig. Its Windows downloader uses the platform certificate verifier. Upstream licenses remain in each vendored directory.

## Out of Scope

Gupi features that do not go through omp-rpc: file browser, git changes panel, workspace diff review, camera, quote capture, tray, temporary windows. They need filesystem or git access the UI does not have (the UI sees only the gateway). Adding them means new gateway methods; ask first. A right-side tabbed editor and generative UI are deferred, not implemented.

## Layout

```
┌ Sidebar controls ─────┬─ Inset session header ──────────────────┐
│ Sidebar               │ Conversation                │ History   │
│  projects             │  transcript                 │ optional  │
│   checkouts           │  context trigger / composer │           │
│    sessions           ├─ terminal (optional) ────────┤           │
└───────────────────────┴──────────────────────────────┴───────────┘
```

The outer title bar holds sidebar and back/forward controls; the inset work area's header shows the active session title, menu and close control, not a tab strip, with a terminal toggle at the trailing edge. Open live sessions remain available through sidebar navigation and back/forward history. The composer groups the editor, model and thinking controls, and Send or Stop in one card. Its top-edge Context trigger opens a popover and refreshes statistics: it shows available session details, last reported context usage and usage totals. The trigger displays the model's context-window capacity, not current usage. History opens a separate auxiliary region. The terminal is below the conversation.

Components include `TitleBar`, `Resizable`, `MessageScroller`, `Editor`, `Select`, `Popover`, `Dialog`, `Notification`, and `v_virtual_list`.

## Views

Modules are grouped by capability: workspace composition, live-session interaction, and terminal interaction. Each capability declares its own implementation modules; `lib.rs` contains only crate documentation, capability declarations, and explicit public exports. Views use `Entity<T>` with `Render` where they retain state, `RenderOnce` otherwise (coding guides, "Choose the right unit").

- `Workspace` (root view, one per window): owns the `alvin_client::Client`, the `Entity<Model>` (below), the sidebar, the open tabs, and the active session view. Placed directly under gpui-kit's `Root`.
- `ProjectSidebar`: projects → checkouts (main, git worktrees, jj workspaces) → sessions, newest first, loaded in pages. Search and recently updated grouping load all remaining pages before displaying results, with loading and retry states. The project/worktree hierarchy retains explicit controls for loading more pages. Project and checkout scope rows use distinct icons and stronger type. A sole root checkout is flattened into its project. New sessions start from a compact `+` on the applicable scope; projects with multiple checkouts expose creation on each checkout. Scope actions appear on hover or keyboard focus and remain visible while their menu is open.
  Projects start collapsed; expanding a project or checkout loads its first session page on demand. Duplicate project folder names include their full paths unless an alias is set.
  The search icon opens a popover with a focused search field. Search filters as the user types; Escape or clicking outside dismisses the popover, clears the query, and returns focus to the search icon.
  The sidebar keeps New session above the independently scrolling projects. New session asks for an existing project checkout instead of choosing one implicitly. The fixed footer shows the local display name and avatar, a user menu with Settings…, and a Settings button. The face supplies the local name; absent names display Local user. The sidebar title strip retains sidebar toggle and session back/forward controls even when the sidebar is hidden. Live sessions appear in their project or checkout tree alongside indexed history.
  Project aliases, pins, and hidden state and session pins and hidden state are local Alvin preferences. Hiding never deletes repositories or omp files; hidden projects and sessions have restore controls. Session rows show the file's relative modification age, recalculated when the view renders. A spinner means an open live session is running, not that a historical transcript ends with an unanswered message.
  The active session is highlighted by its session ID, known on open for listed sessions and from `live/state` for new sessions; the live file identifies a new session until its state loads. Keyboard tree selection remains independent of the active tab.
- `SessionView` (one per live session): transcript, widgets, queue, status, composer, context popover, and the session menu (rename, compact, stats, export HTML, and model and thinking cycling). The header close control closes the active session. A newly opened session has a centred greeting and composer until a command succeeds. An existing session with an empty transcript remains a transcript, not a new-session view. The gateway opens a new live session when the user selects a checkout, before the first command.
- `TerminalPanel`: one retained terminal per checkout and workspace window, below the active conversation in a vertical resizable region. The titlebar button and `ToggleTerminal` action show or hide it without stopping its shell. Closing the panel stops the gateway-owned process; opening after close starts a new shell. The terminal is a sibling of `SessionView`, so its keys do not invoke composer actions.
- Terminal rendering uses the private gpui-cn terminal source cut and `ghostty-vt`; chrome uses the active gpui-kit theme. The retained engine consumes ordered output while hidden. A reconnect resumes into the same engine from the last accepted event; input is never automatically retried.
- Terminal font family, ordered fallbacks, size, colors, palette, and cursor defaults come from the application's terminal appearance settings. The workspace applies changes to retained panels and supplies them to new panels. Unset font values follow the theme's monospace defaults rather than app zoom; a terminal's explicit zoom remains until reset.
- `Transcript`: a retained registry behind `MessageScroller`. Message and interactive content entities survive virtual-row eviction and streaming completion. Event modules own presentation and local interaction state; `SessionView` coordinates the transcript rather than rendering each event kind. Assistant text, reasoning, calls and injected messages keep source order. A matched result updates its call-position block; orphan results remain separate events in arrival order. Errors remain visible with partial answers. Markdown fences use Verdant language detection and active-theme highlighting.
  Highlighting uses Verdant 0.8.0 with the `most` parser feature, including Markdown and its inline grammar. Dependencies retain the supported `main-git-deploy` source because the published `verdant-queries` 0.8.0 package omits its generated query files.
  User bubbles use the application-provided `UserMessageColors` background and foreground, falling back to the theme's primary colors when no global is installed. Assistant and tool roles keep their own theme colors.
  Transcript and composer share a centred lane capped at 120 columns of the active monospace font's measured glyph advance; zoom and font changes update the cap. Widescreen fills the available width with the same inset and keeps its selection across sessions for the application lifetime.
  Message footers expose Copy message and an overflow menu, with the same commands in the context menu. Copy preserves user text and assistant text or narration. Local Collapse and Expand do not alter history. Restore to composer never replaces a draft. Reported generation metadata appears once on its owning message, not as invented tool statistics. The messages response has no authoritative session entry mapping, so message actions cannot target entries for branching or editing.
  `ConversationTimeline` marks loaded user messages at the transcript's left edge when content width is at least 54rem; narrower content uses a Queries menu in the session header, outside the scrolling transcript. Hover or focus previews a query. Selecting it scrolls to its presentation row without branching or changing history; the rail supports arrow keys, Home, End, Enter and Space.
  Existing conversations reserve a 2.5rem query-navigation gutter before the horizontally clipped transcript viewport. Activity markers have separate clearance within that viewport; activity content clips independently so horizontal output cannot paint beneath either marker column.
  Thinking starts collapsed and renders muted, left-aligned Markdown when expanded. Manual process folding leaves the final answer and its actions visible, restores child disclosures when reopened and never runs automatically on completion.
- `ToolBlock`: one retained operation containing an outer header, a retained `ToolCard`, and a footer for reported tool-scoped statistics. Bash `details.wallTimeMs` supplies elapsed time when present; an explicitly reported exit code belongs to the inner card footer. Successful results without an exit code do not invent zero. Potentially mutating and unknown tools start expanded; known read-only tools start collapsed. General → Conversation persists mutating-tool, read-only-tool and thinking defaults application-wide. An explicit block choice overrides its default through streaming, regrouping and settings changes. Non-script bodies have a 20rem preview; Show all/Show less, raw-data disclosure and card collapse have independent retained state. Controls remain outside the preview cap. Full-copy actions preserve complete output. Unknown tools and raw payloads remain inspectable. Edit patches use native `Diff` with Verdant highlighting and retained file-collapse state. Writes show supplied content rather than a fabricated diff when prior contents are unavailable.
  Read resources retain reported paths, links and ranges. Multi-target and compatible adjacent reads provide independent target disclosures without crossing intervening events; combined output remains available once, without inventing which target produced each segment. Search renders reported files and matches without discarding original output.
  Task rows retain reported identity, assignment, status and progress, including updates after an asynchronous spawn response. A completed synchronous response remains authoritative over older progress. Matching `async-result` deliveries and direct `wait` results settle the original operation; durations come from the reported job, not message latency. Attributable delivered text becomes normal output and full-copy content, while raw inspection keeps the original response, latest available progress and full delivery. A custom batch with only shared text does not invent per-job attribution. Unknown statuses and unmatched deliveries remain inspectable.
- `CurrentPlan`: one retained projection of typed todo phases, separate from historical todo results. Completed counts, task status, completed strikethrough and reported blockers remain visible. Reminders use System activity rather than another plan card.
- `SystemActivity`: session notices and source-defined transcript activity share compact severity/provenance treatment and retained details, original-content copy and raw inspection. Internal agent messages are a notice kind, not a provider-specific UI category: preserve sender/source and complete content, with neutral treatment unless a severity is reported. Provider forms such as omp IRC reconcile live and committed history without duplicate arrivals. Unknown custom types follow the same category policy; `display:false` remains hidden. Ordinary user text is not reclassified by tag-like strings. Session activity starts behind one collapsed summary with a 12rem independently scrolling details region. Routine information creates no notification overlay; warning/error previews are bounded while complete originals remain inspectable. Typed todo reminders retain their reported attempt and complete task payload without replacing the current plan.
- `Composer`: a markdown `Editor`. Enter sends, Shift+Enter inserts a newline. Text starting with `!` goes to `live/bash` with the rest as the command; everything else goes to `live/prompt` unchanged, and omp expands `/` commands, skills, templates, and `^model` (`SPEC.md`). While a run is active, Enter steers by default; modified Enter queues a follow-up. With an empty editor, Stop aborts the active turn or bash command. If messages are queued, Restore queue returns them to the editor instead of aborting. Interrupt and send replaces the active turn with the editor text. `/` offers completion from the session's command catalogue; choosing one inserts text without running it. `set_editor_text` requests replace the composer text.
  Expand fills the available session content with the same editor state, preserving draft, selection, undo and completion. In expanded mode Enter inserts a newline, Command/Control+Enter submits using existing follow-up semantics, and Escape collapses without discarding the draft. Completion handles its own keys first.
  Dictation captures audio on the desktop and transcribes it locally through `alvin-stt` using the multilingual Nemotron 3.5 streaming model. First use downloads approximately 2.6 GB with a progress bar and estimated remaining time; later uses reuse its cache. The microphone button starts and stops capture. Recognized text appears in the editable composer during recording, beginning at the current selection. Later chunks preserve a user selection and follow earlier chunks in order. Text arriving during IME composition waits until composition ends. Each update is undoable, and the user sends the draft explicitly. Escape, submission, session navigation and closing, settings, and shutdown cancel pending audio and results without removing text already inserted.
- `QueueView`: steering and follow-up entries from `queue_update`, with remove and promote (`queue/remove`, `queue/promote`). Entries counted in `live_steered` show as sent and have no actions.
- `ModelPicker` and `ThinkingPicker`: `Select` controls for `model/list` and `thinking/levels`; set values in the composer and cycle them from the session menu. Current values come from `live/state` and model and thinking events.
- `HistoryTreeView`: the session's tree (`alvin-conversation` `tree`), rendered as rows in a `v_virtual_list`: a chain of single children stays at one indent, a branch point indents its children, so a long linear session is a flat list. The active path (root to leaf) is highlighted; labels show inline. Actions on a row: navigate here (`live/navigateTree`, with optional summary and custom instructions), branch from a user message (`live/branch`, candidates from `live/branchMessages`), fork (`live/fork`), set or clear a label (`live/label`), cancel a running branch summary (`live/abortBranchSummary`). gpui-kit's `Tree` is not used: it nests one level per child, which for omp's chains means one level per message.
  At a session-content width of at least 80rem, History opens inline at 20rem; narrower content uses a popover with the same retained history view.
- `DialogHost`: extension UI requests by request id. `select`, `confirm`, `input`, `editor` open a `Dialog` (one at a time per session, queued in arrival order) and answer with `dialog/respond`; a `cancel` from omp for that id closes it. `notify` is retained as System activity, with bounded notification previews only for warnings/errors. `setStatus` updates the status bar segment for that key. `setWidget` renders the widget lines above the composer.
- `ConnectionBanner` and `SessionBanner`: `Reconnecting { attempt }` and `Lost` from `Client::state()`; reload-needed after `Resync`; process exited with a close action.

## State and Async

- `Model` (`Entity`): wraps `alvin_conversation` state. It is the only owner of domain state; views read it and keep only transient view state (scroll, expanded cards, composer text) per the coding guides' "State ownership".
- One task per open live session, started with `cx.spawn` when the tab opens and dropped when it closes, reads `Client::events(live)` and folds events through a weak model handle. State-change events distinguish session content, session metadata, workspace lists, and connection state. A session view subscribes only to its own session changes; streaming content does not synchronize other sessions or rebuild workspace navigation. Run-state and title changes still update sidebar and tab metadata. A `Resync` item calls `resync`, then reloads history and starts a new stream.
- `tree_stale` triggers `live/entries { since: tree_cursor }`; one request in flight per session, later triggers coalesce into it.
- A first-page session refresh invalidates outstanding page requests for that list; replies apply only while their generation and expected cursor remain current. Concurrent opens all register tabs, but only the latest navigation intent activates one; selecting a tab supersedes pending opens.
- Session navigation history is transient per workspace. Opening a session or selecting a tab records the active visit; back and forward traverse without recording another visit, supersede pending opens, and disable at their bounds. Closing a session removes its entries.
- Commands run with `cx.spawn` and the client; their effects arrive as events. Each request carries the live session id it was issued for and the result is dropped if that session closed meanwhile (coding guides, "Async work").
- Errors: a typed `alvin_client::Error` shows as a `Notification` with the protocol code's name and the message; process-exited and stale-cursor codes route to the banners instead. Nothing is only logged.
- Runtime: this crate has no tokio dependency and creates no runtime. The face owns the runtime and builds the `Client` (`skill://gpui-tokio-client-wiring`); the views receive the client.

## Actions and Keys

`actions!` in `actions.rs` registers `Abort`, `ToggleSidebar`, and `ToggleTerminal`. The crate's `init(speech_cache_dir, cx)`, called after `gpui_kit::init`, binds Escape to Abort in a session, and the platform modifier plus B or J to the workspace sidebar or terminal. Active dictation handles Escape before composer completion or agent abort.

## Identity

Transcript element identities combine the live session, snapshot generation and retained source slot. An optional process `messageId` arriving late does not re-key an existing component. Interactive entities retain their state through streaming completion and virtual-row eviction. Replacing a historical snapshot resets the registry because its messages have no authoritative entry mapping. The history tree retains its separate `EntryId` actions.

## Platform

Supported inline `ImageContent` renders its actual base64 image bytes or supplied URL. Invalid data, missing sources and unsupported MIME types remain visible errors; raw inspection retains the original payload. Metadata-only image reads do not imply an image preview. This does not add clipboard paste or drop support. Fonts for the web build are the web face's concern.

## Dependencies

`alvin-conversation`, `alvin-client`, `alvin-protocol`, `alvin-stt`, `gpui-kit` with speech, `async-channel`, `base64` (inline image decoding), `thiserror`, and `serde_json` (rendering open tool JSON). Dev dependencies include `gpui-kit` with `test-support`, `tokio`, `test-context` (real asynchronous fixture lifecycle), `tempfile`, `hound`, and `alvin-gateway` (`AGENTS.md` rule 7).

## Project Structure

```
crates/alvin-conversation-ui/src/
  lib.rs                 capability declarations and public exports only
  actions.rs             application key bindings
  error.rs               crate error boundary
  model.rs               domain state, typed change events, client tasks
  workspace.rs           window composition and navigation
  workspace/sidebar.rs   project and session browsing
  session.rs             live-session composition and subscriptions
  session/               composer, timeline navigation, message chrome, queue,
                         history, dialogs, pickers, status, markdown and ANSI
  session/transcript.rs  scroller composition and source-row grouping
  session/transcript/    retained registry, event modules, content modules,
                         tool block, tool card and specialized tool adapters
  terminal.rs            terminal capability interface and key bindings
  terminal/              panel, settings, rendering engine and transport
  test_support.rs        shared headless test setup
```

## Testing Strategy

- Tree row flattening (indent rule, active path) is pure logic: plain `#[test]` in its module.
- UI integration tests in `#[cfg(test)] mod tests` inside each view's module (`AGENTS.md`, Testing; `skill://gpui-tokio-client-wiring`), `#[tokio::test]` stacked over `#[gpui_kit::test]`, headless window with the real `Workspace`, an embedded gateway over the channel transport, real omp (`--mode rpc-ui`, `openrouter/inclusionai/ling-3.0-flash`) in a temporary project. Interaction through `TestWindowExt` (clicks, typed text, named keys); assertions on view and model state and on the accessibility tree, never pixels. Explicit imports, never `use gpui_kit::*` or `use super::*`. No `tests/`.
- Flows: open a listed session and page its transcript; new session, prompt, streamed reply ends settled; a tool call renders a card that completes; `!cmd` runs bash; abort mid-turn; steer while running and see the queue update, then remove an entry; change model and thinking; an `ask` tool call opens a dialog and the answer reaches omp, and a second test cancels it; history tree: branch from a user message, navigate back, label a node, fork and see the tab follow the new file; close a tab and confirm its omp is gone.
- Failure states: kill omp and see the exited banner; drop the gateway's channel and see `Lost`; a command rejected by the gateway shows its error code.

## Boundaries

- Always: follow `AGENTS.md` and the gpui-kit coding guides; read the component page before using a component; domain ids for identity.
- Ask first: new dependencies; features outside omp-rpc; a custom `Element`.
- Never: depend on `alvin-gateway` outside dev-dependencies, `alvin-omp-host`, or `alvin-session-index`; parse composer text beyond the leading `!`; create a tokio runtime; keep domain state in a view; assert pixels.

## Open Questions

None. Decided: `alvin-gateway` is a dev-dependency of this crate (`AGENTS.md` rule 7); the out-of-scope list stays out of v1.
