# Spec: app

Module id `app`, crate `alvin-app` (binary). Depends on `conversation-ui`, `client`, `gateway`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

The desktop face. Parses arguments into typed settings, starts telemetry and the tokio runtime, either embeds the gateway and connects over the channel transport or connects to a `ws(s)://` gateway, opens the window with the `alvin-conversation-ui` `Workspace`, and shuts everything down on quit. No protocol, session, or agent logic (`AGENTS.md`, Faces).

Success:

- `mise dev` opens a window on macOS, Windows, and Linux showing the projects from the machine's omp sessions directory.
- `alvin-app --endpoint ws://127.0.0.1:<port>` shows the same UI against a running `alvind`, with no embedded gateway started.
- Quitting leaves no omp process running and flushes telemetry; a flush or shutdown error is logged, not dropped.

## Configuration

`confique` with the clap layer, the pattern tin and crumbs use (`tin-cli/src/config/types.rs`, `crumbs-cli/src/config/{types,load}.rs`).

```rust
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
struct AppConfig {
    /// Connect to a running gateway (`ws://` or `wss://`). Absent: embed one.
    #[config(env = "ALVIN_ENDPOINT")]
    #[config(layer_attr(arg(long)))]
    endpoint: Option<url::Url>,
    #[config(nested)]
    gateway: EmbeddedGateway,
}

