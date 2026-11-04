use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use serde_json::{Map, Value};

pub(super) fn model(value: &Option<Map<String, Value>>) -> &str {
  value
    .as_ref()
    .and_then(|value| value.get("model"))
    .and_then(Value::as_str)
    .unwrap_or("unknown")
}

pub(super) fn view(from: &str, to: &str, cx: &App) -> AnyElement {
  div()
    .w_full()
    .min_w_0()
    .text_color(cx.theme().foreground)
    .child(SharedString::from(format!("Provider fallback · {from} → {to}")))
    .into_any_element()
}
