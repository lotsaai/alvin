//! App-wide defaults for transcript disclosure.

use gpui_kit::{App, Global};

/// Presentation defaults for newly displayed conversation blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConversationPreferences {
  /// Expand potentially mutating tool calls.
  pub expand_mutating_tools: bool,
  /// Expand readonly tool calls.
  pub expand_readonly_tools: bool,
  /// Expand thinking blocks.
  pub expand_thinking: bool,
}

impl ConversationPreferences {
  /// Creates disclosure defaults.
  pub fn new(expand_mutating_tools: bool, expand_readonly_tools: bool, expand_thinking: bool) -> Self {
    Self {
      expand_mutating_tools,
      expand_readonly_tools,
      expand_thinking,
    }
  }

  pub(crate) fn current(cx: &App) -> Self {
    cx.try_global::<Self>().copied().unwrap_or_default()
  }
}

impl Default for ConversationPreferences {
  fn default() -> Self {
    Self::new(true, false, false)
  }
}

impl Global for ConversationPreferences {}
