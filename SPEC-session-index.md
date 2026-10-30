# Spec: session-index

Module id `session-index`, crate `alvin-session-index`. Depends on `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

Read-only view of omp's session files for listing, grouped by project: headers and a tail window for status, never full transcripts (those come from a live omp after `live/open`). The only crate that reads session files and the only consumer of `code-workspace`. It never writes, renames, or deletes anything under omp's directories.

Success:

- Given omp's sessions directory, lists projects: one per repository, with checkouts (the main working copy, git worktrees, jj workspaces) as sub-groups, plus an unresolved group per `cwd` that no longer resolves.
- Lists a checkout's sessions newest first, paged, each with the fields the UI shows.
- Handles a directory of hundreds of sessions without reading whole files.

## Inputs and Outputs

Constructor input: omp's sessions directory (a `PathBuf`, from the gateway's typed config, resolved by the face as described below). No default lookup, no environment reads here.

Outputs are `alvin-protocol` types: `ProjectSummary { id, root, checkouts }` or the unresolved group, `CheckoutSummary { id, root, vcs }`, `SessionSummary { id, path, title, created, modified, status, parent }`. `created` comes from the header timestamp, `modified` from the file's mtime, as omp does. Project and checkout ids are blake3 hashes of the canonical root. Paging uses the session's modified time and id as cursor, not offsets.

## omp File Facts

Read from omp source, to be confirmed against real files in the first slice:

- Root: omp's sessions directory is `<agent dir>/sessions`, except that with `XDG_DATA_HOME` set and `$XDG_DATA_HOME/omp` existing, it is `$XDG_DATA_HOME/omp/sessions` (`packages/utils/src/dirs.ts:430-435,998-999`). The face resolves it; the index takes the resolved directory.
- Layout: `<sessions dir>/<encoded cwd>/<timestamp>_<id>.jsonl`, timestamp in file-safe ISO form such as `2026-10-26T18-29-27-740Z` (`session-listing.ts:838-849`). The directory name encoding is not used; every file's header carries the real `cwd`.
- Header: `type: "session"` with `id`, `timestamp`, `cwd`, optional `title`, `titleSource`, `parentSession`, `continuationOf`, `version`, `additionalDirectories` (`session-entries.ts`, `SessionHeader`). The file may begin with a fixed-width title slot record (`type: "title"`) before the header; its title overrides the header's, and an empty slot title means no title (`session-listing.ts:328-391`).
- Display title, as omp computes it: slot or header title, else the latest compaction `shortSummary`, else the first user message, else untitled (`session-listing.ts`, `scanSessionFile` and `sessionDisplayName`).
- Status: omp reads the first 4 KiB for header, first message, and counts, and the last 32 KiB to derive status (complete, interrupted, aborted, error, pending, unknown) (`session-listing.ts:71-77,408-513`). The reader does the same; a final message larger than the tail window yields `unknown`. omp's message and turn counts come from those windows and are not exact, so the summary carries no counts.
- Only `*.jsonl` files are read; backups (`*.jsonl.<id>.bak`) and other files are skipped. Sessions with zero turns and no title are omitted, as omp's picker does (`isEmptySession`, `filterSessionsForPicker`).
- Session files grow while omp runs. Reads tolerate a torn last line.

## Grouping

For each distinct `cwd` in headers, call `Workspace::discover(cwd)` (code-workspace 0.2.0, read from the published source). `Workspace::repository_root()` returns the primary repository for a linked git worktree (`.git` file, `gitdir:` then `commondir`) or a secondary jj workspace (`.jj/repo` pointer); `Workspace::root()` is the checkout itself; `Workspace::vcs()` reports git, jj, or colocated jj. Sessions group by (repository root, checkout root). A `cwd` is resolved once per scan and cached by path.

Unresolved sessions: when `discover` fails (the `cwd` is gone, or a worktree whose main repository was deleted, which 0.2.0 reports as an error), the sessions go into an explicit unresolved group keyed by the recorded `cwd`, carrying the typed discovery error. This is a domain state shown to the user, not a fallback; the protocol's `ProjectSummary` has a variant for it.

## Dependencies

`alvin-protocol`, `code-workspace`, `serde_json`, `jiff`, `blake3` (approved; ids), `tokio` (fs, `spawn_blocking`) or `std::fs` on a blocking pool as the gateway runs it, `thiserror`, `alvin-workspace-hack`. Latest stable of each.

## Errors

One `Error` in `error.rs`. Variants carry the path and phase: reading the directory, opening a file, decoding a record (with byte offset), resolving a workspace for a `cwd`. A malformed file is an error for that file, reported with its path; the scan does not repair or skip silently. The gateway decides what a caller sees.

## Project Structure

```
crates/alvin-session-index/src/lib.rs        thin
crates/alvin-session-index/src/error.rs
crates/alvin-session-index/src/scan.rs       directory walk, header and tail reads
crates/alvin-session-index/src/header.rs     typed header and title slot parsing
crates/alvin-session-index/src/status.rs     tail-derived status
crates/alvin-session-index/src/grouping.rs   projects and checkouts from cwds
```

## Testing Strategy

- Real omp output: tests start real `omp` (model `zai/glm-5.3-flash`) in a temporary project to write session files, then index them. Temporary directories are removed by the test.
- Real layouts: tests create a real git repository with worktrees and a real jj repository with workspaces using the actual `git` and `jj` binaries, then assert the grouping.
- Torn writes: a test appends a partial record to a real session file and reads it.
- Large files: a test writes a multi-megabyte real session file and asserts the scan finishes with bounded memory and reads only the head and tail windows, measured through the file handle's reported read offsets.
- Cursors: paging over many real files returns each session exactly once in order.

## Boundaries

- Always: open files read-only; follow `AGENTS.md`.
- Ask first: new dependencies; any caching that persists to disk.
- Never: write under omp directories, repair or default missing omp fields, depend on `alvin-omp-host` or `alvin-gateway`, call omp.

## Open Questions

None.
