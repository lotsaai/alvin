use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};

pub(in crate::session::transcript) fn view(error: &str, cx: &App) -> AnyElement {
  div()
    .w_full()
    .min_w_0()
    .text_color(cx.theme().danger)
    .child(SharedString::from(error.to_owned()))
    .into_any_element()
}
