//! Projects, their checkouts and sessions, and the commands to open them.

use alvin_conversation::SessionsKey;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div, px};

use crate::model::Model;

/// An element id derived from a domain id: `session-<id>`, `new-session-<checkout id>`.
pub(crate) fn id(prefix: &str, key: &str) -> ElementId {
  ElementId::from(SharedString::from(format!("{prefix}-{key}")))
}

/// The sidebar's content, read from the model.
pub(crate) fn sidebar(model: &Entity<Model>, cx: &App) -> impl IntoElement {
  let state = model.read(cx);
  let workspace = state.conversation().workspace();
  let mut column = v_flex()
    .id("sidebar")
    .w(px(280.))
    .h_full()
    .gap_2()
    .p_2()
    .overflow_hidden()
    .border_r_1()
    .border_color(cx.theme().border);
  for project in workspace.projects() {
    column = column.child(
      div()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .truncate()
        .child(SharedString::from(project.root.display().to_string())),
    );
    for checkout in &project.checkouts {
      let key = SessionsKey {
        project: project.id.clone(),
        checkout: Some(checkout.id.clone()),
      };
      let new_model = model.clone();
      let new_project = project.id.clone();
      let new_checkout = checkout.id.clone();
      column = column.child(
        h_flex()
          .gap_2()
          .child(
            div()
              .flex_1()
              .min_w_0()
              .truncate()
              .text_color(cx.theme().muted_foreground)
              .child(SharedString::from(checkout.root.display().to_string())),
          )
          .child(
            Button::new(id("new-session", checkout.id.as_str()))
              .small()
              .ghost()
              .label("New")
              .on_click(move |_, _, cx| new_model.update(cx, |model, cx| model.new_session(new_project.clone(), new_checkout.clone(), cx))),
          ),
      );
      if let Some(list) = workspace.sessions(&key) {
        for session in &list.sessions {
          let open_model = model.clone();
          let session_id = session.id.clone();
          let title = session.title.clone().unwrap_or_else(|| session.id.as_str().to_owned());
          column = column.child(
            Button::new(id("session", session.id.as_str()))
              .ghost()
              .label(SharedString::from(title))
              .on_click(move |_, _, cx| open_model.update(cx, |model, cx| model.open_session(session_id.clone(), cx))),
          );
        }
        if let Some(next) = list.next.clone() {
          let more_model = model.clone();
          let more_key = key.clone();
          column = column.child(
            Button::new(id("more-sessions", checkout.id.as_str()))
              .small()
              .link()
              .label("Older sessions")
              .on_click(move |_, _, cx| more_model.update(cx, |model, cx| model.load_sessions(more_key.clone(), Some(next.clone()), cx))),
          );
        }
      }
    }
  }
  column
}
