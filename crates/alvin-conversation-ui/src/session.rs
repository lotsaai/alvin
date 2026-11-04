//! One live session: transcript, status line and its composer.

mod ansi;
mod ask;
mod composer;
mod dialogs;
mod history_tree;
mod markdown;
mod message_chrome;
mod pickers;
mod queue;
mod status;
mod timeline;
mod transcript;

pub use markdown::refresh_markdown_theme;
pub(crate) use timeline::init as init_timeline;
pub use transcript::UserMessageColors;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use alvin_protocol::{LiveSessionId, WidgetPlacement};
use gpui_kit::base::StyledExt as _;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogFooter, DialogHeader, DialogTitle};
use gpui_kit::component::input::{EditorState, Enter, Escape, Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  Anchor, AnyElement, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement,
  ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, canvas, div,
};

use crate::actions::{Abort, SESSION_CONTEXT};
use crate::model::{Model, ModelChange, ModelEvent};
use dialogs::Dialogs;
use history_tree::HistoryTreeView;
use pickers::Pickers;
use status::Banner;
use timeline::ConversationTimeline;

/// Keep the transcript's comfortable text lane when history is shown alongside it.
const HISTORY_INLINE_MIN_WIDTH_REM: f32 = 80.;

// LobeHub's 960px lane at Alvin's 14px default font, scaled with application zoom.
const READING_WIDTH_REM: f32 = 960. / 14.;

#[derive(Default)]
struct WideScreen(bool);

impl gpui_kit::Global for WideScreen {}

fn reading_lane(wide: bool) -> gpui_kit::Div {
  div()
    .w_full()
    .min_w_0()
    .when(!wide, |lane| lane.max_w(gpui_kit::rems(READING_WIDTH_REM)))
}

/// A live session's view. Holds only local view state: scroll position, the composer and how many rows the scroller was
/// last told about. Everything else is read from the model.
pub(crate) struct SessionView {
  live: LiveSessionId,
  model: Entity<Model>,
  scroller: Entity<MessageScrollerState>,
  composer: Entity<EditorState>,
  composer_layout: composer::Layout,
  timeline: Entity<ConversationTimeline>,
  pickers: Pickers,
  dialogs: Dialogs,
  /// The history panel, whether it is open.
  history: Entity<HistoryTreeView>,
  history_open: bool,
  history_inline: bool,
  history_focus: FocusHandle,
  /// The name typed into the rename dialog.
  name: Entity<InputState>,
  rows: Rc<Vec<transcript::Row>>,
  transcript: transcript::Transcript,
  _subscriptions: Vec<Subscription>,
}

impl SessionView {
  /// An element id of this session: the same control in two tabs never shares one.
  fn id(&self, part: &str) -> ElementId {
    ElementId::from(SharedString::from(format!("{part}-{}", self.live.as_uuid())))
  }

