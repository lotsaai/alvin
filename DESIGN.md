---
name: Alvin
description: Native conversation workspace for omp
colors:
  background: "var(--alvin-background)"
  foreground: "var(--alvin-foreground)"
  sidebar: "var(--alvin-sidebar)"
  muted: "var(--alvin-muted)"
  muted-foreground: "var(--alvin-muted-foreground)"
  border: "var(--alvin-border)"
  primary: "var(--alvin-primary)"
  primary-foreground: "var(--alvin-primary-foreground)"
  popover: "var(--alvin-popover)"
  user-message: "var(--alvin-user-message)"
typography:
  composer:
    lineHeight: 1.5
spacing:
  composer-lane: "68.571429rem"
  sidebar-default: "15rem"
  sidebar-min: "12rem"
  sidebar-max: "24rem"
  terminal-default: "16rem"
  history: "20rem"
---

# Design System: Alvin

## Overview

A restrained native desktop workspace keeps project navigation available while the conversation remains the main working area. The interface uses gpui-kit components, compact controls and a user-selected verdant theme rather than a fixed brand palette.

**Key characteristics:**
- Persistent project and session navigation with an active session title in the inset work area.
- A centred conversation and composer lane with auxiliary history and terminal regions on demand.
- Semantic theme colours and native control states, including keyboard focus.

## Colors

The `var(--alvin-*)` values above are portable semantic placeholders, not CSS variables exported by the native application. Their authority is the corresponding `cx.theme()` role or `UserMessageColors` in Rust. Their actual values change with the selected light or dark verdant theme and user colour choices; there is no fixed Alvin hex palette.

- **Background / foreground:** Verdant supplies the base colors. Catppuccin Latte and Mocha use mantle for secondary panes, surface0 for muted surfaces, and surface1 for borders, following the supplied shadcn palettes.
- **Muted / popover:** Catppuccin uses subtext1 when it has sufficient contrast against surface0, otherwise foreground. Other themes derive muted surfaces and borders from their base colors.
- **Primary:** the selected theme accent or custom color. Selection and tool surfaces use restrained accent tints; focus uses the accent ring.
- **User message:** the independently selected color or accent, with text chosen against its actual composite background. Custom RGBA values remain unchanged.
- **Status:** Catppuccin maps red, green, yellow and blue to danger, success, warning and info. Other themes use their Verdant status styles. Labels and icons accompany color.

Source: `crates/alvin-app/src/settings.rs` (`apply_appearance`), `crates/alvin-conversation-ui/src/session/transcript.rs` (`UserMessageColors`).

## Typography

Use gpui-kit's active theme fonts and semantic text sizes; no fixed family is imposed here. Sidebar, status and secondary text use small text, while settings topic headings use extra-large text. The composer uses the theme monospace size with a 1.5 line-height, grows from two to eight rows, and adds 1rem vertical padding. Terminal font family and size may be configured independently and otherwise follow terminal/theme defaults.

Sources: `crates/alvin-conversation-ui/src/session/composer.rs`, `crates/alvin-conversation-ui/src/session/status.rs`, `crates/alvin-app/src/settings.rs`.

## Layout

The desktop window starts at 1200 × 800 logical pixels. The outer title bar carries sidebar and back/forward navigation. The inset work area's header contains the active session title and menu, a close control, and the terminal toggle. It does not display a session tab strip. The sidebar defaults to 15rem and resizes between 12rem and 24rem; navigation actions precede a separately scrolling project tree, with account and settings controls at the bottom. The conversation occupies the remaining region, inset with the theme's large radius.

Transcript and composer share a centred maximum 960px lane at the default 14px application font, expressed as `960 / 14` rem to follow zoom. Widescreen fills the available width with the same inset. Its selection applies across sessions for the application lifetime. New sessions centre their greeting and composer; existing sessions place the transcript above the composer. At a session-content width of at least 80rem, History opens as a 20rem inline region; narrower content uses a popover. Expanded composer mode fills the available content height. The terminal defaults to 16rem and resizes from 8rem to 40rem. Sidebar, history and terminal transitions use gpui-kit motion.

Sources: `crates/alvin-app/src/app.rs`, `crates/alvin-conversation-ui/src/workspace.rs`, `crates/alvin-conversation-ui/src/session.rs`, `crates/alvin-conversation-ui/src/session/transcript.rs`.

## Elevation & Depth

The workspace distinguishes regions primarily through theme surface tones, borders and inset geometry. Use gpui-kit popovers and menus for temporary layers rather than inventing a separate shadow system.

## Shapes

The main conversation region uses `theme.radius_lg`; the composer uses `theme.radius` and a one-pixel theme border. Resolve radii through the active theme, not fixed values. Keep content clipped to its region where the implementation does so.

## Components

- **Navigation:** compact ghost buttons and a project/checkout/session tree. Active sessions are distinct from keyboard tree selection. Hover- or focus-revealed actions retain accessible labels.
- **Conversation:** user messages use filled bubbles; assistant text uses Markdown without a bubble surface. Event components own their interaction state under a retained transcript registry. A matched tool result updates its call-position block rather than adding a second card. Each tool block contains an outer header, a retained tool card, and reported tool statistics. The card starts with a capped body preview; Show all and Show less are independent of card collapse. Headers precede bodies, and controls remain outside the preview cap. Diff state survives card collapse and source updates. Generation statistics appear once on their owning message. Message actions copy text, collapse the local view, and restore text only to an empty composer.
- **Query navigation:** at a transcript width of at least 54rem, a left-edge rail marks loaded user messages and previews them on hover or focus. Narrower content uses a Queries menu in the session header, outside the scrolling transcript. Selecting a query scrolls the transcript; it does not alter session history.
- **Composer:** muted, bordered container with a context trigger at its top edge, markdown editor, model and thinking pickers, expand/collapse and send/stop controls. Expanded mode shares the editor state; Enter inserts a newline and Command/Control+Enter submits, while Escape collapses without discarding the draft. Opening context refreshes session statistics; the popover shows working directory, available model, message count, last reported context usage and available usage totals. The trigger reports the model's context-window capacity, or says it is unavailable; it is not a live usage percentage.
- **Settings:** a separate settings view with a navigation sidebar containing Back to sessions and exactly General, Appearance, Hosts and Terminal destinations. Appearance includes system/light/dark mode and theme and colour choices. Settings forms use a centred maximum 42rem content lane in an inset scrollable surface.

## Do's and Don'ts

### Do:
- **Do** use semantic theme roles and preserve user-selected light and dark themes.
- **Do** keep session context statistics labelled according to what the gateway reported.
- **Do** use native components, accessible control names and visible interaction states.

### Don't:
- **Don't** replace dynamic colours with a fixed palette.
- **Don't** present context-window capacity as current usage.
- **Don't** treat the deferred editor or generative UI as implemented.
