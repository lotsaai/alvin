//! One live session: its transcript, its status line and its composer.

use alvin_protocol::{LiveSessionId, WidgetPlacement};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
  StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};

use crate::actions::{Abort, SESSION_CONTEXT};
use crate::dialogs::{self, DialogHost};
use crate::history_tree::HistoryTreeView;
use crate::model::{Model, ModelEvent};
use crate::pickers::Pickers;
use crate::{composer, queue, status, transcript};

/// A live session's view. Holds only what is local to the view: the scroll state, the composer and how many rows the
/// scroller was last told about. Everything else is read from the model.
pub(crate) struct SessionView {
  live: LiveSessionId,
  model: Entity<Model>,
  scroller: Entity<MessageScrollerState>,
  composer: Entity<TextareaState>,
  pickers: Pickers,
  dialogs: Entity<DialogHost>,
  /// The history panel, and whether it is open.
  history: Entity<HistoryTreeView>,
  history_open: bool,
  /// The session menu, and the name being typed into it.
  menu_open: bool,
  name: Entity<TextareaState>,
  rows: usize,
  _subscriptions: Vec<Subscription>,
}

impl SessionView {
  /// While a turn runs, a button that aborts it and sends what the composer holds in its place.
  fn interrupt_button(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
    let live = self.live;
    let running = self
      .model
      .read(cx)
      .conversation()
      .live(live)
      .is_some_and(|view| view.run == alvin_conversation::Run::Running);
    if !running {
      return None;
    }
    let (model, composer) = (self.model.clone(), self.composer.clone());
    Some(
      h_flex()
        .px_4()
        .child(
          Button::new(ElementId::from("session-interrupt"))
            .small()
            .ghost()
            .label("Interrupt and send")
            .on_click(move |_, window, cx| {
              let text = composer.read(cx).value().trim().to_owned();
              if text.is_empty() {
                return;
              }
              composer.update(cx, |composer, cx| composer.set_value("", window, cx));
              model.update(cx, |model, cx| model.interrupt_and_send(live, text, cx));
            }),
        )
        .into_any_element(),
    )
  }

  /// The session menu: rename, compact, export, and the counts omp gives.
  fn session_menu(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
    let live = self.live;
    let model = self.model.clone();
    let stats = model.read(cx).stats(live).map(|stats| {
      format!(
        "{} messages, {} tool calls, {} tokens",
        stats.total_messages, stats.tool_calls, stats.tokens.total
      )
    });
    let act = |name: &'static str, label: &'static str| Button::new(ElementId::from(name)).small().ghost().label(label);
    let rename = {
      let (model, name) = (model.clone(), self.name.clone());
      move |_: &_, _: &mut Window, cx: &mut gpui_kit::App| {
        let name = name.read(cx).value().trim().to_owned();
        if !name.is_empty() {
          model.update(cx, |model, cx| model.rename(live, name, cx));
        }
      }
    };
    let (compact_model, export_model, stats_model) = (model.clone(), model.clone(), model);
    v_flex()
      .id("session-menu")
      .px_4()
      .py_1()
      .gap_1()
      .child(
        h_flex()
          .gap_1()
          .child(
            div().flex_1().child(
              Textarea::new(&self.name)
                .accessibility_id("session-name")
                .aria_label("Session name"),
            ),
          )
          .child(act("session-rename", "Rename").on_click(rename)),
      )
      .child(
        h_flex()
          .gap_1()
          .child(act("session-compact", "Compact").on_click(move |_, _, cx| compact_model.update(cx, |model, cx| model.compact(live, cx))))
          .child(
            act("session-export", "Export HTML").on_click(move |_, _, cx| export_model.update(cx, |model, cx| model.export_html(live, cx))),
          )
          .child(
            act("session-stats", "Refresh counts")
              .on_click(move |_, _, cx| stats_model.update(cx, |model, cx| model.refresh_stats(live, cx))),
          ),
      )
      .children(stats.map(|text| div().id("session-stats-text").text_sm().child(SharedString::from(text))))
      .into_any_element()
  }

  /// While the composer holds a `/` and a name being typed, the session's commands that start with it. Choosing one puts
  /// its name in the composer; nothing runs it.
  fn command_completion(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
    let typed = self.composer.read(cx).value().to_string();
    let prefix = typed.strip_prefix('/')?;
    if prefix.contains(char::is_whitespace) {
      return None;
    }
    let names: Vec<String> = self
      .model
      .read(cx)
      .conversation()
      .live(self.live)?
      .commands
      .iter()
      .filter(|command| command.name.starts_with(prefix))
      .take(8)
      .map(|command| command.name.clone())
      .collect();
    if names.is_empty() {
      return None;
    }
    let mut row = h_flex().id("command-completion").px_4().gap_1().flex_wrap();
    for name in names {
      let composer = self.composer.clone();
      let text = format!("/{name} ");
      row = row.child(
        Button::new(ElementId::from(SharedString::from(format!("command-{name}"))))
          .small()
          .ghost()
          .label(SharedString::from(format!("/{name}")))
          .on_click(move |_, window, cx| composer.update(cx, |composer, cx| composer.set_value(text.clone(), window, cx))),
      );
    }
    Some(row.into_any_element())
  }

