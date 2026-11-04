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

Use gpui-kit's active theme fonts and semantic text sizes; no fixed family is imposed here. Sidebar, status and secondary text use small text, while settings topic headings use extra-large text. The composer uses the theme monospace size with a 1.5 line-height and grows from two to eight rows. Terminal font family and size may be configured independently and otherwise follow terminal/theme defaults.

Sources: `crates/alvin-conversation-ui/src/session/composer.rs`, `crates/alvin-conversation-ui/src/session/status.rs`, `crates/alvin-app/src/settings.rs`.

## Layout

The desktop window starts at 1200 × 800 logical pixels. The outer title bar carries sidebar and back/forward navigation. The inset work area's header contains the active session title and menu, a close control, and the terminal toggle. It does not display a session tab strip. The sidebar defaults to 15rem and resizes between 12rem and 24rem; navigation actions precede a separately scrolling project tree, with account and settings controls at the bottom. The conversation occupies the remaining region, inset with the theme's large radius.

Transcript and composer share a centred lane capped at 120 columns of the active theme's monospace font. The width is measured from the font's shaped glyph advance, not a fixed pixel estimate, and follows font changes and zoom. Available space can narrow the lane; roughly 80–120 characters is the desktop working range, not a minimum imposed on small windows. Widescreen still fills the available width with the same inset. Its selection applies across sessions for the application lifetime. New sessions centre their greeting and composer; existing sessions place the transcript above the composer. At a session-content width of at least 80rem, History opens as a 20rem inline region; narrower content uses a popover. Expanded composer mode fills the available content height. The terminal defaults to 16rem and resizes from 8rem to 40rem. Sidebar, history and terminal transitions use gpui-kit motion.

Existing conversations reserve a 2.5rem query-navigation column before the horizontally clipped transcript viewport. Activity markers retain their separate clearance inside that viewport; each activity's content clips independently of its marker. Transcript and composer edges remain aligned in normal and widescreen modes. The 120-column cap is unchanged; the reserved navigation column reduces available width rather than covering content.

Sources: `crates/alvin-app/src/app.rs`, `crates/alvin-conversation-ui/src/workspace.rs`, `crates/alvin-conversation-ui/src/session.rs`, `crates/alvin-conversation-ui/src/session/transcript.rs`.

## Elevation & Depth

The workspace distinguishes regions primarily through theme surface tones, borders and inset geometry. The composer uses the active theme's medium shadow token, muted surface and border for restrained elevation; its margins leave space for the shadow. The border remains visible when a theme disables shadows. Use gpui-kit popovers and menus for temporary layers rather than inventing a separate shadow system.

## Shapes

The main conversation region and composer use `theme.radius_lg`; the composer retains a one-pixel theme border. Resolve radii through the active theme, not fixed values. Keep content clipped to its region where the implementation does so.

## Components

