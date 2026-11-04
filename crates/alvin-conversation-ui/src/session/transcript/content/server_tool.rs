use alvin_protocol::AnthropicServerToolContent;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use serde_json::Value;

pub(super) fn name(tool: &AnthropicServerToolContent) -> &str {
  tool
    .block
    .as_ref()
    .and_then(|block| block.get("name").or_else(|| block.get("type")))
    .and_then(Value::as_str)
    .unwrap_or("unknown")
}

pub(super) fn view(name: &str, cx: &App) -> AnyElement {
  div()
    .w_full()
    .min_w_0()
    .text_color(cx.theme().foreground)
    .child(SharedString::from(format!("Server tool · {name}")))
    .into_any_element()
}