  fn toggle_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.composer_layout = match self.composer_layout {
      composer::Layout::Compact => composer::Layout::Expanded,
      composer::Layout::Expanded => composer::Layout::Compact,
    };
    self.composer.update(cx, |state, cx| {
      state.set_submit_on_enter(self.composer_layout == composer::Layout::Compact, cx);
      state.focus(window, cx);
    });
    cx.notify();
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
    if !running || self.composer.read(cx).value().trim().is_empty() {
      return None;
    }
    let (model, composer) = (self.model.clone(), self.composer.clone());
    Some(
      div()
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

  /// Sends the draft, or stops the running operation when there is no draft.
  fn send_button(&self, cx: &App) -> AnyElement {
    let (model, composer, live) = (self.model.clone(), self.composer.clone(), self.live);
    let running = model.read(cx).bash_running(live)
      || model
        .read(cx)
        .conversation()
        .live(live)
        .is_some_and(|view| view.run == alvin_conversation::Run::Running);
    let empty = composer.read(cx).value().trim().is_empty();
    if running && empty {
      let queued = !model.read(cx).bash_running(live)
        && model
          .read(cx)
          .conversation()
          .live(live)
          .is_some_and(|view| !view.queue.steering.is_empty() || !view.queue.follow_up.is_empty());
      return Button::new(self.id("session-stop"))
        .small()
        .label(if queued { "Restore queue" } else { "Stop" })
        .on_click(move |_, _, cx| model.update(cx, |model, cx| model.abort(live, cx)).detach())
        .into_any_element();
    }
    Button::new(self.id("session-send"))
      .small()
      .primary()
      .label("Send")
      .disabled(empty)
      .on_click(move |_, window, cx| {
        let text = composer.read(cx).value().trim().to_owned();
        if text.is_empty() {
          return;
        }
        composer.update(cx, |composer, cx| composer.set_value("", window, cx));
        model.update(cx, |model, cx| model.send(live, &text, false, cx)).detach();
      })
      .into_any_element()
  }

  /// The session menu: rename, compact, export and the counts omp gives.
  pub(crate) fn session_menu(&self) -> impl IntoElement {
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
          .item(command("Next model", Model::cycle_model))
          .item(command("Next thinking", Model::cycle_thinking))
          .when_some(stats, |menu, text| menu.label(SharedString::from(text)))
      })
  }

  pub(crate) fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let composer = composer::state(&model, live, window, cx);
    let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
    let timeline = cx.new(|cx| ConversationTimeline::new(scroller.clone(), window, cx));
    let pickers = Pickers::new(&model, live, window, cx);
    let dialogs = Dialogs::new();
    let history = cx.new(|cx| HistoryTreeView::new(model.clone(), live, window, cx));
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Session name"));
    let subscriptions = vec![
      cx.observe_global::<WideScreen>(|this, cx| {
        this.scroller.update(cx, |scroller, cx| scroller.remeasure(cx));
        cx.notify();
      }),
      cx.observe(&scroller, |_, _, cx| cx.notify()),
      cx.observe(&timeline, |_, _, cx| cx.notify()),
      cx.subscribe_in(&model, window, |this, model, change: &ModelChange, window, cx| {
        if matches!(change, ModelChange::Session(live) if *live == this.live) {
          this.sync_rows(window, cx);
          this.pickers.sync(model, this.live, window, cx);
          this.dialogs.sync(model, this.live, window, cx);
          cx.notify();
        }
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
        if let Some(submission) = composer::submitted(event, state, this.composer_layout, cx) {
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
      composer_layout: composer::Layout::Compact,
      timeline,
      pickers,
      dialogs,
      history,
      history_open: false,
      history_inline: false,
      history_focus: cx.focus_handle().tab_stop(true),
      name,
      rows: Rc::new(Vec::new()),
      transcript: transcript::Transcript::default(),
      _subscriptions: subscriptions,
    };
    view.sync_rows(window, cx);
    view.pickers.sync(&view.model, live, window, cx);
    view.dialogs.sync(&view.model, live, window, cx);
    view
  }

  /// Tells the scroller how the transcript changed since it last looked: rows appended, transcript replaced, or the
  /// streaming row grew taller.
  fn sync_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(view) = self.model.read(cx).conversation().live(self.live) else {
      self.transcript = transcript::Transcript::default();
      self.rows = Rc::new(Vec::new());
      self.scroller.update(cx, |scroller, cx| scroller.reset(0, cx));
      return;
    };
    let rows = Rc::new(transcript::rows(view));
    let reset = self.transcript.generation() != view.snapshot_generation;
    let prepared = self.transcript.prepare(view);
    let changed = prepared.changed();
    self.transcript.apply(prepared, &self.composer, &self.scroller, window, cx);
    let previous = self.rows.len();
    let count = rows.len();
    self.rows = rows;
    self
      .timeline
      .update(cx, |timeline, cx| timeline.sync(self.live, &self.model, &self.rows, cx));
    if reset || count != previous || changed {
      self.scroller.update(cx, |scroller, cx| {
        if reset {
          scroller.reset(count, cx);
        } else if count > previous {
          scroller.append(count - previous, cx);
        } else if count < previous {
          scroller.reset(count, cx);
        }
        if count > 0 && changed {
          scroller.remeasure(cx);
        }
      });
    }
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

  /// The session status, including extension-provided entries.
  fn status_row(&self, cx: &Context<Self>) -> impl IntoElement {
    let (text, highlights) = ansi::parse(&self.status_line(cx));
    let line = SharedString::from(text);
    h_flex().px_4().gap_2().child(
      div()
        .id(self.id("session-status"))
        .test_support()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .role(gpui_kit::Role::Status)
        .aria_label(line.clone())
        .child(gpui_kit::StyledText::new(line).with_highlights(highlights)),
    )
  }

  fn session_header(&self, cx: &Context<Self>) -> impl IntoElement {
    let history = self.history.clone();
    h_flex()
      .px_4()
      .gap_2()
      .justify_end()
      .child(
        Button::new(self.id("widescreen-toggle"))
          .xsmall()
          .ghost()
          .label("Widescreen")
          .selected(cx.try_global::<WideScreen>().is_some_and(|wide| wide.0))
          .toggled(cx.try_global::<WideScreen>().is_some_and(|wide| wide.0))
          .on_click(cx.listener(|_, _, _, cx| {
            let wide = !cx.try_global::<WideScreen>().is_some_and(|wide| wide.0);
            cx.set_global(WideScreen(wide));
            cx.refresh_windows();
          })),
      )
      .when(self.composer_layout == composer::Layout::Compact, |header| {
        header.children(self.timeline.read(cx).compact_menu(&self.timeline))
      })
      .child(
        Popover::new(self.id("history-popover"))
          .anchor(Anchor::TopRight)
          // Keep the same native popup state across width changes so it closes cleanly.
          .open(self.history_open && !self.history_inline)
          .trigger(
            Button::new(self.id("history-toggle"))
              .xsmall()
              .ghost()
              .label("History")
              .selected(self.history_open)
              .track_focus(&self.history_focus),
          )
          .on_open_change(cx.listener(|this, open, window, cx| {
            this.history_open = if this.history_inline { !this.history_open } else { *open };
            if this.history_inline || !*open {
              this.history_focus.focus(window, cx);
            }
            cx.notify();
          }))
          .content(move |_, _, _| {
            div()
              .id("history-overlay")
              .test_support()
              .w(gpui_kit::rems(20.))
              .h(gpui_kit::rems(28.))
              .child(history.clone())
          }),
      )
  }

  /// Session context alongside the transcript, derived from the same live state as the controls.
  fn context_panel(&self, cx: &App) -> impl IntoElement + use<> {
    let model = self.model.read(cx);
    let view = model.conversation().live(self.live);
    let mut panel = v_flex()
      .id(self.id("session-context"))
      .test_support()
      .w(gpui_kit::rems(18.))
      .max_h(gpui_kit::rems(24.))
      .overflow_y_scroll()
      .min_w_0()
      .gap_4()
      .child(div().font_semibold().child("Session"));
    if let Some(view) = view {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Working directory"))
          .child(div().child(SharedString::from(view.cwd.display().to_string()))),
      );
      if let Some(model) = &view.model {
        panel = panel.child(
          v_flex()
            .gap_1()
            .child(div().text_color(cx.theme().muted_foreground).child("Model"))
            .child(div().child(SharedString::from(model.name.clone())))
            .child(
              div()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(model.provider.clone())),
            ),
        );
      }
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Messages"))
          .child(div().child(SharedString::from(view.total_messages.to_string()))),
      );
    }
    if let Some(usage) = model.stats(self.live).and_then(|stats| stats.context_usage.as_ref()) {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Last reported context"))
          .child(div().child(SharedString::from(format!(
            "{} of {} tokens · {:.1}%",
            usage.tokens, usage.context_window, usage.percent
          )))),
      );
    }
    if let Some(stats) = model.stats(self.live) {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Usage"))
          .child(div().child(SharedString::from(format!("{} tool calls", stats.tool_calls))))
          .child(div().child(SharedString::from(format!("{} tokens", stats.tokens.total)))),
      );
    }
    panel
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
    let new_session = self.model.read(cx).is_new_session(live);
    let expanded = self.composer_layout == composer::Layout::Expanded;
    let wide = cx.try_global::<WideScreen>().is_some_and(|wide| wide.0);
    let context_label = self
      .model
      .read(cx)
      .conversation()
      .live(live)
      .and_then(|view| view.model.as_ref())
      .and_then(|model| model.context_window)
      .filter(|window| *window > 0)
      .map_or_else(|| "Context window unavailable".to_owned(), |window| format!("{window} tokens"));
    let mut trigger_style = div().w_full();
    let model = self.model.clone();
    let view = cx.entity();
    let context = Popover::new(self.id("context-popover"))
      .anchor(Anchor::BottomLeft)
      .trigger_style(trigger_style.style().clone())
      .trigger(
        Button::new(self.id("context-toggle"))
          .small()
          .ghost()
          .w_full()
          .accessibility_label("Show session context")
          .child(
            h_flex()
              .w_full()
              .justify_between()
              .text_color(cx.theme().muted_foreground)
              .child("Context")
              .child(SharedString::from(context_label)),
          ),
      )
      .on_open_change(move |open, _, cx| {
        if *open {
          model.update(cx, |model, cx| model.refresh_stats(live, cx)).detach();
        }
      })
      .content(move |_, _, cx| view.read(cx).context_panel(cx).into_any_element());
    let dock = reading_lane(wide)
      .id(self.id("composer-lane"))
      .test_support()
      .flex()
      .flex_col()
      .min_h_0()
      .when(expanded, |dock| dock.flex_1())
      .children(
        self
          .model
          .read(cx)
          .conversation()
          .live(live)
          .and_then(|view| queue::view(&self.model, live, &view.queue, cx)),
      )
      .children(widgets(WidgetPlacement::AboveEditor))
      .child(
        v_flex()
          .mb_2()
          .min_w_0()
          .gap_2()
          .min_h_0()
          .when(expanded, |editor| editor.flex_1())
          .rounded(cx.theme().radius)
          .border_1()
          .border_color(cx.theme().border)
          .bg(cx.theme().muted)
          .child(context)
          .child(
            div()
              .min_h_0()
              .when(expanded, |editor| editor.flex_1())
              .capture_action(cx.listener(|this, enter: &Enter, _, cx| {
                // Native Enter still owns completion acceptance and newline insertion.
                this.composer.update(cx, |state, cx| {
                  state.set_submit_on_enter(this.composer_layout == composer::Layout::Compact || enter.secondary, cx);
                });
                cx.propagate();
              }))
              .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.composer_layout == composer::Layout::Expanded {
                  this.toggle_composer(window, cx);
                } else {
                  cx.propagate();
                }
              }))
              .child(composer::view(&self.composer, self.composer_layout, window, cx)),
          )
          .child(
            h_flex()
              .flex_wrap()
              .px_2()
              .pb_2()
              .gap_2()
              .min_w_0()
              .child(self.pickers.view(cx))
              .children(self.interrupt_button(cx))
              .when(expanded, |toolbar| {
                toolbar.child(
                  div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if cfg!(target_os = "macos") {
                      "⌘Enter to send"
                    } else {
                      "Ctrl+Enter to send"
                    }),
                )
              })
              .child(div().flex_1())
              .child(
                Button::new(self.id("composer-expand"))
                  .small()
                  .ghost()
                  .label(if expanded { "Collapse" } else { "Expand" })
                  .accessibility_label(if expanded { "Collapse editor" } else { "Expand editor" })
                  .on_click(cx.listener(|this, _, window, cx| this.toggle_composer(window, cx))),
              )
              .child(self.send_button(cx)),
          ),
      )
      .children(widgets(WidgetPlacement::BelowEditor))
      .child(self.status_row(cx));
    let content = if expanded {
      h_flex()
        .items_stretch()
        .flex_1()
        .min_h_0()
        .w_full()
        .justify_center()
        .px_4()
        .pt_2()
        .child(dock)
        .into_any_element()
    } else if new_session {
      v_flex()
        .flex_1()
        .min_h_0()
        .px_4()
        .py_4()
        .justify_center()
        .items_center()
        .child(
          reading_lane(wide)
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_color(cx.theme().muted_foreground).child("Start a conversation"))
            .child(dock),
        )
        .into_any_element()
    } else {
      v_flex()
        .flex_1()
        .min_h_0()
        .child(
          div()
            .id(self.id("transcript-content"))
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .child(transcript::transcript(
              live,
              &self.scroller,
              self.rows.clone(),
              &self.transcript,
              wide,
            ))
            .child(self.timeline.clone()),
        )
        .child(h_flex().w_full().justify_center().px_4().child(dock))
        .into_any_element()
    };
    let column = v_flex()
      .id(self.id("session-main"))
      .test_support()
      .flex_1()
      .min_w_0()
      .h_full()
      .child(self.session_header(cx))
      .children(self.banner(cx))
      .child(content);
    let history_width = gpui_kit::base::motion::transition(
      self.id("history-motion"),
      if self.history_open && self.history_inline {
        1.0_f32
      } else {
        0.0_f32
      },
      gpui_kit::base::motion::Transition::new(Duration::from_millis(180)),
      window,
      cx,
    );
    let history_width = if self.history_inline { history_width } else { 0. };
    let owner = cx.entity();
    let inline = self.history_inline;
    let measure = canvas(
      move |bounds, window, _| {
        let next = bounds.size.width >= window.rem_size() * HISTORY_INLINE_MIN_WIDTH_REM;
        if next != inline {
          window.on_next_frame(move |window, cx| {
            owner.update(cx, |this, cx| {
              if this.history_inline != next {
                this.history_inline = next;
                if this.history_open {
                  this.history_focus.focus(window, cx);
                }
                cx.notify();
              }
            });
          });
        }
      },
      |_, (), _, _| {},
    )
    .absolute()
    .size_full();
    h_flex()
      .id(self.id("session"))
      .relative()
      .key_context(SESSION_CONTEXT)
      .on_action(cx.listener(move |this, _: &Abort, window, cx| {
        if this.composer_layout == composer::Layout::Expanded {
          this.toggle_composer(window, cx);
        } else {
          this.model.update(cx, |model, cx| model.abort(live, cx)).detach();
        }
      }))
      .items_stretch()
      .size_full()
      .child(column)
      .child(measure)
      .when(history_width > 0., |this| {
        this.child(
          div()
            .w(gpui_kit::rems(20. * history_width))
            .id("history-inline")
            .test_support()
            .h_full()
            .overflow_hidden()
            .child(self.history.clone()),
        )
      })
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
  use gpui_kit::{AnyWindowHandle, AppContext as _, TestAppContext};

  use crate::test_support::{Opened, event, live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  /// Headless draws do not deliver native frame callbacks; drain their actual queue, including nested layout changes.
  fn render_layout_frames(cx: &mut TestAppContext, handle: AnyWindowHandle) {
    while cx
      .update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.simulate_next_frame(cx)
      })
      .unwrap()
      > 0
    {}
  }

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
  async fn editor_and_reading_width_remain_aligned_across_layout_changes(cx: &mut TestAppContext) {
    use std::path::PathBuf;

    use alvin_client::{ChannelPair, Client, Endpoint};
    use alvin_gateway::{Gateway, GatewayConfig, HostConfig};
    use gpui_kit::{Bounds, Focusable as _, WindowBounds, WindowOptions, px, size};

    use super::SessionView;
    use crate::model::Model;

    cx.update(gpui_kit::init);
    cx.update(crate::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let directory = std::env::temp_dir().join(format!("alvin-composer-{}", alvin_protocol::EventId::new().as_uuid()));
    std::fs::create_dir_all(&directory).unwrap();
    let gateway = Gateway::new(
      GatewayConfig::builder()
        .sessions_dir(directory.clone())
        .omp(HostConfig::builder().omp(PathBuf::from("omp")).build())
        .build(),
    );
    let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64).unwrap())))
      .await
      .unwrap();
    let session = live_session(directory.to_str().unwrap());
    let (handle, view, model) = cx.update(|cx| {
      let model = cx.new(|cx| {
        let mut model = Model::new(client, cx);
        model.seed_open(&session, cx);
        model
      });
      let (handle, view) = gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1100.), px(700.))))),
          ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| SessionView::new(model.clone(), session.live, window, cx)),
      )
      .unwrap();
      (handle, view, model)
    });
    let id = session.live.as_uuid();
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let editor = window.find_by_label("Message").bounds();
      window.pointer_down(editor.center(), gpui_kit::MouseButton::Left, cx);
      window.pointer_up(editor.center(), gpui_kit::MouseButton::Left, cx);
      window.input("# Plan", cx);
      window.press("shift-left", cx);
      let state = view.read(cx).composer.clone();
      let selection = state.read(cx).selected_range();
      window.click(format!("composer-expand-{id}"), cx);
      assert_eq!(view.read(cx).composer, state);
      assert_eq!(state.read(cx).selected_range(), selection);
      assert!(state.focus_handle(cx).is_focused(window));
      assert!(window.find_by_label("Message").bounds().size.height > editor.size.height);
      assert!(window.find("model-picker").visible());
      assert_eq!(window.find("model-picker").bounds().size.width, window.rem_size() * 16.);
      assert_eq!(window.find("thinking-picker").bounds().size.width, window.rem_size() * 7.);
      assert!(window.find(format!("session-send-{id}")).visible());
      window.input("N", cx);
      window.press("enter", cx);
      window.input("second line", cx);
      assert_eq!(state.read(cx).value().as_ref(), "# PlaN\nsecond line");
      window.click(format!("composer-expand-{id}"), cx);
      assert_eq!(state.read(cx).value().as_ref(), "# PlaN\nsecond line");
      assert!(state.focus_handle(cx).is_focused(window));
      window.press("secondary-z", cx);
      assert_ne!(state.read(cx).value().as_ref(), "# PlaN\nsecond line");
      window.press("secondary-shift-z", cx);
      assert_eq!(state.read(cx).value().as_ref(), "# PlaN\nsecond line");
      window.press("shift-enter", cx);
      assert_eq!(state.read(cx).value().as_ref(), "# PlaN\nsecond line\n");
      window.click(format!("composer-expand-{id}"), cx);
    })
    .unwrap();
    cx.update(|cx| {
      model.update(cx, |model, cx| {
        model.seed_event(
          &event(
            session.live,
            serde_json::json!({ "kind": "agent", "frame": { "type": "agent_start" } }),
          ),
          cx,
        );
      });
    });
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find(format!("session-status-{id}")).label(), Some("working"));
      window.press("escape", cx);
      assert_eq!(window.find(format!("composer-expand-{id}")).label(), Some("Expand editor"));
      assert_eq!(view.read(cx).composer_text(cx), "# PlaN\nsecond line\n");
      assert_eq!(
        model.read(cx).conversation().live(session.live).unwrap().run,
        alvin_conversation::Run::Running,
      );
      assert!(view.read(cx).composer.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    let (history, composer, selection) = cx.update(|cx| {
      let view = view.read(cx);
      (view.history.clone(), view.composer.clone(), view.composer.read(cx).selected_range())
    });
    cx.update_window(handle, |_, window, cx| {
      let width = window.find(format!("session-main-{id}")).bounds().size.width;
      window.click(format!("history-toggle-{id}"), cx);
      assert!(window.find("history-overlay").visible());
      assert!(gpui_kit::base::GlobalState::is_in_deferred_context(cx));
      assert!(window.try_find("history-inline").is_none());
      assert_eq!(window.find(format!("session-main-{id}")).bounds().size.width, width);
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(1500.), px(700.)));
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.find("history-inline").visible());
      assert!(window.try_find("history-overlay").is_none());
      assert!(!gpui_kit::base::GlobalState::is_in_deferred_context(cx));
      assert_eq!(view.read(cx).history, history);
      assert_eq!(view.read(cx).composer, composer);
      assert_eq!(composer.read(cx).selected_range(), selection);
      assert_eq!(composer.read(cx).value().as_ref(), "# PlaN\nsecond line\n");
      assert!(view.read(cx).history_focus.is_focused(window));
      let main = window.find(format!("session-main-{id}")).bounds();
      let history = window.find("history-inline").bounds();
      assert_eq!(history.size.width, window.rem_size() * 20.);
      assert_eq!(main.size.width + history.size.width, window.viewport_size().width);
      assert!(main.size.width >= window.rem_size() * 60.);
      assert!(main.right() <= history.left());
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(900.), px(700.)));
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.find("history-overlay").visible());
      assert!(gpui_kit::base::GlobalState::is_in_deferred_context(cx));
      assert!(window.try_find("history-inline").is_none());
      assert_eq!(view.read(cx).history, history);
      assert_eq!(view.read(cx).composer, composer);
      assert_eq!(composer.read(cx).selected_range(), selection);
      assert_eq!(composer.read(cx).value().as_ref(), "# PlaN\nsecond line\n");
      assert_eq!(
        window.find(format!("session-main-{id}")).bounds().size.width,
        window.viewport_size().width,
      );
      let overlay = window.find("history-overlay").bounds();
      assert_eq!(overlay.size.width, window.rem_size() * 20.);
      assert!(overlay.left() >= gpui_kit::px(0.));
      assert!(overlay.right() <= window.viewport_size().width);
      window.press("escape", cx);
      assert!(window.try_find("history-overlay").is_none());
      assert!(!view.read(cx).history_open);
      assert!(!gpui_kit::base::GlobalState::is_in_deferred_context(cx));
      assert!(view.read(cx).history_focus.is_focused(window));
      assert_eq!(
        model.read(cx).conversation().live(session.live).unwrap().run,
        alvin_conversation::Run::Running,
      );
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(480.), px(700.)));
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.click(format!("composer-expand-{id}"), cx);
      assert_eq!(view.read(cx).composer, composer);
      assert_eq!(composer.read(cx).selected_range(), selection);
      assert_eq!(composer.read(cx).value().as_ref(), "# PlaN\nsecond line\n");
      assert!(composer.focus_handle(cx).is_focused(window));
      assert_eq!(window.find("model-picker").bounds().size.width, window.rem_size() * 16.);
      assert_eq!(window.find("thinking-picker").bounds().size.width, window.rem_size() * 7.);
      let main = window.find(format!("session-main-{id}")).bounds();
      for control in [
        "model-picker".to_owned(),
        "thinking-picker".to_owned(),
        format!("session-interrupt-{id}"),
        format!("composer-expand-{id}"),
        format!("session-send-{id}"),
      ] {
        let control = window.find(control);
        assert!(control.visible());
        assert!(control.bounds().left() >= main.left());
        assert!(control.bounds().right() <= main.right());
        assert!(control.bounds().bottom() <= main.bottom());
      }
    })
    .unwrap();
    cx.update(|cx| {
      model.update(cx, |model, cx| {
        model.seed_messages(
          session.live,
          serde_json::from_str(include_str!("../fixtures/tools_turn.messages.json")).unwrap(),
          cx,
        );
      });
    });
    cx.simulate_window_resize(handle, size(px(760.), px(800.)));
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.click(format!("composer-expand-{id}"), cx);
    })
    .unwrap();
    cx.run_until_parked();
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let content = window.find(format!("transcript-content-{id}")).bounds();
      let rem = window.rem_size();
      assert_eq!(
        content.size.width,
        window.viewport_size().width,
        "narrow content {content:?}, rem {rem:?}"
      );
      assert!(content.size.width < rem * 54., "narrow content {content:?}, rem {rem:?}");
      assert!(window.try_find("conversation-timeline").is_none());
      window.click("conversation-timeline-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let query = window.find_all_by_role(gpui_kit::Role::MenuItem).into_iter().next().unwrap();
      window.pointer_down(query.bounds().center(), gpui_kit::MouseButton::Left, cx);
      window.pointer_up(query.bounds().center(), gpui_kit::MouseButton::Left, cx);
    })
    .unwrap();
    cx.run_until_parked();
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let queries = window.find("conversation-timeline-menu").bounds();
      let transcript = window.find(format!("transcript-content-{id}")).bounds();
      let first_id = super::transcript::message_id(model.read(cx).conversation().live(session.live).unwrap(), 0);
      let first_message = window.within(first_id).find_by_label("Copy message").bounds();
      assert!(queries.bottom() <= transcript.top());
      assert!(queries.bottom() <= first_message.top());
      assert!(first_message.top() >= transcript.top());
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(1100.), px(800.)));
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let content = window.find(format!("transcript-content-{id}")).bounds();
      let rem = window.rem_size();
      assert_eq!(
        content.size.width,
        window.viewport_size().width,
        "wide content {content:?}, rem {rem:?}"
      );
      assert!(content.size.width >= rem * 54., "wide content {content:?}, rem {rem:?}");
      assert!(window.find("conversation-timeline").visible());
      assert!(window.try_find("conversation-timeline-menu").is_none());
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(1500.), px(800.)));
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let normal = window.find(format!("composer-lane-{id}")).bounds();
      let first_id = super::transcript::message_id(model.read(cx).conversation().live(session.live).unwrap(), 0);
      let lane_id = format!("{first_id}-reading-lane");
      let row = window.find(lane_id.clone()).bounds();
      assert_eq!(normal.size.width, (window.rem_size() * super::READING_WIDTH_REM).round());
      assert_eq!(normal.left(), row.left());
      assert_eq!(normal.size.width, row.size.width);
      window.click(format!("widescreen-toggle-{id}"), cx);
      let wide = window.find(format!("composer-lane-{id}")).bounds();
      let row = window.find(lane_id).bounds();
      assert!(wide.size.width > normal.size.width);
      assert_eq!(wide.left(), row.left());
      assert_eq!(wide.size.width, row.size.width);
      assert_eq!(window.find(format!("widescreen-toggle-{id}")).checked(), Some(true));
    })
    .unwrap();
    cx.simulate_window_resize(handle, size(px(480.), px(800.)));
    render_layout_frames(cx, handle);
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      let wide = window.find(format!("composer-lane-{id}")).bounds();
      let first_id = super::transcript::message_id(model.read(cx).conversation().live(session.live).unwrap(), 0);
      let lane_id = format!("{first_id}-reading-lane");
      let row = window.find(lane_id.clone()).bounds();
      assert_eq!(wide.left(), row.left());
      assert_eq!(wide.size.width, row.size.width);
      window.click(format!("widescreen-toggle-{id}"), cx);
      let normal = window.find(format!("composer-lane-{id}")).bounds();
      let row = window.find(lane_id).bounds();
      assert_eq!(normal.left(), row.left());
      assert_eq!(normal.size.width, row.size.width);
      assert_eq!(wide, normal);
      assert_eq!(window.find(format!("widescreen-toggle-{id}")).checked(), Some(false));
    })
    .unwrap();
    gateway.shutdown().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
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
        assert!(window.try_find(format!("session-context-{id}")).is_none());
        let editor = window.find_by_label("Message").bounds();
        let model = window.find("model-picker").bounds();
        let send = window.find(format!("session-send-{id}")).bounds();
        assert!(editor.bottom() <= model.top());
        assert!(editor.bottom() <= send.top());
      })
      .unwrap();
      feed(
        cx,
        &opened,
        &session,
        serde_json::json!({ "kind": "agent", "frame": { "type": "agent_start" } }),
      );
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(format!("session-stop-{id}")).label(), Some("Stop"));
        window.click(format!("context-toggle-{id}"), cx);
        assert!(window.find(format!("session-context-{id}")).visible());
        window.press("escape", cx);
        assert!(window.try_find(format!("session-context-{id}")).is_none());
        let editor = window.find_by_label("Message").bounds().center();
        window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
        window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
        window.input("hello", cx);
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
      cx.update(|cx| cx.set_reduce_motion(true));
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
        assert!(window.try_find(format!("session-context-{id}")).is_none());
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
    cx.executor().allow_parking();
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
