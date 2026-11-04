use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::{AnyElement, App, IntoElement as _, SharedString, StyleRefinement, Styled as _, div, transparent_black};

use crate::session::markdown;

pub(super) fn detected_language(value: &str) -> Option<String> {
  markdown::detected_language(value)
}

pub(super) fn view(id: SharedString, value: &str, language: &str, _cx: &App) -> AnyElement {
  let language = if language.is_empty() {
    detected_language(value).unwrap_or_else(|| "text".to_owned())
  } else {
    language.to_owned()
  };
  let longest = value.split(|c| c != '`').map(str::len).max().unwrap_or_default();
  let fence = "`".repeat(longest.max(2).saturating_add(1));
  let content = format!("{fence}{language}\n{value}\n{fence}");
  let block = StyleRefinement::default().bg(transparent_black()).border_0().p_0();
  TextView::markdown(id, content)
    .style(TextViewStyle::default().code_block(block))
    .code_block_actions(|_, _, _| div())
    .into_any_element()
}