/// The embedded gateway; unused with `endpoint`.
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
struct EmbeddedGateway {
    /// omp executable; a bare name is found on `PATH` when spawned.
    #[config(default = "omp", env = "ALVIN_OMP")]
    #[config(layer_attr(arg(long = "omp")))]
    omp: PathBuf,
    /// Default model for new sessions, `provider/id`.
    #[config(env = "ALVIN_MODEL")]
    #[config(layer_attr(arg(long)))]
    model: Option<String>,
    /// omp's sessions directory. Absent: resolved from omp's rules (below).
    #[config(env = "ALVIN_SESSIONS_DIR")]
    #[config(layer_attr(arg(long)))]
    sessions_dir: Option<PathBuf>,
}
```

The clap struct flattens `<AppConfig as confique::Config>::Layer` and adds `--config-file <path>`.

Sources, highest precedence first (confique's builder: earlier sources win):

1. Command-line flags (`.preloaded(cli_layer)`).
2. `ALVIN_*` environment variables (`.env()`).
3. `--config-file <path>`, when given.
4. User files: on macOS `~/.config/alvin/config.toml`, then `dirs::config_dir()/alvin/config.toml` (`~/Library/Application Support/alvin`); elsewhere `dirs::config_dir()/alvin/config.toml` (`$XDG_CONFIG_HOME/alvin` on Linux, `%APPDATA%\alvin` on Windows). The macOS `~/.config` layer on top follows tin's `tin_config_dirs` (`tin-cli/src/config/dirs.rs:19-30`); crumbs orders those two the other way, tin's order is used here.
5. Schema defaults.

Missing files are skipped, as confique and crumbs do. No project-local file: alvin is not scoped to one repository. No secrets file: the app holds no credentials.

Settings provide independent `light_theme` and `dark_theme` selections from the Verdant catalog. Their defaults are
`github::light` and `github::dark`. `appearance` selects `system`, `light`, or `dark`; system mode switches between the
two selections when the window appearance changes. The corresponding command-line flags and environment variables
are `--light-theme` / `ALVIN_LIGHT_THEME`, `--dark-theme` / `ALVIN_DARK_THEME`, and `--appearance` /
`ALVIN_APPEARANCE`. `accent_color` defaults to `module`; `user_message_color` defaults to `operator` and may be
`accent` to follow the accent selection. Both accept the representative Verdant keys; their flags and environment
variables are `--accent-color` / `ALVIN_ACCENT_COLOR` and `--user-message-color` / `ALVIN_USER_MESSAGE_COLOR`.
Settings presents compact, keyboard-accessible color samples instead of the internal keys: accent samples resemble
buttons, message samples resemble bubbles, and a check marks the current selection. Selected samples retain their
hover-strength theme color when the pointer moves away, with a separate keyboard focus indicator. `Use accent` previews
and follows the current accent. Samples resolve from the active Verdant theme and update when appearance or theme changes.
Appearance, theme, and color selections save and apply immediately. Each gateway field saves independently when Enter
is pressed, then reconnects if the effective gateway changed. Unsubmitted edits in other fields are not saved. The
existing source precedence still applies, and save or connection errors appear in settings.
The sidebar's settings control opens Settings; Back to sessions returns to the workspace and restores its focus.

Project aliases, pins, and hidden projects, plus pinned and hidden sessions within each project, are local presentation
preferences. They are stored in `projects.json` beside the writable `config.toml` (or beside the file passed with
`--config-file`) and apply across gateway reconnects. Delete in the project menu hides a project; restoring it does not
change omp files. A missing preferences file starts empty; older entries without session lists start with empty lists.
Malformed or unreadable preferences stop startup; save errors appear in the window and the unsaved change is reverted.

Verdant's `_normal` supplies base foreground and background. Exact `comment`, `markup.list.unchecked`,
`ibl.indent.char.1`, and `ibl.whitespace.char.1` provide muted text, borders, and control surfaces when available;
missing interface shades are derived from base colors. Secondary panes are derived away from the foreground, and
selection blends the muted text into the background at 25%. Accent and user-message colors resolve through Verdant's
`find_style`: exact key, dotted parent, then theme foreground. The user-message foreground chooses the higher-contrast
theme foreground or background, falling back to black or white when needed for 4.5:1 contrast. Links resolve from
`function`, the caret from `markup.link.url`, and status colors from `diff.plus`, `diff.minus`, `comment.warning`,
and `comment.note`. Missing status styles retain GPUI Kit's mode defaults. Mapping is independent of theme name.

After loading, `AppConfig` converts once into `Settings`, an enum: `Remote(Url)` when `endpoint` is set, else `Embedded { omp, model, sessions_dir }` with the sessions directory resolved. The conversion is the only check, and it is syntactic (`AGENTS.md`, Faces): `endpoint` must be `ws` or `wss`; `endpoint` together with `gateway.model` or `gateway.sessions_dir` from any source is an error naming both keys, since they configure a gateway the app does not start. Whether the executable or directory exists, or the model is valid, is decided by the gateway or omp and surfaces as their typed errors.

### omp sessions directory

When `gateway.sessions_dir` is absent, the face resolves it from omp's own rules (`packages/utils/src/dirs.ts`), in this order:

1. `PI_CODING_AGENT_DIR` set: `<it>/sessions` (`dirs.ts:4-5`).
2. Else the config root is `<home>/<PI_CONFIG_DIR or .omp>` (`dirs.ts:4,29,115-118`); with `OMP_PROFILE` (or legacy `PI_PROFILE`) set, the agent dir is `<root>/profiles/<profile>/agent`, else `<root>/agent` (`dirs.ts:84-138`); sessions are `<agent dir>/sessions`.
3. On Linux, `XDG_DATA_HOME` set and `$XDG_DATA_HOME/omp` existing: `$XDG_DATA_HOME/omp/sessions` (`dirs.ts:430-435,998-999`, as in `SPEC-session-index.md`).

`<home>` is `dirs::home_dir()`; none is a startup error. The rule duplicates omp's, so `sessions_dir` overrides it; the first slice confirms the resolved path against real omp sessions on each OS, including which wins when both `PI_CODING_AGENT_DIR` and `XDG_DATA_HOME` are set.

## Startup

`main.rs` is thin; `app.rs` holds the wiring.

1. Load the configuration (below); a parse or load error exits with its message.
2. Telemetry: `alvin_gateway::telemetry` with service name `alvin-app` and the crate version (`SPEC-gateway.md`, Telemetry). First, before runtime threads start.
3. `#[tokio::main]` supplies the runtime (`skill://gpui-tokio-client-wiring`). No other runtime is created.
4. Embedded: build `GatewayConfig` from the settings, start the gateway, take its channel pair (`Gateway::connect_channel`, `SPEC-client.md`), and connect `Client::connect(Endpoint::Channel(pair))`. Remote: `Client::connect(Endpoint::WebSocket(url))`.
5. `gpui_kit::application().with_assets(..).run(..)`: register GPUI Kit's default assets plus the selected `GitFork` and `Smartphone` icons, then `gpui_kit::init(cx)`, `alvin_conversation_ui::init(cx)` (actions and key bindings), and open the window with gpui-kit's `Root` over `Workspace::new(client, ..)`.

The face supplies the local user name to the workspace from `USER` on Unix or `USERNAME` on Windows. A missing or
blank value leaves the neutral “Local user” identity; a non-Unicode value is reported as a startup error. Gateway
reconnection carries the displayed identity to the replacement workspace. This is a local display name, not an
authenticated account.

