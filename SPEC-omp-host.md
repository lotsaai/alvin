# Spec: omp-host

Module id `omp-host`, crate `alvin-omp-host`. Depends on `protocol`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

Own the omp processes. One `omp --mode rpc-ui` process per live session, started, supervised, and torn down here. The only crate that depends on `omp-rpc` or spawns omp.

Success:

- Starting a host session spawns one omp process in a given working directory and waits for its ready frame.
- Every omp command the protocol exposes has a typed async method on the session handle.
- omp frames arrive as a bounded async stream of protocol event bodies, in order.
- Closing a session, dropping its handle, or shutting the host leaves no omp process or child of omp behind, on macOS, Linux, and Windows.
- An omp that exits on its own surfaces as an event and a typed error on later calls, never a hang.

## Inputs and Outputs

Constructor input (typed, from the gateway's config): the omp executable path and the extra launch settings omp needs. No lookups, no environment reads here.

`Host::start(request) -> Session`. `StartRequest` (typed-builder): working directory, what to open (new session, resume a session file, or fork), optional model. A `Session` handle owns the process and exposes async methods named after the protocol's live commands, plus `events()` returning a stream of event bodies. Event bodies carry no ids; the gateway stamps each with a fresh UUIDv7 and the live session id. The host does not know about `EventId` or `LiveSessionId`.

## omp-rpc Facts

Read from omp-rpc source and README:

- The client is blocking. A reader thread decodes stdout, routes responses by request id, runs prompt collectors, and delivers every other frame as an `Event` over a channel. A writer thread owns stdin.
- `Client::spawn(std::process::Command, ClientOptions) -> Result<(Client, Receiver<Event>), Error>` sets the process group on Unix, pipes stdin and stdout, leaves stderr inherited, and returns only after the ready frame and protocol negotiation (`client.rs:750-765`). `Client::from_io` takes existing pipes instead.
- On Unix the child leads its own process group; close and drop send SIGTERM to the group, then SIGKILL after one second. On Windows only the leader is killed, so grandchildren survive.
- Errors: `Io`, `Json`, `Command { command, error, code }`, `Timeout`, `Closed`, `Protocol`, `InvalidArgument`.
- One omp process holds one active session. `open_session` and `switch_session` move it to another session file.

## Bridging

- Calls run on `spawn_blocking`; a call never blocks a runtime worker.
- A dedicated thread per session drains the omp event receiver into a bounded `tokio::sync::mpsc` channel after translating frames to event bodies. When the channel is full the thread blocks, which backpressures omp's stdout; frames are never dropped here.
- Dropping a `Session` closes the client and joins its threads. The drop path must not panic and must not block the runtime; it hands the blocking close to a blocking pool.

## Windows Teardown

On Windows the host places omp in a Job Object configured to kill all members when the last handle closes, so omp's children die with it. The Job Object code lives in one small module with an unsafe boundary documented by `# Safety` and `// SAFETY:` comments, behind a safe wrapper. Unix keeps omp-rpc's process-group teardown.

`Client::spawn` starts the child and waits for `ready` before returning (`client.rs:750-765`), so a job assigned after it returns would miss children omp starts during extension startup. On Windows the host therefore spawns omp itself, suspended, assigns it to the job, resumes it, and hands its pipes to `Client::from_io` (`client.rs:778-788`), which takes existing pipes and owns no child. The job handle owns teardown: closing it kills omp and every child. `from_io`'s reader thread exits once omp's stdout closes, which the job kill causes. No omp-rpc change is needed.

## Event Translation

`translate` is a pure function from `omp_rpc::Event` to either a protocol `EventBody` or a typed `Untranslated` value carrying the raw frame, for kinds the protocol does not expose (python, live voice, dictation, btw, subagent frames). Untranslated frames are counted and logged with `tracing`, not silently dropped and not forwarded. Frames that failed to decode (`Event::Undecodable`, `RpcFrameError`) become an event body naming the failure.

`get_tree` and `get_entries` results arrive as untyped maps (omp's schema leaves them open). The session handle parses them into the protocol's `HistoryTree` and `Entry` (`SPEC-protocol.md`, History Tree) where it reads them; an entry without `id`, `parentId`, `timestamp`, or `type` is a typed error carrying the entry's raw JSON and the live session's working directory.

## Dependencies

`alvin-protocol`, `omp-rpc` (git dependency, `tag = "v<omp release>"`, the version pinned in the root `package.json`), `tokio` (rt, sync, process off), `tracing`, `thiserror`, `typed-builder`, `alvin-workspace-hack`. Windows: `windows-sys` with the Job Object features, behind `cfg(windows)` (approved).

## Errors

One `Error` in `error.rs`. `omp_rpc::Error` is the `#[source]` of its variants. Variants carry the working directory, session file, or command name: spawn failed, ready timeout, command failed (with omp's `code`), process exited, close could not confirm teardown, event channel closed. The gateway maps them to protocol error codes.

## Project Structure

```
crates/alvin-omp-host/src/lib.rs        thin
crates/alvin-omp-host/src/error.rs
crates/alvin-omp-host/src/host.rs       Host, StartRequest
crates/alvin-omp-host/src/session.rs    Session handle and command methods
crates/alvin-omp-host/src/bridge.rs     blocking-to-async event pump
crates/alvin-omp-host/src/translate.rs  frames to event bodies
crates/alvin-omp-host/src/job.rs        Windows Job Object, cfg(windows)
```

## Testing Strategy

- Real omp: tests start `omp --mode rpc-ui` with model `zai/glm-5.3-flash` and use omp's existing configuration; they never read credentials.
- Dialogs: a prompt that makes omp call its `ask` tool produces one `ask` extension UI request, and the answer reaches omp.
- A real prompt round trip streams text deltas and ends with a prompt result, in order.
- Abort stops a running turn; `bash` runs a command and returns its output.
- Resume: open a session file written by an earlier test run and read its history.
- Teardown: after close and after drop, the process id is gone; a long-running `bash` child of omp is gone too. On Windows this exercises the Job Object.
- Process death: killing omp from the test yields a process-exited event and a typed error on the next call.
- Backpressure: a slow consumer does not lose events.
- Translation: unit tests over frames recorded from real omp runs, stored as fixtures in the repository.
- The host's omp binary version is compared with the pinned release by `mise check`, not by tests.

## Boundaries

- Always: follow `AGENTS.md`; one process per session; no frame dropped without an event.
- Ask first: new dependencies; passing extra flags to omp.
- Never: depend on or call `alvin-session-index` or `alvin-gateway`; write session files; hand-edit or fork `omp-rpc`; mint ids; add auth.

## Launch

Read from omp's `docs/rpc.md` and `docs/cli-reference.md`:

- Command: `<omp> --mode rpc-ui --cwd <dir> [--model <provider/id>] [--session-dir <dir>]` (`docs/rpc.md:28`, `docs/rpc.md:1293-1301`, `docs/cli-reference.md:73-104,254`). Plain `--mode rpc` emits no UI requests except a host-issued `login`, so the `ask` tool and extension dialogs would never reach the UI. `rpc-ui` is the same transport with UI requests enabled; omp-rpc speaks it unchanged. `rpc-ui` also sets `PI_NO_PTY=1`. `--cwd` sets the working directory the session records. Resume uses `switch_session { sessionPath }` on a fresh process, or `open_session { sessionDir }` to adopt the newest session in a directory; both accept `provider` and `modelId` together to bind a model. A resumed session restores its saved model; if that model cannot be restored, the command fails with `Could not restore model <provider/id>` and the previous session stays active. That failure maps to a typed error, not a retry.
- After `ready`, the host sends `set_ask_dialog { enabled: true }` (`docs/rpc.md:726-737`), so each `ask` tool call arrives as one `ask` request carrying all its questions instead of one `select` per choice. omp's docs say builds without the command answer with a failed response; omp is pinned to one release, so a failure is a typed startup error, not a fallback to `select`.
- Never pass `--no-session`: it disables `open_session` and persistence, and alvin reads omp's session files.
- omp sets `PI_NO_TITLE=1` itself in rpc modes, so no title is generated. Titles come from `live/rename` and from the file.
- `session_settled` and `prompt_result.sessionSettled` mean nothing can wake the session again, so the gateway may close the process then; the host reports the event and leaves the policy to the gateway.
- Environment: the child inherits the host's environment unchanged; the host sets nothing.
- `--no-ui` is not passed, so extension dialogs and presentation updates (`select`, `confirm`, `input`, `editor`, `notify`, `setStatus`, `setWidget`, `set_editor_text`) reach the UI as extension UI requests. Tool approval prompts go through the extension runner and also reach the UI. `setTitle` stays suppressed unless `PI_RPC_EMIT_TITLE=1`; the host does not set it.

## Windows Binding

`windows-sys` with the Job Object features, behind `cfg(windows)` (approved).

## Open Questions

None.
