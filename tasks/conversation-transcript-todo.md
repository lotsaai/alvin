# Conversation transcript checklist

Status: approved for execution by the user on 2026-11-05. Track verified work against the [plan](conversation-transcript-plan.md).

The linked plan owns acceptance criteria, dependencies, affected modules and verification requirements. Check a task only when its behavioral tests and required native evidence exist. This list does not replace or close work in `tasks/todo.md`.

Approved acceptance adjustments: interface zoom is excluded; existing behavioral coverage is accepted instead of fresh provider-specific diff and automatic-clear captures. Verify both native motion states and restore the original macOS preference.

## Record current evidence

- [x] T0. Record current revision, native baseline and settings.
- [x] T0. Reuse real tool/task/async-result fixtures and recorded multi-target, todo, notice, stream and media scenarios under the approved behavioral coverage.
- [x] T0. Associate each reference requirement with its captured scenario and visible acceptance outcome.

## Fix thinking and establish composition

- [x] T1. Left-align the actual thinking trigger label and its expanded content.
- [x] T1. Render thinking as muted Markdown, initially collapsed, with independent reversible disclosure.
- [x] T2. Replace flattened tool presentations with typed structured bodies and migrate every caller.
- [x] T2. Preserve ScriptPanel, Diff, full copy, raw access, status and retained state.
- [x] T2a. Open potentially mutating tools by default and collapse known read-only tools by default.
- [x] T2a. Add persisted General > Conversation defaults for mutating tools, read-only tools and thinking.
- [x] T2a. Preserve explicit per-block choices through streaming and settings changes, keeping source/output extents independent.
- [x] T3. Compose the continuous marker/content/metadata lanes without overlap or reading-width regression.
- [x] T3. Establish theme-aware text hierarchy, path accents, surfaces and status distinctions.
- [x] Gate A. Review real thinking, disclosure defaults/settings and timeline renders in light/dark, narrow/normal/wide layouts.

## Render event-specific content

- [x] T4. Render lightweight resource rows and structured search results.
- [x] T4. Render upstream multi-target and compatible adjacent reads with independent details and no data loss.
- [x] T5. Render task progress as structured agent rows.
- [x] T5. Render async-result lifecycle cards from the existing captured job data.
- [x] T6. Preserve and reconcile typed todo state, updates, reminders and auto-clear.
- [x] T6. Render phases, progress, completed strikethrough and blocked reasons without duplicate plans.
- [x] T7. Present internal messages as notices with source identity, complete content and live/history reconciliation; preserve omp IRC support without making its producer a generic acceptance prerequisite.
- [x] T7. Render notices, warnings, hooks and errors with provenance, proper text formatting and complete details.
- [x] Apply compact defaults, severity/provenance, retained disclosure and full inspection through one shared System activity category, including unknown members and hidden visibility.
- [x] Prevent startup/activity payloads from covering the conversation: no routine informational popups, one collapsed session summary and bounded retained details.
- [x] Gate B. Review the lightweight and contained representations together in a mixed conversation.

## Complete the integrated experience

- [x] T8. Render known streamed tool updates before completion without duplicate final output.
- [x] T8. Place message, execution and session metadata on their actual owners.
- [x] T8. Add reversible manual process folding that leaves the final answer visible.
- [x] T8. Preserve supported message/tool actions, native menus, keyboard access and focus.
- [x] T9. Verify real inline media where the data supplies it, not a byte-description substitute.
- [x] T9. Exercise the accepted mixed recorded/live coverage across layouts, themes and both native motion states; interface zoom is excluded.
- [x] T9. Verify large/streaming content, virtualization and retained state against the baseline workload.
- [x] T9. Update the feature/design documentation to the exercised result.
- [x] Gate C. Review current revision screenshots and behavior evidence against the explicitly approved acceptance scope.

## Required gate for each completed Rust slice

- [x] Run `mise format` and read the complete findings.
- [x] Run `mise run --force test` with no targeted filters, added sleeps, test deadlines or ignored failures.
- [x] Run the real native path as a managed nonblocking application/service and capture fresh evidence.

Repeat this verification gate for each slice. Never infer completion from an earlier slice's result.

## Completion

Implementation and acceptance are complete under the approved adjustments. Both native motion states were exercised, the original macOS preference was restored, and isolated verification processes were stopped. Final Rust gate: 367 passed, zero skipped; format/build passed. No commit or publication was performed.