On macOS GPUI Kit `TitleBar::window_options()` provides transparent full-size content and native traffic lights; the
sidebar header supplies the matching `TitleBar`. Other platforms retain their default native window decorations.

On macOS, `mise run package` builds `target/debug/Alvin.app` and generates its `Alvin.icns` from
`assets/chipmunk-mascot.webp`. `mise dev` runs the bundled executable so macOS can use the bundle icon;
running the raw `target/debug/alvin-app` binary does not provide bundle icon metadata.

Errors before the window opens are `eyre` reports at the edge, printed and exiting non-zero. After the window opens, errors are the UI's (`SPEC-conversation-ui.md`).

## Shutdown

Quitting never relies on GPUI's quit hook alone: it runs for 200 ms (`SHUTDOWN_TIMEOUT`), and closing omp sessions takes longer, so the processes would outlive the app. The `Quit` action (bound to the platform's command key plus `q`) and the last window closing therefore shut the embedded gateway down in full, then call `cx.quit()`. The `on_app_quit` hook still starts a shutdown for quits that bypass both. A second request while one runs does nothing.

On quit (`Context::on_app_quit`, gpui-pre 0.3.10, read on docs.rs; bounded by GPUI's `SHUTDOWN_TIMEOUT`): embedded mode shuts the gateway down, which closes every live session and its omp process tree (`SPEC-gateway.md`, `SPEC-omp-host.md`); then telemetry flushes and shuts down explicitly, logging any error. Remote mode closes the client only; the remote gateway's sessions stay live.

## Out of Scope (v1)

- A web face. `alvin-protocol`, `alvin-client`, and `alvin-conversation-ui` build for wasm32 so it can be added; a browser entry point (`wasm-bindgen`, served bundle, fonts, per gpui-kit's WebAssembly docs) is a later module.
- Signing, installers, and distribution tasks.

## Dependencies

`alvin-conversation-ui`, `alvin-client`, `alvin-gateway`, `gpui-kit`, `tokio` (`rt-multi-thread`, `macros`), `clap` (`derive`, `env`; approved for the gateway), `confique` (`toml`), `dirs`, `url` (`serde`), `eyre`, `tracing`, `alvin-workspace-hack`. Dev: `gpui-kit` with `test-support`. Every dependency at latest stable when added.

## Project Structure

```
crates/alvin-app/src/main.rs       thin: settings, telemetry, run
crates/alvin-app/src/config.rs     confique schema, layer loading, Settings conversion, sessions-directory resolution
crates/alvin-app/src/app.rs        gateway or remote client, window, quit hook
.mise/tasks/dev                    run the desktop app; uses the macOS bundle on macOS
.mise/tasks/package                build the macOS application bundle
```

## Testing Strategy

In-module only (`AGENTS.md`, Testing).

- `config.rs`: plain `#[test]`, as tin does (`tin-cli/src/config/tests.rs`): load the schema from TOML files in a temp directory with `AppConfig::builder().file(..)`; precedence (CLI layer over file, the `--config-file` over the user files, `~/.config/alvin` over the platform directory on macOS); an unknown key is a load error; `endpoint` with `[gateway] model` or `sessions_dir` is an error; a non-`ws(s)` endpoint is an error; schema defaults resolve with zero sources. The sessions-directory resolver takes the environment values as arguments, so each rule is tested without setting process environment. confique's own env source is not re-tested.
- `app.rs`: one `#[tokio::test]` over `#[gpui_kit::test]` that runs the real startup in embedded mode against real omp (`--mode rpc-ui`, `openrouter/inclusionai/ling-3.0-flash`) in a temporary project and sessions directory, opens the window, asserts the project appears in the sidebar, opens a live session, then runs the quit path and asserts no omp process remains. View flows are `alvin-conversation-ui`'s tests, not repeated here.
- Remote mode is covered by the gateway's acceptance tests over `ws://` and the client's tests; here only its settings path is tested.

## Boundaries

- Always: follow `AGENTS.md`; wiring only.
- Ask first: new dependencies; new config keys beyond the schema above; new subcommands; packaging.
- Never: protocol, session, or agent logic; reading omp files; a second runtime; validating settings beyond parsing.

## Open Questions

None. Approved: `confique` (`toml`), `dirs`, `url` (`serde`), each at latest stable when added; sessions directory resolved from omp's rules with `gateway.sessions_dir` as override.
