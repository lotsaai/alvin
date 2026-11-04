use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::{App, FontWeight, IntoElement as _, ParentElement as _, SharedString, Styled as _, div, prelude::FluentBuilder as _};

pub(super) fn view(path: &str, suffix: Option<&str>, cx: &App) -> gpui_kit::AnyElement {
  let (directory, name) = path
    .rsplit_once('/')
    .map_or((None, path), |(directory, name)| (Some(directory), name));
  h_flex()
    .min_w_0()
    .flex_wrap()
    .when_some(directory, |row, directory| {
      row.child(
        div()
          .min_w_0()
          .text_color(cx.theme().muted_foreground)
          .child(SharedString::from(format!("{directory}/"))),
      )
    })
    .child(
      div()
        .min_w_0()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().foreground)
        .child(SharedString::from(name.to_owned())),
    )
    .when_some(suffix, |row, suffix| {
      row.child(
        div()
          .text_color(cx.theme().muted_foreground)
          .child(SharedString::from(format!(" {suffix}"))),
      )
    })
    .into_any_element()
}
