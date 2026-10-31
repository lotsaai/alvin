//! The steering and follow-up queue of a session, with the actions each entry allows.
//!
//! omp identifies a queued message by its text, so an entry has no id of its own: the element ids name the queue and the
//! position, and the actions send the text.

use alvin_conversation::Queue;
use alvin_protocol::{LiveSessionId, QueuedMessageQueue};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{App, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div};

use crate::model::Model;

fn id(parts: &str, index: usize) -> ElementId {
  ElementId::from(SharedString::from(format!("{parts}-{index}")))
}

/// The queue, or nothing when it is empty. Leading steering entries that were already sent into the streaming response
/// show without actions: omp can no longer take them back.
pub(crate) fn view(model: &Entity<Model>, live: LiveSessionId, queue: &Queue, cx: &App) -> Option<impl IntoElement> {
  if queue.steering.is_empty() && queue.follow_up.is_empty() {
    return None;
  }
  let mut column = v_flex()
    .id("queue")
    .px_4()
    .py_1()
    .gap_1()
    .border_t_1()
    .border_color(cx.theme().border);
  for (index, text) in queue.steering.iter().enumerate() {
    let sent = index < queue.live_steered;
    let mut row = h_flex().id(id("queue-steering", index)).gap_2().child(
      div()
        .flex_1()
        .min_w_0()
        .truncate()
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(format!("{} {text}", if sent { "sent:" } else { "steering:" }))),
    );
    if !sent {
      let model = model.clone();
      let message = text.clone();
      row = row.child(
        Button::new(id("queue-remove-steering", index))
          .small()
          .ghost()
          .label("Remove")
          .on_click(move |_, _, cx| {
            model.update(cx, |model, cx| {
              model.remove_queued(live, message.clone(), QueuedMessageQueue::Steering, cx)
            });
          }),
      );
    }
    column = column.child(row);
  }
  for (index, text) in queue.follow_up.iter().enumerate() {
    let remove_model = model.clone();
    let promote_model = model.clone();
    let remove_message = text.clone();
    let promote_message = text.clone();
    column = column.child(
      h_flex()
        .id(id("queue-follow-up", index))
        .gap_2()
        .child(
          div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(format!("after this turn: {text}"))),
        )
        .child(
          Button::new(id("queue-promote", index))
            .small()
            .ghost()
            .label("Send now")
            .on_click(move |_, _, cx| {
              promote_model.update(cx, |model, cx| model.promote_queued(live, promote_message.clone(), cx));
            }),
        )
        .child(
          Button::new(id("queue-remove-follow-up", index))
            .small()
            .ghost()
            .label("Remove")
            .on_click(move |_, _, cx| {
              remove_model.update(cx, |model, cx| {
                model.remove_queued(live, remove_message.clone(), QueuedMessageQueue::FollowUp, cx)
              });
            }),
        ),
    );
  }
  Some(column)
}
