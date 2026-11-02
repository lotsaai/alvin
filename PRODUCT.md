# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop (macOS, Windows, Linux); the Impeccable platform schema has no desktop value.

## Users

Developers using the omp coding agent across projects and sessions. Assume users are competent.

## Product Purpose

alvin is a desktop client for omp. It helps developers find project history and work with multiple live agent sessions.

## Positioning

A gateway manages one omp process per live session. The desktop client can use an embedded gateway or connect to a standalone gateway.

## Operating Context

Projects are grouped by repository, with git worktrees and jj workspaces as subgroups. Session history comes from omp session files. Desktop platforms are macOS, Windows, and Linux.

## Capabilities and Constraints

- Run several sessions concurrently, each with its own omp process.
- Support omp's commands and conversation capabilities without inventing composer conventions.
- Read omp session files without writing, renaming, or deleting them.
- Do not add access controls, tokens, or protective defaults.
- Use gpui-kit for the desktop interface.
- Open settings from the bottom bar beside the sidebar toggle.

## Product Principles

- Support the developer's existing omp workflows and history.
- Make multiple projects and live sessions manageable together.
- Trust users to control their own environment.
- Preserve omp as the source of session history.

## Evidence on Hand

`SPEC.md` describes the product, capabilities, and constraints. `SPEC-app.md` describes the desktop application and gateway connection options.
