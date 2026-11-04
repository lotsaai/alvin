use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};

pub(super) fn view(size: Option<usize>, cx: &App) -> AnyElement {
  let label = match size {
    Some(size) => format!("Redacted reasoning · {size} encoded bytes"),
    None => "Redacted reasoning".to_owned(),
  };
  div()
    .w_full()
    .min_w_0()
    .text_color(cx.theme().foreground)
    .child(SharedString::from(label))
    .into_any_element()
}
