# alvin v1 — Task List

Plan, acceptance criteria, and risks: `tasks/plan.md`. Every task ends with `mise format` (read each step) and `mise run --force test`.

## Phase 1: Foundation
- [x] Task 1: Scaffold workspace and prune heyday tooling
- [x] Task 2: Generate omp wire types into `alvin-protocol` (risk spike)

### Checkpoint 1: Foundation
- [x] format/test/check clean; spike findings recorded; human approves generated types

## Phase 2: Read-only path
- [x] Task 3: Scan session headers and group projects
- [x] Task 4: `projects/list` end to end over channel and `ws://`
- [x] Task 5: `sessions/list` with status and paging
- [x] Task 6: `alvind` face with telemetry

### Checkpoint 2a: Read path
- [x] Projects and sessions over channel and ws; pinned list matches; human reviews protocol shapes

## Phase 3: Live sessions (CLI-proven)
- [x] Task 7: omp-host core: start, events, prompt, abort, bash, teardown
- [x] Task 8: Live turn end to end: open, prompt, subscribe, close
- [x] Task 9: Client reconnect and `Resync`
- [ ] Task 10: Windows Job Object teardown (verified on the Windows VM via `tasks/briefs/task-10-windows.md`; may stay blocked while later tasks continue)

### Checkpoint 2b: CLI end to end
- [x] Full turn via `alvind` over ws with reconnect (automated; human review waived: build auto run to the final checkpoint)

## Phase 4: UI walking skeleton
- [x] Task 11: Conversation fold core plus recorded fixtures
- [x] Task 12: UI browse and chat: sidebar, open, transcript, composer (wasm32 build not met: needs nightly, see plan)
- [x] Task 13: `alvin-app` face: config, embedded/remote, quit (real window launch not verifiable in the sandbox, see plan)

### Checkpoint 3: Usable app
- [ ] Real turn in `mise dev` on macOS; human UX review

## Phase 5: Feature slices
- [x] Task 14: Queue: steer, follow-up, and queue edits
- [x] Task 15: Model and thinking pickers
- [x] Task 16: Extension dialogs
- [x] Task 17: History tree: read and render
- [x] Task 18: History tree actions
- [x] Task 19: Session operations and command catalog
- [x] Task 20: Failure states in the UI
- [ ] Task 21: Built-in tool renderers

### Checkpoint: Complete
- [ ] Every method has a UI path and every event has a visible effect; all checks clean; Windows verified; human sign-off
