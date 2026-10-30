# Spec: scaffold

Module id `scaffold`. Depends on nothing. Parent: `SPEC.md`. Law: `AGENTS.md`.

## Objective

Give every later module a compiling workspace, working `mise` tasks, `hk` checks, and hakari. Delete heyday and NSH leftovers. No product code.

Success:

- `mise format` and `mise test` run clean on the empty workspace; each step read.
- `mise hakari` produces no diff on a second run.
- `.mise/tasks/` holds only tasks alvin uses.
- `hk.pkl`, `mise.toml`, and `.mise/tasks/` contain no NSH or heyday references.

## Scope

In: root `Cargo.toml`, `.config/hakari.toml`, `crates/alvin-workspace-hack/`, `.mise/tasks/` pruning, `hk.pkl` and `mise.toml` NSH cleanup, `.cargo/config.toml` only if a task needs it.

Out: all other crates. Each module spec creates its own crate, its row in `AGENTS.md`, and its hakari entry. `mise` environment problems are escalated to the user, never fixed.

## Workspace

- Root `Cargo.toml`: `[workspace]`, `resolver = "3"`, `members = ["crates/*"]`, `[workspace.package]` edition 2024, `[workspace.dependencies]` for shared crates.
- Only member at this stage: `alvin-workspace-hack`. Hakari at two or more crates; the first real crate (`alvin-protocol`) makes it required. Create the hack crate now so the task is proven.
- `.config/hakari.toml`: `hakari-package = "alvin-workspace-hack"`, `dep-format-version = "4"`, `resolver = "2"`, platforms: aarch64-apple-darwin, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu. `wasm32-unknown-unknown` is a platform too, because `alvin-protocol` and `alvin-client` build for it.
- `rustfmt.toml` unchanged (2 spaces, 140 columns).

## Task Set

Keep, rewritten for alvin:

| Task | Content |
|---|---|
| `format` | `hk fix --quiet --all` (unchanged) |
| `check` | `hk check --all` (unchanged) |
| `hakari` | `generate`, `manage-deps -y`, `cargo sort --workspace --grouped .`; `outputs` path becomes `crates/alvin-workspace-hack/Cargo.toml` |
| `clean` | `cargo clean` (unchanged) |
| `build` (`_default`, `debug`, `release`) | `cargo build --workspace [--release]`. `sources`: `Cargo.toml`, `Cargo.lock`, `crates/*/Cargo.toml`, `crates/*/build.rs`, `crates/*/src/**/*.rs`. `outputs`: `target/{debug,release}/alvin-app` and `target/{debug,release}/alvind`, each added by the module that creates the binary |
| `test` (`_default`) | `cargo nextest run --workspace` through `test/rust`; drop the OS fan-out. `sources`: the build sources plus `crates/*/tests/**/*.rs` and `crates/*/fixtures/**` |
| `test/rust` | unchanged |
| `dev` | runs `alvin-app` (`cargo run -p alvin-app`); created by the `app` module, not here |

Delete: `nsh/prep`, `generate/sql/heydayd`, `test/server.sh`, `test/{macos,linux,windows,web,ios,android}`, `dev/{desktop,web,ios,android,api}`, `dist/*`.

Open item for `app`: desktop packaging tasks come back under `dist/` only when that module needs them.

`mise.toml`: keep every tool and the wasm32 target (mandatory). Remove `sqlc`, `java`, and the iOS and Android targets (approved). Keep the rest, including the `fnox` plugin. Drop the stale NSH comment.

`hk.pkl`: remove `**/.nsh/**` excludes. Keep cargo-check, cargo-clippy, rustfmt, cargo-sort steps and the toml, shell, yaml linters.

## Commands

```
Format:  mise format
Test:    mise test
Fresh:   mise run --force test
Hakari:  mise hakari
```

`mise test` with zero tests must exit cleanly. If nextest rejects an empty workspace, escalate; do not add a dummy test.

## Code Style

Nothing to write yet. The hack crate is hakari-managed and never edited by hand.

## Testing Strategy

No tests. Verification is running the commands above and reading each step.

## Boundaries

- Always: follow `AGENTS.md`; jj for mutations.
- Ask first: removing tools from `mise.toml`, adding dependencies.
- Never: fix mise environment problems, hand-edit the hakari block, add `allow` attributes, add crates beyond the `AGENTS.md` table.

## Open Questions

None.
