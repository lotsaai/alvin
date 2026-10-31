//! What extensions put around the composer: widget lines above and below it, and status entries.

use alvin_conversation::LiveView;
use alvin_protocol::WidgetPlacement;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div};

/// The widgets of one placement, in key order. A widget without a placement goes above the composer, where omp's own
/// widgets live.
pub(crate) fn widgets(view: &LiveView, placement: WidgetPlacement, cx: &App) -> Option<impl IntoElement> {
  let shown: Vec<_> = view
    .widgets
    .iter()
    .filter(|(_, widget)| widget.placement.unwrap_or(WidgetPlacement::AboveEditor) == placement)
    .collect();
  if shown.is_empty() {
    return None;
  }
  let id = match placement {
    WidgetPlacement::AboveEditor => "widgets-above",
    WidgetPlacement::BelowEditor => "widgets-below",
  };
  let mut column = v_flex()
    .id(ElementId::from(id))
    .px_4()
    .gap_0p5()
    .text_sm()
    .text_color(cx.theme().muted_foreground);
  for (_, widget) in shown {
    for line in &widget.lines {
      column = column.child(div().child(SharedString::from(line.clone())));
    }
  }
  Some(column)
}

/// The status entries extensions set, joined in key order, or nothing.
pub(crate) fn statuses(view: &LiveView) -> Option<String> {
  (!view.statuses.is_empty()).then(|| view.statuses.values().map(String::as_str).collect::<Vec<_>>().join(" · "))
}
