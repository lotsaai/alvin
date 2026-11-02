//! One live session: transcript, status line and its composer.

use std::cell::Cell;

use alvin_protocol::{LiveSessionId, WidgetPlacement};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogFooter, DialogHeader, DialogTitle};
use gpui_kit::component::input::{EditorState, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  AnyElement, App, AppContext as _, Context, ElementId, Entity, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _,
  Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div,
};

use crate::actions::{Abort, SESSION_CONTEXT};
use crate::dialogs::Dialogs;
use crate::history_tree::HistoryTreeView;
use crate::model::{Model, ModelEvent};
use crate::pickers::Pickers;
use crate::status::Banner;
use crate::{composer, queue, status, transcript};

/// A live session's view. Holds only local view state: scroll position, the composer and how many rows the scroller was
/// last told about. Everything else is read from the model.
pub(crate) struct SessionView {
  live: LiveSessionId,
  model: Entity<Model>,
  scroller: Entity<MessageScrollerState>,
  composer: Entity<EditorState>,
  pickers: Pickers,
  dialogs: Dialogs,
  /// The history panel, whether it is open.
  history: Entity<HistoryTreeView>,
  history_open: bool,
  /// The name typed into the rename dialog.
  name: Entity<InputState>,
  rows: usize,
  _subscriptions: Vec<Subscription>,
}

impl SessionView {
  /// An element id of this session: the same control in two tabs never shares one.
  fn id(&self, part: &str) -> ElementId {
    ElementId::from(SharedString::from(format!("{part}-{}", self.live.as_uuid())))
  }

