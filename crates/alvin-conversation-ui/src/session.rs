//! One live session: its transcript, its status line and its composer.

use alvin_protocol::LiveSessionId;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
  AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
  StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};

use crate::actions::{Abort, SESSION_CONTEXT};
use crate::model::{Model, ModelEvent};
use crate::{composer, queue, transcript};

/// A live session's view. Holds only what is local to the view: the scroll state, the composer and how many rows the
/// scroller was last told about. Everything else is read from the model.
pub(crate) struct SessionView {
  live: LiveSessionId,
  model: Entity<Model>,
  scroller: Entity<MessageScrollerState>,
  composer: Entity<TextareaState>,
  rows: usize,
  _subscriptions: Vec<Subscription>,
}

impl SessionView {
  pub(crate) fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let composer = composer::state(window, cx);
    let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
    let subscriptions = vec![
      cx.observe(&scroller, |_, _, cx| cx.notify()),
      cx.observe(&model, |this, _, cx| {
        this.sync_rows(cx);
        cx.notify();
      }),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        if let ModelEvent::Restored { live, text } = event
          && *live == this.live
        {
          this
            .composer
            .update(cx, |composer, cx| composer.set_value(text.clone(), window, cx));
        }
      }),
      cx.subscribe_in(&composer, window, |this, state, event: &InputEvent, window, cx| {
        if let Some(submission) = composer::submitted(event, state, cx) {
          this
            .model
            .update(cx, |model, cx| model.send(this.live, &submission.text, submission.follow_up, cx));
          state.update(cx, |state, cx| state.set_value("", window, cx));
        }
      }),
    ];
    let mut view = Self {
      live,
      model,
      scroller,
      composer,
      rows: 0,
      _subscriptions: subscriptions,
    };
    view.sync_rows(cx);
    view
  }

  /// Tells the scroller how the transcript changed since it last looked: rows appended, the transcript replaced, or the
  /// streaming row grew taller.
  fn sync_rows(&mut self, cx: &mut Context<Self>) {
    let Some(view) = self.model.read(cx).conversation().live(self.live) else {
      return;
    };
    let rows = transcript::row_count(view);
    let previous = self.rows;
    self.rows = rows;
    self.scroller.update(cx, |scroller, cx| {
      if rows > previous {
        scroller.append(rows - previous, cx);
      } else if rows < previous {
        scroller.reset(rows, cx);
      }
      if let Some(last) = rows.checked_sub(1) {
        scroller.remeasure_items(last..rows, cx);
      }
    });
  }
}

impl Render for SessionView {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let model = self.model.read(cx);
    let status: SharedString = match model.conversation().live(self.live) {
      Some(view) if view.exited => "omp exited".into(),
      Some(view) if view.reload_needed => "reload needed".into(),
      Some(_) if model.bash_running(self.live) => "running a command".into(),
      Some(view) => match view.run {
        alvin_conversation::Run::Running => "working".into(),
        alvin_conversation::Run::Idle => "idle".into(),
        alvin_conversation::Run::Settled => "settled".into(),
      },
      None => "closed".into(),
    };
    let live = self.live;
    v_flex()
      .key_context(SESSION_CONTEXT)
      .on_action(cx.listener(move |this, _: &Abort, _, cx| {
        this.model.update(cx, |model, cx| model.abort(live, cx));
      }))
      .size_full()
      .child(transcript::transcript(&self.model, self.live, &self.scroller))
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(self.live)
          .and_then(|view| queue::view(&self.model, self.live, &view.queue, cx)),
      )
      .child(
        div()
          .id("session-status")
          .px_4()
          .text_sm()
          .text_color(cx.theme().muted_foreground)
          .role(gpui_kit::Role::Status)
          .aria_label(status.clone())
          .child(status),
      )
      .child(div().p_2().child(composer::view(&self.composer)))
  }
}
