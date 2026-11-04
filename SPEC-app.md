# Spec: app

Module id `app`, crate `alvin-app` (binary). Depends on `conversation-ui`, `client`, and `telemetry`. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

The desktop face parses client settings, starts telemetry and the tokio runtime, connects to a separately running `alvind` over WebSocket, and opens the `alvin-conversation-ui` workspace. It never embeds the gateway or starts omp. No protocol, session, or agent logic lives here.

Success:

- `mise dev` connects to `ws://127.0.0.1:7410` by default and shows the server's projects and sessions.
- `alvin-app --endpoint ws://host:port` connects to another gateway. `wss://` is supported by the client.
- Connection failure is an error, never an embedded fallback.
- Quitting closes the desktop connection and requests closure of its retained terminals. Server agent sessions stay live.
- Both faces use `alvin-telemetry` and explicitly flush providers before exit.

## Configuration

`confique` with the clap layer, the pattern tin and crumbs use (`tin-cli/src/config/types.rs`, `crumbs-cli/src/config/{types,load}.rs`).

```rust
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
struct AppConfig {
    /// Running gateway. The desktop never starts one.
    #[config(default = "ws://127.0.0.1:7410", env = "ALVIN_ENDPOINT")]
    #[config(layer_attr(arg(long)))]
    endpoint: url::Url,
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
`accent` to follow the accent selection. Both accept representative Verdant keys or custom `#RRGGBB` /
`#RRGGBBAA` colors; their flags and environment variables are `--accent-color` / `ALVIN_ACCENT_COLOR` and
`--user-message-color` / `ALVIN_USER_MESSAGE_COLOR`. Settings uses GPUI Kit's native `ColorSelect` fields,
which open its color picker with palettes, hex entry, and HSLA controls. Theme colors remain available as
featured samples. `Use accent` makes user messages follow the accent selection. Theme-derived selections
update when appearance or theme changes; custom colors keep their saved values.
Appearance, theme, and color selections save and apply immediately. In Hosts, pressing Enter saves the endpoint
and reconnects if the effective endpoint changed. The current workspace remains usable until the replacement
connection succeeds. Source precedence still applies, and save or connection errors appear in settings.
The sidebar's settings control opens Settings; Back to sessions returns to the workspace and restores its focus.

Terminal font family and logical-pixel size are independent settings: `terminal_font_family` and `terminal_font_size`
in TOML, `ALVIN_TERMINAL_FONT_FAMILY` and `ALVIN_TERMINAL_FONT_SIZE`, or `--terminal-font-family` and
`--terminal-font-size`. The family picker lists installed families; press Enter to save the size. An explicit family
replaces Ghostty's entire font-family list without resolving or warning about it; other Ghostty appearance settings
still apply. Use “Use inherited font” to clear the saved family override and restore inheritance, subject to source
precedence. With no override, fonts inherit from local Ghostty configuration or the app's monospace defaults. Imported
Ghostty colors, palette, cursor and fonts do not change application zoom or theme. A nonempty `XDG_CONFIG_HOME` takes
precedence; otherwise the importer uses `dirs`' platform config directory on Linux and local config directory on
Windows, or `~/.config` on macOS. On macOS it also reads the `com.mitchellh.ghostty` application-support directory.
Legacy `config` precedes `config.ghostty`; relative includes follow all default files in recorded order. Named themes
are searched in the Ghostty user themes directory, then `GHOSTTY_RESOURCES_DIR/themes`, or the installed Ghostty
resources discovered from its executable on `PATH` using Ghostty's terminfo layout. A Finder launch whose `PATH`
does not include Ghostty may require `GHOSTTY_RESOURCES_DIR`. Missing themes, required includes
and font families are shown in the window without blocking valid explicit font overrides. For inherited families, the
first installed family in the configured order becomes primary; remaining installed families retain their order.
This is an appearance-field import, not the Ghostty app configuration loader; background opacity, minimum contrast
and window effects are not reproduced.

Project aliases, pins, and hidden projects, plus pinned and hidden sessions within each project, are local presentation
preferences. They are stored in `projects.json` beside the writable `config.toml` (or beside the file passed with
`--config-file`) and apply across gateway reconnects. Delete in the project menu hides a project; restoring it does not
change omp files. A missing preferences file starts empty; older entries without session lists start with empty lists.
Malformed or unreadable preferences stop startup; save errors appear in the window and the unsaved change is reverted.

Verdant's `_normal` supplies base foreground and background. Exact `comment`, `markup.list.unchecked`,
`ibl.indent.char.1`, and `ibl.whitespace.char.1` provide muted text, borders, and control surfaces when available;
missing interface shades are derived from base colors. Secondary panes are derived away from the foreground, and
selection blends the muted text into the background at 25%. Theme-derived accent and user-message colors resolve
through Verdant's `find_style`: exact key, dotted parent, then theme foreground. Custom colors use their saved RGBA values.
The user-message foreground chooses the higher-contrast
theme foreground or background, falling back to black or white when needed for 4.5:1 contrast. Links resolve from
`function`, the caret from `markup.link.url`, and status colors from `diff.plus`, `diff.minus`, `comment.warning`,
and `comment.note`. Missing status styles retain GPUI Kit's mode defaults. Mapping is independent of theme name.