  /// While a turn runs, a button aborts it and sends what the composer holds in its place.
  fn interrupt_button(&self, cx: &App) -> Option<AnyElement> {
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
          Button::new(self.id("session-interrupt"))
            .small()
            .ghost()
            .label("Interrupt and send")
            .on_click(move |_, window, cx| {
              let text = composer.read(cx).value().trim().to_owned();
              if text.is_empty() {
                return;
              }
              composer.update(cx, |composer, cx| composer.set_value("", window, cx));
              model.update(cx, |model, cx| model.interrupt_and_send(live, text, cx)).detach();
            }),
        )
        .into_any_element(),
    )
  }

  /// Sends what the composer holds, as Enter does.
  fn send_button(&self) -> impl IntoElement {
    let (model, composer, live) = (self.model.clone(), self.composer.clone(), self.live);
    Button::new(self.id("session-send"))
      .primary()
      .label("Send")
      .on_click(move |_, window, cx| {
        let text = composer.read(cx).value().trim().to_owned();
        if text.is_empty() {
          return;
        }
        composer.update(cx, |composer, cx| composer.set_value("", window, cx));
        model.update(cx, |model, cx| model.send(live, &text, false, cx)).detach();
      })
  }

  /// The session menu: rename, compact, export and the counts omp gives.
  fn session_menu(&self) -> impl IntoElement {
    let live = self.live;
    let model = self.model.clone();
    let name = self.name.clone();
    Button::new(self.id("session-menu"))
      .small()
      .ghost()
      .icon(IconName::Ellipsis)
      .accessibility_label("Session menu")
      .dropdown_menu(move |menu, _, cx| {
        let stats = model.read(cx).stats(live).map(|stats| {
          format!(
            "{} messages, {} tool calls, {} tokens",
            stats.total_messages, stats.tool_calls, stats.tokens.total
          )
        });
        let command = |label: &'static str, run: fn(&mut Model, LiveSessionId, &mut Context<Model>) -> Task<()>| {
          let model = model.clone();
          PopupMenuItem::new(label).on_click(move |_, _, cx| model.update(cx, |model, cx| run(model, live, cx)).detach())
        };
        let rename = {
          let (model, name) = (model.clone(), name.clone());
          PopupMenuItem::new("Rename…").on_click(move |_, window, cx| open_rename(&model, &name, live, window, cx))
        };
        menu
          .item(rename)
          .item(command("Compact", Model::compact))
          .item(command("Export as HTML", Model::export_html))
          .separator()
          .item(command("Refresh statistics", Model::refresh_stats))
          .when_some(stats, |menu, text| menu.label(SharedString::from(text)))
      })
  }

  pub(crate) fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let composer = composer::state(&model, live, window, cx);
    let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
    let pickers = Pickers::new(&model, live, window, cx);
    let dialogs = Dialogs::new();
    let history = cx.new(|cx| HistoryTreeView::new(model.clone(), live, window, cx));
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Session name"));
    let subscriptions = vec![
      cx.observe(&scroller, |_, _, cx| cx.notify()),
      cx.observe_in(&model, window, |this, model, window, cx| {
        this.sync_rows(cx);
        this.pickers.sync(&model, this.live, window, cx);
        this.dialogs.sync(&model, this.live, window, cx);
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
            .update(cx, |model, cx| model.send(this.live, &submission.text, submission.follow_up, cx))
            .detach();
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
      name,
      rows: 0,
      _subscriptions: subscriptions,
    };
    view.sync_rows(cx);
    view
  }

  /// Tells the scroller how the transcript changed since it last looked: rows appended, transcript replaced, or the
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

  /// One line describing what the session is doing, with the statuses extensions set.
  fn status_line(&self, cx: &App) -> SharedString {
    let model = self.model.read(cx);
    let view = model.conversation().live(self.live);
    let mut line: SharedString = match view {
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
    if let Some(statuses) = view.and_then(status::statuses) {
      line = format!("{line} · {statuses}").into();
    }
    line
  }

  /// The alert above the transcript while the session has no live process or is reading its history again.
  fn banner(&self, cx: &App) -> Option<impl IntoElement> {
    let live = self.live;
    let banner = self.model.read(cx).conversation().live(live).and_then(status::banner)?;
    let model = self.model.clone();
    let alert = Alert::warning(self.id("session-banner-alert"), banner.text()).banner();
    let alert = alert.when(banner == Banner::Exited, |alert| {
      alert.on_close(move |_, _, cx| model.update(cx, |model, cx| model.close(live, cx)).detach())
    });
    Some(
      div()
        .id(self.id("session-banner"))
        .test_support()
        .role(gpui_kit::Role::Status)
        .aria_label(banner.text())
        .child(alert),
    )
  }

  /// The row under the transcript: status line, session menu and the history toggle.
  fn status_row(&self, cx: &Context<Self>) -> impl IntoElement {
    let (text, highlights) = crate::ansi::parse(&self.status_line(cx));
    let line = SharedString::from(text);
    let toggle_view = cx.entity();
    h_flex()
      .px_4()
      .gap_2()
      .child(
        div()
          .id(self.id("session-status"))
          .test_support()
          .flex_1()
          .text_sm()
          .text_color(cx.theme().muted_foreground)
          .role(gpui_kit::Role::Status)
          .aria_label(line.clone())
          .child(gpui_kit::StyledText::new(line).with_highlights(highlights)),
      )
      .child(self.session_menu())
      .child(
        Button::new(self.id("history-toggle"))
          .small()
          .ghost()
          .label("History")
          .selected(self.history_open)
          .on_click(move |_, _, cx| {
            toggle_view.update(cx, |view, cx| {
              view.history_open = !view.history_open;
              cx.notify();
            });
          }),
      )
  }
}

/// Opens the rename dialog, with the session's current title typed in.
fn open_rename(model: &Entity<Model>, name: &Entity<InputState>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
  let title = model
    .read(cx)
    .conversation()
    .live(live)
    .and_then(|view| view.title.clone())
    .unwrap_or_default();
  name.update(cx, |name, cx| name.set_value(title, window, cx));
  // The builder runs again whenever the dialog layer is rebuilt; only the first run takes focus.
  let initial_focus = Cell::new(true);
  let (model, name) = (model.clone(), name.clone());
  window.open_dialog(cx, move |dialog, window, cx| {
    if initial_focus.replace(false) {
      name.focus_handle(cx).focus(window, cx);
    }
    let (model, name) = (model.clone(), name.clone());
    dialog.content(move |content, _, _| {
      let (model, name) = (model.clone(), name.clone());
      content
        .child(DialogHeader::new().child(DialogTitle::new().child("Rename session")))
        .child(Input::new(&name).accessibility_id("session-name").aria_label("Session name"))
        .child(
          DialogFooter::new()
            .child(
              Button::new("rename-cancel")
                .outline()
                .label("Cancel")
                .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(Button::new("rename-confirm").primary().label("Rename").on_click({
              let name = name.clone();
              move |_, window, cx| {
                let text = name.read(cx).value().trim().to_owned();
                if text.is_empty() {
                  return;
                }
                model.update(cx, |model, cx| model.rename(live, text, cx)).detach();
                window.close_dialog(cx);
              }
            })),
        )
    })
  });
}

impl Render for SessionView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let live = self.live;
    let widgets = |placement| {
      self
        .model
        .read(cx)
        .conversation()
        .live(live)
        .and_then(|view| status::widgets(view, placement, cx))
    };
    let column = v_flex()
      .flex_1()
      .min_w_0()
      .h_full()
      .children(self.banner(cx))
      .child(transcript::transcript(&self.model, live, &self.scroller))
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(live)
          .and_then(|view| queue::view(&self.model, live, &view.queue, cx)),
      )
      .child(self.status_row(cx))
      .children(self.interrupt_button(cx))
      .child(self.pickers.view(&self.model, live))
      .children(widgets(WidgetPlacement::AboveEditor))
      .child(
        h_flex()
          .p_2()
          .gap_2()
          .items_end()
          .child(div().flex_1().min_w_0().child(composer::view(&self.composer, window, cx)))
          .child(self.send_button()),
      )
      .children(widgets(WidgetPlacement::BelowEditor));
    h_flex()
      .id(self.id("session"))
      .key_context(SESSION_CONTEXT)
      .on_action(cx.listener(move |this, _: &Abort, _, cx| {
        this.model.update(cx, |model, cx| model.abort(live, cx)).detach();
      }))
      .items_stretch()
      .size_full()
      .child(column)
      .children(self.history_open.then(|| self.history.clone()))
  }
}

