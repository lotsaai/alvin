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
- [ ] Task 6: `alvind` face with telemetry and `wss://`

### Checkpoint 2a: Read path
- [ ] Projects and sessions over channel, ws, and wss; pinned list matches; human reviews protocol shapes

## Phase 3: Live sessions (CLI-proven)
- [ ] Task 7: omp-host core: start, events, prompt, abort, bash, teardown
- [ ] Task 8: Live turn end to end: open, prompt, subscribe, close
- [ ] Task 9: Client reconnect and `Resync`
- [ ] Task 10: Windows Job Object teardown (verified on the Windows VM via `tasks/briefs/task-10-windows.md`; may stay blocked while later tasks continue)

### Checkpoint 2b: CLI end to end
- [ ] Full turn via `alvind` over ws with reconnect; human review before UI

## Phase 4: UI walking skeleton
- [ ] Task 11: Conversation fold core plus recorded fixtures
- [ ] Task 12: UI browse and chat: sidebar, open, transcript, composer
- [ ] Task 13: `alvin-app` face: config, embedded/remote, quit

### Checkpoint 3: Usable app
- [ ] Real turn in `mise dev` on macOS; human UX review

## Phase 5: Feature slices
- [ ] Task 14: Queue: steer, follow-up, and queue edits
- [ ] Task 15: Model and thinking pickers
- [ ] Task 16: Extension dialogs
- [ ] Task 17: History tree: read and render
- [ ] Task 18: History tree actions
- [ ] Task 19: Session operations and command catalog
- [ ] Task 20: Failure states in the UI
- [ ] Task 21: Built-in tool renderers

### Checkpoint: Complete
- [ ] Every method has a UI path and every event has a visible effect; all checks clean; Windows verified; human sign-off
