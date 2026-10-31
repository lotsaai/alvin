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

/// What a session's banner says, when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Banner {
  /// The omp process ended. Nothing more arrives; the session stays until it is closed.
  Exited,
  /// The event stream had a gap: what was on screen was dropped and is being read again.
  Reloading,
}

impl Banner {
  /// The words the banner shows.
  pub(crate) fn text(self) -> &'static str {
    match self {
      Self::Exited => "omp exited",
      Self::Reloading => "Reloading the session",
    }
  }
}

/// The banner a session shows: the process ending outranks a reload, which waits for a process that can still answer.
pub(crate) fn banner(view: &LiveView) -> Option<Banner> {
  if view.exited {
    Some(Banner::Exited)
  } else if view.reload_needed {
    Some(Banner::Reloading)
  } else {
    None
  }
}