/// Test-only access, so tests read the composer without widening visibility.
#[cfg(test)]
impl SessionView {
  /// What the composer holds.
  pub(crate) fn composer_text(&self, cx: &App) -> String {
    self.composer.read(cx).value().to_string()
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use alvin_protocol::LiveSessionSummary;
  use gpui_kit::test::{TestAppContextExt as _, TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
  use gpui_kit::{AppContext as _, TestAppContext};

  use crate::test_support::{Opened, event, live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  /// An open session in a window, over a gateway that answers nothing: a command the view sends is never answered.
  async fn opened_session(cx: &mut TestAppContext) -> (Opened, LiveSessionSummary) {
    let opened = open_workspace(cx).await;
    let session = live_session("/work/api");
    cx.update(|cx| opened.model.update(cx, |model, cx| model.seed_open(&session, cx)));
    let suffix = session.live.as_uuid().to_string();
    cx.wait_for(opened.handle, BUDGET, move |window, _| {
      window.try_find(format!("session-send-{suffix}")).is_some()
    })
    .await;
    (opened, session)
  }

  fn feed(cx: &mut TestAppContext, opened: &Opened, session: &LiveSessionSummary, body: serde_json::Value) {
    cx.update(|cx| {
      opened
        .model
        .update(cx, |model, cx| model.seed_event(&event(session.live, body), cx))
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_status_line_follows_the_run_and_the_session_shows_a_send_button(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      let id = session.live.as_uuid();
      cx.update_window(opened.handle, |_, window, _| {
        assert_eq!(window.find(format!("session-status-{id}")).label(), Some("idle"));
        assert_eq!(window.find(format!("session-send-{id}")).label(), Some("Send"));
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_process_exiting_shows_a_banner_that_closes_the_session(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      let banner = format!("session-banner-{}", session.live.as_uuid());
      cx.update_window(opened.handle, |_, window, _| assert!(window.try_find(banner.clone()).is_none()))
        .unwrap();

      feed(cx, &opened, &session, serde_json::json!({ "kind": "processExited" }));
      cx.wait_for(opened.handle, BUDGET, {
        let banner = banner.clone();
        move |window, _| window.try_find(banner.clone()).is_some()
      })
      .await;
      cx.update_window(opened.handle, |_, window, _| {
        assert_eq!(window.find(banner).label(), Some("omp exited"));
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_menu_lists_the_session_commands_and_rename_opens_a_dialog_with_the_title(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      let menu = format!("session-menu-{}", session.live.as_uuid());
      cx.update_window(opened.handle, |_, window, cx| window.click(menu.clone(), cx))
        .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("Compact").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        for label in ["Rename…", "Compact", "Export as HTML", "Refresh statistics"] {
          assert!(window.find_by_label(label).visible(), "{label} is in the menu");
        }
        let rename = window.find_by_label("Rename…").bounds().center();
        window.pointer_down(rename, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(rename, gpui_kit::MouseButton::Left, cx);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("rename-confirm").is_some())
        .await;

      // Cancel closes the dialog without renaming.
      cx.update_window(opened.handle, |_, window, cx| window.click("rename-cancel", cx))
        .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("rename-confirm").is_none())
        .await;
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_history_button_opens_and_closes_the_panel(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      let id = session.live.as_uuid();
      cx.update_window(opened.handle, |_, window, cx| {
        assert!(window.try_find("history-tree").is_none());
        window.click(format!("history-toggle-{id}"), cx);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("history-tree").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        assert!(window.try_find("history-tree").is_some());
        window.click(format!("history-toggle-{id}"), cx);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("history-tree").is_none())
        .await;
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn queued_messages_show_with_their_actions_and_sent_steering_has_none(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      feed(
        cx,
        &opened,
        &session,
        serde_json::json!({ "kind": "agent", "frame": {
          "type": "queue_update",
          "steering": ["already sent", "take it back"],
          "followUp": ["after the turn"],
          "liveSteered": 1
        } }),
      );
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("queue").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        let queued = opened.model.read(cx).conversation().live(session.live).unwrap().queue.clone();
        assert_eq!(queued.live_steered, 1);
        let (sent, taken, after) = (&queued.steering[0], &queued.steering[1], &queued.follow_up[0]);
        for row in [
          format!("queue-steering-{}", sent.id.as_uuid()),
          format!("queue-steering-{}", taken.id.as_uuid()),
          format!("queue-follow-up-{}", after.id.as_uuid()),
        ] {
          assert!(window.try_find(row.clone()).is_some(), "{row} is listed");
        }
        // omp can no longer take back the steering entry it already sent, so only the other two rows have Remove.
        assert!(window.try_find(format!("queue-remove-steering-{}", sent.id.as_uuid())).is_none());
        assert!(window.try_find(format!("queue-remove-steering-{}", taken.id.as_uuid())).is_some());
        assert!(window.try_find(format!("queue-remove-follow-up-{}", after.id.as_uuid())).is_some());
        assert!(window.try_find(format!("queue-promote-{}", after.id.as_uuid())).is_some());
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn a_slash_offers_omps_commands_and_enter_picks_one_instead_of_sending(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (opened, session) = opened_session(cx).await;
      feed(
        cx,
        &opened,
        &session,
        serde_json::json!({ "kind": "availableCommandsUpdate", "frame": { "commands": [
          { "name": "model", "source": "builtin", "description": "Pick a model" },
          { "name": "compact", "source": "builtin", "description": "Shrink the session" }
        ] } }),
      );
      cx.update_window(opened.handle, |_, window, cx| {
        let composer = window.find_by_label("Message").bounds().center();
        window.pointer_down(composer, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(composer, gpui_kit::MouseButton::Left, cx);
        window.input("/mo", cx);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("/model").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        assert!(
          window.try_find_by_label("/compact").is_none(),
          "only commands that start with the typed word"
        );
        window.press("enter", cx);
      })
      .unwrap();
      let (workspace, live) = (opened.workspace.clone(), session.live);
      cx.wait_for(opened.handle, BUDGET, move |_, cx| {
        workspace
          .read(cx)
          .session(live)
          .is_some_and(|view| view.read(cx).composer_text(cx) == "/model ")
      })
      .await;
    });
  }
}