  pub(crate) fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let composer = composer::state(window, cx);
    let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
    let pickers = Pickers::new(&model, live, window, cx);
    let dialogs = DialogHost::new(window, cx);
    let history = cx.new(|cx| HistoryTreeView::new(model.clone(), live, window, cx));
    let name = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 1).placeholder("Session name"));
    let subscriptions = vec![
      cx.observe(&scroller, |_, _, cx| cx.notify()),
      cx.observe_in(&model, window, |this, model, window, cx| {
        this.sync_rows(cx);
        this.pickers.sync(&model, this.live, window, cx);
        DialogHost::sync(&this.dialogs, &model, this.live, window, cx);
        cx.notify();
      }),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        if let ModelEvent::ComposerText { live, text } = event
          && *live == this.live
        {
          this
            .composer
            .update(cx, |composer, cx| composer.set_value(text.clone(), window, cx));
        }
      }),
      cx.subscribe_in(&composer, window, |this, state, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::Change) {
          cx.notify();
        }
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
      pickers,
      dialogs,
      history,
      history_open: false,
      menu_open: false,
      name,
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
    let mut line: SharedString = match model.conversation().live(self.live) {
      Some(view) if view.exited => "omp exited".into(),
      Some(view) if view.reload_needed => "reload needed".into(),
      Some(_) if model.bash_running(self.live) => "running a command".into(),
      Some(view) if view.compaction.is_some() => "compacting".into(),
      Some(view) if view.retry.is_some() => view.retry.as_ref().map_or_else(
        || "retrying".into(),
        |retry| format!("retrying, attempt {} of {}", retry.attempt, retry.max_attempts).into(),
      ),
      Some(view) => match view.run {
        alvin_conversation::Run::Running => "working".into(),
        alvin_conversation::Run::Idle => "idle".into(),
        alvin_conversation::Run::Settled => "settled".into(),
      },
      None => "closed".into(),
    };
    if let Some(statuses) = model.conversation().live(self.live).and_then(status::statuses) {
      line = format!("{line} · {statuses}").into();
    }
    let live = self.live;
    let toggle_view = cx.entity();
    let banner = model.conversation().live(live).and_then(status::banner).map(|banner| {
      let close_model = self.model.clone();
      h_flex()
        .id("session-banner")
        .px_4()
        .py_1()
        .gap_2()
        .bg(cx.theme().warning)
        .text_color(cx.theme().warning_foreground)
        .role(gpui_kit::Role::Status)
        .aria_label(banner.text())
        .child(div().flex_1().child(banner.text()))
        .children((banner == status::Banner::Exited).then(|| {
          Button::new(ElementId::from("session-banner-close"))
            .small()
            .label("Close session")
            .on_click(move |_, _, cx| close_model.update(cx, |model, cx| model.close(live, cx)))
        }))
    });
    let column = v_flex()
      .flex_1()
      .min_w_0()
      .h_full()
      .children(banner)
      .child(transcript::transcript(&self.model, self.live, &self.scroller))
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(self.live)
          .and_then(|view| queue::view(&self.model, self.live, &view.queue, cx)),
      )
      .children(dialogs::view(&self.dialogs, &self.model, live, cx))
      .child(
        h_flex()
          .px_4()
          .gap_2()
          .child(
            div()
              .id("session-status")
              .flex_1()
              .text_sm()
              .text_color(cx.theme().muted_foreground)
              .role(gpui_kit::Role::Status)
              .aria_label(line.clone())
              .child(line),
          )
          .child(
            Button::new(ElementId::from("session-menu-toggle"))
              .small()
              .ghost()
              .selected(self.menu_open)
              .label("Session")
              .on_click({
                let menu_view = cx.entity();
                move |_, _, cx| {
                  menu_view.update(cx, |view, cx| {
                    view.menu_open = !view.menu_open;
                    if view.menu_open {
                      let live = view.live;
                      view.model.update(cx, |model, cx| model.refresh_stats(live, cx));
                    }
                    cx.notify();
                  });
                }
              }),
          )
          .child(
            Button::new(ElementId::from("history-toggle"))
              .small()
              .ghost()
              .selected(self.history_open)
              .label("History")
              .on_click(move |_, _, cx| {
                toggle_view.update(cx, |view, cx| {
                  view.history_open = !view.history_open;
                  cx.notify();
                });
              }),
          ),
      )
      .children(self.interrupt_button(cx))
      .children(self.menu_open.then(|| self.session_menu(cx)))
      .children(self.command_completion(cx))
      .child(self.pickers.view(&self.model, live))
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(live)
          .and_then(|view| status::widgets(view, WidgetPlacement::AboveEditor, cx)),
      )
      .child(div().p_2().child(composer::view(&self.composer)))
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(live)
          .and_then(|view| status::widgets(view, WidgetPlacement::BelowEditor, cx)),
      );
    h_flex()
      .key_context(SESSION_CONTEXT)
      .on_action(cx.listener(move |this, _: &Abort, _, cx| {
        this.model.update(cx, |model, cx| model.abort(live, cx));
      }))
      .size_full()
      .child(column)
      .children(self.history_open.then(|| self.history.clone()))
  }
}

impl SessionView {
  /// Types into the composer.
  #[cfg(test)]
  pub(crate) fn type_composer(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    self
      .composer
      .update(cx, |composer, cx| composer.set_value(text.to_owned(), window, cx));
  }

  /// Types into the session menu's name field.
  #[cfg(test)]
  pub(crate) fn type_session_name(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    self.name.update(cx, |name, cx| name.set_value(text.to_owned(), window, cx));
  }

  /// Types into the history panel's label field.
  #[cfg(test)]
  pub(crate) fn type_history_label(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
    self.history.update(cx, |history, cx| history.type_label(text, window, cx));
  }

  /// Selects a history row, as a click on it does.
  #[cfg(test)]
  pub(crate) fn select_history_row(&self, id: alvin_protocol::EntryId, cx: &mut Context<Self>) {
    self.history.update(cx, |history, cx| history.select(id, cx));
  }

  /// The rows the history panel lists, whether or not it is open.
  #[cfg(test)]
  pub(crate) fn history_row_list(&self, cx: &gpui_kit::App) -> Vec<crate::history_tree::Row> {
    self.history.read(cx).rows().to_vec()
  }
}
