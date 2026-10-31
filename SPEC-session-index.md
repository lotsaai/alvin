# Spec: session-index

Module id `session-index`, crate `alvin-session-index`. Depends on `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

Read-only view of omp's session files for listing, grouped by project: headers and a tail window for status, never full transcripts (those come from a live omp after `live/open`). The only crate that reads session files and the only consumer of `code-workspace`. It never writes, renames, or deletes anything under omp's directories.

Success:

- Given omp's sessions directory, lists projects: one per repository, checkouts (the main working copy, git worktrees, jj workspaces) as sub-groups. Sessions whose `cwd` no longer resolves are left out.
- Lists a checkout's sessions newest first, paged, each with the fields the UI shows.
- Handles a directory of hundreds of sessions without reading whole files.

## Inputs and Outputs

Constructor input: omp's sessions directory (a `PathBuf`, from the gateway's typed config, resolved by the face as described below). No default lookup, no environment reads here.

Outputs are `alvin-protocol` types: `ProjectSummary { id, root, checkouts }`, `CheckoutSummary { id, root, vcs }`, `SessionSummary { id, path, title, created, modified, status, parent }`. `created` comes from the header timestamp, `modified` from the file's mtime, as omp does. Project and checkout ids are blake3 hashes of the canonical root. Paging uses the session's modified time and id as cursor, not offsets.

## omp File Facts

Read from omp source, to be confirmed against real files in the first slice:

- Root: omp's sessions directory is `<agent dir>/sessions`, except that with `XDG_DATA_HOME` set and `$XDG_DATA_HOME/omp` existing, it is `$XDG_DATA_HOME/omp/sessions` (`packages/utils/src/dirs.ts:430-435,998-999`). The face resolves it; the index takes the resolved directory.
- Layout: `<sessions dir>/<encoded cwd>/<timestamp>_<id>.jsonl`, timestamp in file-safe ISO form such as `2026-10-26T18-29-27-740Z` (`session-listing.ts:838-849`). The directory name encoding is not used; every file's header carries the real `cwd`.
- Header: `type: "session"` with `id`, `timestamp`, `cwd`, optional `title`, `titleSource`, `parentSession`, `continuationOf`, `version`, `additionalDirectories` (`session-entries.ts`, `SessionHeader`). The file may begin with a fixed-width title slot record (`type: "title"`) before the header; its title overrides the header's, and an empty slot title means no title (`session-listing.ts:328-391`).
- Display title, as omp computes it: slot or header title, else the latest compaction `shortSummary`, else the first user message, else untitled (`session-listing.ts`, `scanSessionFile` and `sessionDisplayName`).
- Status: omp reads the first 4 KiB for header, first message, and counts, and the last 32 KiB to derive status (complete, interrupted, aborted, error, pending, unknown) (`session-listing.ts:71-77,408-513`). The reader does the same; a final message larger than the tail window yields `unknown`. omp's message and turn counts come from those windows and are not exact, so the summary carries no counts.
- Only `*.jsonl` files are read; backups (`*.jsonl.<id>.bak`) and other files are skipped. Sessions with zero turns and no title are omitted, as omp's picker does (`isEmptySession`, `filterSessionsForPicker`).
- Session files grow while omp runs. Reads tolerate a torn last line.

Confirmed against real files (omp 18.14.1, written by `omp -p` into a temporary `--session-dir`):

- The file starts with the title slot (`{"type":"title","v":1,"title":"","updatedAt":…,"pad":"   …"}`, about 300 bytes with padding), then the header (`type:"session"`, `version:3`, `id`, `timestamp`, `cwd`); a fresh session has an empty slot title, so the title is `None`. omp writes the header with no `title` field.
- The header `cwd` is the spelling the OS gave omp, not a canonical path: a temp dir created as `/private/var/...` was recorded as `/var/...`. Grouping therefore canonicalizes the checkout root before hashing.
- Header ids are UUIDv7 strings today; the index still accepts any string.
- `code-workspace` 0.2.0 returns a plain (non-VCS) checkout root as given, not canonicalized, and reports the nearest existing ancestor as `NoWorkspaceMarker.start` when the `cwd` was deleted.

## Test Isolation

Tests that start omp set `HOME` to a throwaway directory, pass `--session-dir` to a temporary directory, and set `PI_CODING_AGENT_DIR` to the machine's existing agent directory so model credentials resolve. omp then touches nothing under the real home: with the real `HOME` it stats `~/.aws/credentials` during provider discovery, which a sandbox may deny. Tests never open, read, or print files in the agent directory.

## Grouping

For each distinct `cwd` in headers, call `Workspace::discover(cwd)` (code-workspace 0.2.0, read from the published source). `Workspace::repository_root()` returns the primary repository for a linked git worktree (`.git` file, `gitdir:` then `commondir`) or a secondary jj workspace (`.jj/repo` pointer); `Workspace::root()` is the checkout itself; `Workspace::vcs()` reports git, jj, or colocated jj. Sessions group by (repository root, checkout root). A `cwd` is resolved once per scan and cached by path.

Unresolved sessions: when `discover` fails (the `cwd` gone, or a worktree whose main repository was deleted, which 0.2.0 reports as an error), the sessions belong to no project and are not listed. The project is gone, so there is nothing to open or resume it in.

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

- Real omp output: tests start real `omp` (model `openrouter/inclusionai/ling-3.0-flash`) in a temporary project to write session files, then index them. Temporary directories are removed by the test.
- Real layouts: tests create a real git repository with worktrees and a real jj repository with workspaces using the actual `git` and `jj` binaries, then assert the grouping.
- Torn writes: a test appends a partial record to a real session file and reads it.
- Large files: a test writes a multi-megabyte real session file and asserts the scan finishes with bounded memory and reads only the head and tail windows, measured through the file handle's reported read offsets.
- Cursors: paging over many real files returns each session exactly once in order.

## Listing Rules (as built)

- `select(heads, project, checkout)` keeps the sessions whose `cwd` resolves to that repository (and checkout); empty means the project or checkout does not exist. Sessions whose `cwd` does not resolve are skipped.
- `list_page(heads, after, limit)` sorts by (modified, id) newest first, reads the 32 KiB tail only of files it reaches, and skips sessions by omp's `isEmptySession` rule: no title, status `Pending` or `Unknown`, and no assistant turn. An assistant turn is looked for in the head window, the tail window, and, only when both miss, the whole file (omp's `hasAssistantTurn` does the same full scan), so a session's listing can read past the windows only when it would otherwise be hidden.
- Display title, as omp computes it: slot or header title, else the latest compaction `shortSummary` in the head window, else the first user message in the head window (a message cut off by the window still names the session), each reduced to its first line.
## Boundaries

- Always: open files read-only; follow `AGENTS.md`.
- Ask first: new dependencies; any caching that persists to disk.
- Never: write under omp directories, repair or default missing omp fields, depend on `alvin-omp-host` or `alvin-gateway`, call omp.

## Open Questions

None.
