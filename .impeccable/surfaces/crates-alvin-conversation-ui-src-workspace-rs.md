---
version: 1
slug: "crates-alvin-conversation-ui-src-workspace-rs"
primary_target: "crates/alvin-conversation-ui/src/workspace.rs"
related_targets: ["crates/alvin-conversation-ui/src/session.rs"]
---

# Conversation workspace

Mode: Operate. Developers browse project sessions and work with omp in a native desktop window. Preserve actual session state, keyboard access, user-selected themes and the gateway boundary. A right tabbed editor and generative UI are deferred.

## Direction contract

**THESIS:** Give navigation and conversation their own continuous, usable regions; refuse a generic dashboard or a tab-strip-centred shell.

**OWN-WORLD:** Follow the user-pinned ZCode, LobeHub and Tern references at full fidelity: ZCode's continuous sidebar frame and inset work area, LobeHub's centred conversation and expandable native markdown composer, and Tern's composer-edge context control. Colours follow the selected verdant theme, not a fixed palette.

**STORY:** Find a project or session, read its conversation and final answer, inspect intermediate activity when needed, then compose the next request.

**FIRST VIEWPORT:** The left frame provides navigation, while the inset area holds the session header and a centred transcript/composer lane. A single left-edge query rail navigates user messages. Context opens at the composer's top edge; its statistics report actual available values.

**FORM:** Native gpui-kit controls and semantic theme roles; final answer outside the process disclosure, a shared-state expanded editor, and a retained terminal below the conversation. Sidebar, history and terminal transitions use the implemented 180ms duration. seed: none (user-pinned ZCode/LobeHub/Tern references)

**FINISH:** unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