- **Navigation:** compact ghost buttons and a project/checkout/session tree. Active sessions are distinct from keyboard tree selection. Hover- or focus-revealed actions retain accessible labels.
- **Conversation:** user messages use filled bubbles; assistant text uses Markdown without a bubble surface. Event components own their interaction state under a retained transcript registry. A matched tool result updates its call-position block rather than adding a second card. Each tool block contains an outer header, a retained tool card, and reported tool statistics. Non-script cards start with a capped body preview; Show all and Show less are independent of card collapse. Headers precede bodies, and controls remain outside the preview cap. Diff state survives card collapse and source updates. Generation statistics appear once on their owning message. Message actions copy text, collapse the local view, and restore text only to an empty composer.
- **Conversation disclosures:** potentially mutating and unknown tools start expanded; known read-only tools and thinking start collapsed. General → Conversation persists the three application-wide defaults. An explicit block choice overrides its category default and survives streaming updates and regrouping.
- **System activity:** notices and source-defined system messages share compact severity/provenance treatment with complete content, raw inspection and copy on demand. Session activity starts as one collapsed summary above the composer; its expanded list is capped at 12rem and scrolls independently. Routine informational activity does not open notification popups. Warning/error notifications use a bounded text preview rather than the full payload; original content stays in the activity list. Typed todo reminders retain their reported attempt and complete task payload here, without adding another current-plan card.
- **Resources and search:** resource rows preserve paths, reported ranges and non-file links. Multi-target and compatible adjacent reads retain independent target disclosures; combined output is shown once, without invented per-target attribution. Search keeps structured files/matches alongside complete original output and raw inspection.
- **Tasks and plans:** task rows retain reported agent/job identity, assignment, status and duration, including progress received after the spawn response. Matching asynchronous delivery or direct `wait` settles the existing operation; attributable delivered text becomes its normal output and full-copy content. Raw inspection retains the original response, latest available progress and delivery. Shared batch text is not invented as per-job output. The current todo plan is separate from historical tool results and retains phases, completed counts and blocked reasons.
- **Process disclosure:** manual process folding hides intermediate work, never the final answer or its actions. Reopening restores child disclosures; completion does not automatically fold the process.
- **Inline images:** supported `ImageContent` supplies actual image bytes or a URL. Invalid data and unsupported formats remain visible errors with the original payload inspectable; metadata-only results do not become fabricated previews.
- **Script panels:** Bash calls, script-evaluation calls and Python execution events share a retained source/output control. Verdant highlights the source, initially showing its first four logical lines. Monochrome preformatted output initially shows its last twelve logical lines. A subtle line-count disclosure follows the source and precedes the output. Each section expands and collapses independently; expansion survives result updates, raw-data viewing and outer-card collapse. Copy uses the full original output, not the visible tail. Compact header actions expose copy and raw data; the footer retains reported exit status and duration.
- **Query navigation:** at a transcript width of at least 54rem, a left-edge rail marks loaded user messages and previews them on hover or focus. Narrower content uses a Queries menu in the session header, outside the scrolling transcript. Selecting a query scrolls the transcript; it does not alter session history.
- **Composer:** a retained container component owns the markdown editor, expansion state, model and thinking pickers, context popover, and send/stop controls. Compact mode puts the controls in a bottom toolbar with icon-only expansion and primary send actions. Context progress shows the last reported usage, not model capacity; unavailable usage has an information icon. Statistics refresh when a session loads, reloads, settles, or its context popover opens. The popover retains the detailed working directory, model, message count, context usage and session totals. Expanded mode shares the editor state; Enter inserts a newline and Command/Control+Enter submits, while Escape collapses without discarding the draft.
- **Model selection:** the composer button and Alt+P open a searchable modal over omp's model catalog. Search matches provider, ID and display name. Rows distinguish the current model from pending selection and show reported limits, reasoning support and pricing in the detail area. Enter or a row click selects; only Use for session applies. Close and Escape leave the model and draft unchanged. Selection uses omp's session-only `/switch` command, not a saved role assignment. Roles and presets are not edited here.
- **Composer keyboard:** native cursor, word and line movement remain editor-owned. Ctrl+W/U/K delete backwards by word, to line start and to line end; Ctrl+D deletes forward without exiting the desktop. Shift+Enter and Alt+Enter insert newlines. Ctrl+C clears the draft. Alt+Shift+L/C copy the current line or whole prompt. Up from an empty editor browses submitted prompts; Down returns toward the saved draft, preserving undo history. Ctrl+R searches prompt history and restores without submitting. Shift+Tab cycles omp's available thinking levels when completion does not own the key, including off and automatic selection. The thinking control uses omp's progressively filled circle glyphs with explicit level labels; auto remains visible instead of being replaced by its effective effort. Clipboard paste is text-only; no external-editor or terminal lifecycle shortcuts are added.
- **Message metadata:** a small, width-bounded model/token/time summary follows the message actions. An information button opens reported usage details, including cache counts and time to first token. Unknown values remain absent; reported zero values remain visible. The details popover supports keyboard scrolling and Escape dismissal.
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
