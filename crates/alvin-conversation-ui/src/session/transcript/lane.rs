use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName};
use gpui_kit::{AnyElement, App, Div, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, div, rems};

pub(super) fn item(id: SharedString, icon: IconName, content: AnyElement, cx: &App) -> Div {
  div()
    .relative()
    .w_full()
    .min_w_0()
    .child(
      div()
        .id(SharedString::from(format!("{id}-lane-marker")))
        .test_support()
        .absolute()
        .left(rems(-2.))
        .top_0()
        .size_6()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().background)
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(icon)),
    )
    .child(
      div()
        .id(SharedString::from(format!("{id}-lane-content")))
        .test_support()
        .w_full()
        .min_w_0()
        .overflow_x_hidden()
        .child(content),
    )
}
