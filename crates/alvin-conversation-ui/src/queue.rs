//! The steering and follow-up queue of a session, and the actions each entry allows.
//!
//! omp names a queued message by its text, so the actions send the text. Each entry carries an id the conversation minted
//! for it, and the element ids are derived from that.

use alvin_conversation::{Queue, QueuedMessage};
use alvin_protocol::{LiveSessionId, QueuedMessageQueue};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div};

use crate::model::Model;

fn id(part: &str, message: &QueuedMessage) -> ElementId {
  ElementId::from(SharedString::from(format!("{part}-{}", message.id.as_uuid())))
}

/// The queue, nothing when empty. Leading steering entries already sent into the streaming response show without actions:
/// omp can no longer take them back.
pub(crate) fn view(model: &Entity<Model>, live: LiveSessionId, queue: &Queue, cx: &App) -> Option<impl IntoElement> {
  if queue.steering.is_empty() && queue.follow_up.is_empty() {
    return None;
  }
  let mut column = v_flex()
    .id("queue")
    .test_support()
    .px_4()
    .py_1()
    .gap_1()
    .border_t_1()
    .border_color(cx.theme().border);
  for (index, message) in queue.steering.iter().enumerate() {
    let sent = index < queue.live_steered;
    let mut row = h_flex().id(id("queue-steering", message)).test_support().gap_2().child(
      div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(format!(
          "{} {}",
          if sent { "sent" } else { "steering" },
          message.text
        ))),
    );
    if !sent {
      let (model, text) = (model.clone(), message.text.clone());
      row = row.child(
        Button::new(id("queue-remove-steering", message))
          .small()
          .ghost()
          .label("Remove")
          .on_click(move |_, _, cx| {
            model
              .update(cx, |model, cx| {
                model.remove_queued(live, text.clone(), QueuedMessageQueue::Steering, cx)
              })
              .detach();
          }),
      );
    }
    column = column.child(row);
  }
  for message in &queue.follow_up {
    let (promote_model, remove_model) = (model.clone(), model.clone());
    let (promote_text, remove_text) = (message.text.clone(), message.text.clone());
    column = column.child(
      h_flex()
        .id(id("queue-follow-up", message))
        .test_support()
        .gap_2()
        .child(
          div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!("after the turn: {}", message.text))),
        )
        .child(
          Button::new(id("queue-promote", message))
            .small()
            .ghost()
            .label("Send now")
            .on_click(move |_, _, cx| {
              promote_model
                .update(cx, |model, cx| model.promote_queued(live, promote_text.clone(), cx))
                .detach();
            }),
        )
        .child(
          Button::new(id("queue-remove-follow-up", message))
            .small()
            .ghost()
            .label("Remove")
            .on_click(move |_, _, cx| {
              remove_model
                .update(cx, |model, cx| {
                  model.remove_queued(live, remove_text.clone(), QueuedMessageQueue::FollowUp, cx)
                })
                .detach();
            }),
        ),
    );
  }
  Some(column)
}