The endpoint parses as a URL and must use `ws` or `wss`. Desktop configuration has no `gateway` table or
`--omp`, `--model`, or `--sessions-dir` flags. Server configuration belongs to `alvind.toml` or `alvind` arguments.
Existing desktop files containing `[gateway]` fail explicitly. Move those settings to `alvind` and remove the table.

The server resolves its sessions directory using omp's rules. The desktop does not inspect local omp configuration
or resolve server paths.

## Startup

`main.rs` is thin; `app.rs` holds the wiring.

1. Load the configuration (below); a parse or load error exits with its message.
2. Initialize `alvin_telemetry::Telemetry` with service name `alvin-app` and the crate version before starting runtime threads.
3. Build and enter the tokio runtime used by client and UI tasks.
4. Connect `Client::connect(Endpoint::WebSocket(url))`. A failed connection exits with its error.
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

`mise dev` starts the gateway daemon, waits for port `7410`, and runs the desktop once. Use
`mise watch --restart dev` to rebuild and restart the desktop when its sources change. The gateway daemon
watches its own sources and rebuilds and restarts on changes; this ends its live agent sessions. It remains
running after the desktop exits; stop it with `mise daemons stop gateway`.
To run a separately configured gateway, use `mise run gateway -- --listen 0.0.0.0:7410 --omp /path/to/omp`.
The desktop accepts `--endpoint`, `ALVIN_ENDPOINT`, or the `endpoint` key in `config.toml`.

Errors before the window opens are `eyre` reports at the edge, printed and exiting non-zero. After the window opens, errors are the UI's (`SPEC-conversation-ui.md`).

## Shutdown

The `Quit` action and last-window-close path release workspace-owned terminals before calling `cx.quit()`.
The bounded `on_app_quit` hook also starts cleanup for platform quit paths. Duplicate quit requests do nothing.
The desktop never calls gateway shutdown or closes agent sessions merely because its window closes.
After the event loop returns, telemetry providers flush and shut down explicitly. `alvind` owns agent teardown.

Terminal cleanup uses the existing remote-mode policy. If the gateway is unreachable when `terminal/close`
is sent, cleanup logs the error and the server shell can remain running. This cutover does not introduce
retries, ownership leases, or a server-side disconnect cleanup policy.

## Out of Scope (v1)

- A web face. `alvin-protocol`, `alvin-client`, and `alvin-conversation-ui` build for wasm32 so it can be added; a browser entry point (`wasm-bindgen`, served bundle, fonts, per gpui-kit's WebAssembly docs) is a later module.
- Signing, installers, and distribution tasks.

## Dependencies

`alvin-conversation-ui`, `alvin-client`, `alvin-telemetry`, `gpui-kit`, `tokio`, `clap`, `confique`, `dirs`, `url`, `eyre`, `tracing`, and the existing appearance/configuration dependencies. `alvin-gateway` is a dev-dependency only. `alvin-app` and `alvin-telemetry` stay outside hakari's server feature-unification graph.

## Project Structure

```
crates/alvin-app/src/main.rs       thin: settings, shared telemetry, run
crates/alvin-app/src/config.rs     confique schema, layer loading, WebSocket endpoint parsing
crates/alvin-app/src/app.rs        network client, window, quit hook
.mise/tasks/dev                    run the desktop app; uses the macOS bundle on macOS
.mise/tasks/package                build the macOS application bundle
```

## Testing Strategy

In-module only (`AGENTS.md`, Testing).

- `config.rs` covers endpoint parsing, unsupported schemes, source precedence, malformed files, obsolete server settings, and round-trip persistence without changing unrelated settings.
- `app.rs` starts a real gateway fixture on `127.0.0.1:0`, connects over WebSocket, opens a live session, and dispatches Quit. After draining headless cleanup, a second connection runs a real bash command in the same session. Fixture teardown shuts the server down. Native smoke separately verifies process exit, because the headless platform's quit is a no-op.
- `settings.rs` exercises the real settings view over WebSocket.
- `mise run check-boundaries`, also run by `mise format`, rejects server implementation crates in desktop, UI, and client normal and build dependencies. Dev-dependencies remain available to real-server tests.

## Boundaries

- Always: follow `AGENTS.md`; wiring only.
- Ask first: new dependencies; new config keys beyond the schema above; new subcommands; packaging.
- Never: protocol, session, or agent logic; reading omp files; a second runtime; validating settings beyond parsing.

## Open Questions

None. In-process desktop transport is deferred. The current desktop requires a separately running gateway.
