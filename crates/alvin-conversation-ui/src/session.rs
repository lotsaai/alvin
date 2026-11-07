//! One live session: transcript, status line and its composer.

mod ansi;
mod ask;
mod composer;
mod dialogs;
pub(crate) mod dictation;
mod focus;
mod history_tree;
mod markdown;
mod message_chrome;
mod notice;
mod pickers;
mod queue;
mod status;
mod timeline;
mod todo;
mod transcript;

pub(crate) use composer::init as init_shortcuts;
pub use markdown::refresh_markdown_theme;
pub(crate) use notice::system_activity_notification;
pub(crate) use timeline::init as init_timeline;
pub use transcript::UserMessageColors;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use alvin_protocol::{LiveSessionId, WidgetPlacement};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogFooter, DialogHeader, DialogTitle};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::speech::SpeechEvent;
use gpui_kit::component::{ActiveTheme as _, IconName, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  Anchor, App, AppContext as _, Context, ElementId, Entity, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement,
  ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, TextRun, Window,
  canvas, div,
};

use crate::actions::{Abort, SESSION_CONTEXT};
use crate::model::{Model, ModelChange};
use dialogs::Dialogs;
use dictation::{DictatedText, Dictation, Phase};
use history_tree::HistoryTreeView;
use status::Banner;
use timeline::ConversationTimeline;

/// Keep the transcript's comfortable text lane when history is shown alongside it.
const HISTORY_INLINE_MIN_WIDTH_REM: f32 = 80.;

const READING_COLUMNS: f32 = 120.;

#[derive(Default)]
struct WideScreen(bool);

impl gpui_kit::Global for WideScreen {}

fn reading_width(window: &mut Window, cx: &App) -> Pixels {
  let theme = cx.theme();
  let cell = window.text_system().shape_line(
    "0".into(),
    theme.mono_font_size,
    &[TextRun {
      len: 1,
      font: gpui_kit::font(theme.mono_font_family.clone()),
      color: theme.foreground,
      background_color: None,
      underline: None,
      strikethrough: None,
    }],
    None,
  );
  cell.width * READING_COLUMNS
}

fn reading_lane(max_width: Option<Pixels>) -> gpui_kit::Div {
  div().w_full().min_w_0().when_some(max_width, |lane, width| lane.max_w(width))
}

/// A live session's view. Holds only local view state: scroll position, the composer and how many rows the scroller was
/// last told about. Everything else is read from the model.
pub(crate) struct SessionView {
  live: LiveSessionId,
  model: Entity<Model>,
  scroller: Entity<MessageScrollerState>,
  composer: Entity<composer::Composer>,
  dictation: Entity<Dictation>,
  dictation_generation: Option<Rc<()>>,
  dictation_edits: composer::DictationEdits,
  timeline: Entity<ConversationTimeline>,
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
  notices: Entity<notice::NoticeList>,
  todo_open: bool,
  _subscriptions: Vec<Subscription>,
}

impl SessionView {
  fn id(&self, part: &str) -> ElementId {
    ElementId::from(SharedString::from(format!("{part}-{}", self.live.as_uuid())))
  }
  pub(crate) fn cancel_dictation(&mut self, cx: &mut Context<Self>) {
    self.dictation.update(cx, |dictation, cx| dictation.cancel(cx));
    self.dictation_generation = None;
    self.dictation_edits = composer::DictationEdits::default();
  }

  fn flush_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if let Some(generation) = &self.dictation_generation {
      if !self.dictation.read(cx).accepts_generation(generation) {
        self.dictation_generation = None;
        self.dictation_edits = composer::DictationEdits::default();
        return;
      }
      let edits = &mut self.dictation_edits;
      let editor = self.composer.read(cx).editor().clone();
      editor.update(cx, |editor, cx| edits.flush(editor, window, cx));
    }
  }

  fn dictation_active(&self, cx: &App) -> bool {
    matches!(
      self.dictation.read(cx).phase(),
      Phase::Preparing(_) | Phase::Capturing | Phase::Transcribing
    )
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
    let dictation = cx.new(Dictation::new);
    Self::with_dictation(model, live, dictation, window, cx)
  }

  fn with_dictation(
    model: Entity<Model>,
    live: LiveSessionId,
    dictation: Entity<Dictation>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let composer = cx.new(|cx| composer::Composer::new(model.clone(), live, dictation.clone(), window, cx));
    let editor = composer.read(cx).editor().clone();
    let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
    let timeline = cx.new(|cx| ConversationTimeline::new(scroller.clone(), window, cx));
    let notices = cx.new(|_| notice::NoticeList::new());
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
      cx.observe(&composer, |_, _, cx| cx.notify()),
      cx.observe(&dictation, |_, _, cx| cx.notify()),
      cx.subscribe_in(&dictation, window, |this, dictation, event: &DictatedText, window, cx| {
        if !dictation.read(cx).accepts_generation(&event.generation) {
          return;
        }
        if this
          .dictation_generation
          .as_ref()
          .is_none_or(|previous| !Rc::ptr_eq(previous, &event.generation))
        {
          this.dictation_edits = composer::DictationEdits::default();
          this.dictation_generation = Some(event.generation.clone());
        }
        this.dictation_edits.push(&event.text);
        this.flush_dictation(window, cx);
      }),
      cx.observe_in(&editor, window, |this, _, window, cx| this.flush_dictation(window, cx)),
      cx.subscribe_in(&dictation, window, |this, _, event: &SpeechEvent, window, cx| {
        if matches!(event, SpeechEvent::Final(_)) {
          let editor = this.composer.read(cx).editor().clone();
          editor.update(cx, |editor, cx| editor.focus(window, cx));
        }
      }),
      cx.subscribe_in(&model, window, |this, model, change: &ModelChange, window, cx| {
        if matches!(change, ModelChange::Session(live) if *live == this.live) {
          this.sync_rows(window, cx);
          this.dialogs.sync(model, this.live, window, cx);
          cx.notify();
        }
      }),
    ];
    let mut view = Self {
      live,
      model,
      scroller,
      composer,
      dictation,
      dictation_generation: None,
      dictation_edits: composer::DictationEdits::default(),
      timeline,
      dialogs,
      history,
      history_open: false,
      history_inline: false,
      history_focus: cx.focus_handle().tab_stop(true),
      name,
      rows: Rc::new(Vec::new()),
      transcript: transcript::Transcript::default(),
      notices,
      todo_open: false,
      _subscriptions: subscriptions,
    };
    view.sync_rows(window, cx);
    view.dialogs.sync(&view.model, live, window, cx);
    view
  }

  /// Tells the scroller how the transcript changed since it last looked: rows appended, transcript replaced, or the
  /// streaming row grew taller.
  fn sync_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(view) = self.model.read(cx).conversation().live(self.live) else {
      self.transcript = transcript::Transcript::default();
      self.rows = Rc::new(Vec::new());
      self.notices.update(cx, |notices, cx| notices.sync(&[], cx));
      self.scroller.update(cx, |scroller, cx| scroller.reset(0, cx));
      return;
    };
    if view.todo_phases.is_empty() {
      self.todo_open = false;
    }
    let rows = Rc::new(transcript::rows(view));
    let reset = self.transcript.generation() != view.snapshot_generation;
    let prepared = self.transcript.prepare(view);
    let notices = view.notices.clone();
    self.notices.update(cx, |list, cx| list.sync(&notices, cx));
    let changed = prepared.changed();
    let editor = self.composer.read(cx).editor().clone();
    self.transcript.apply(prepared, &editor, &self.scroller, window, cx);
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
    let dictation = self.dictation.clone();
    let alert = Alert::warning(self.id("session-banner-alert"), banner.text()).banner();
    let alert = alert.when(banner == Banner::Exited, |alert| {
      alert.on_close(move |_, _, cx| {
        dictation.update(cx, |dictation, cx| dictation.cancel(cx));
        model.update(cx, |model, cx| model.close(live, cx)).detach();
      })
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
      .when(!self.composer.read(cx).expanded(), |header| {
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
    let plan_focus = focus::tab_stop((ElementId::View(cx.entity_id()), "plan-focus"), window, cx);
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
    let expanded = self.composer.read(cx).expanded();
    let wide = cx.try_global::<WideScreen>().is_some_and(|wide| wide.0);
    let max_width = (!wide).then(|| reading_width(window, cx));
    let dock = reading_lane(max_width)
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
      .child(self.notices.clone())
      .children(self.model.read(cx).conversation().live(live).and_then(|view| {
        todo::view(
          &view.todo_phases,
          self.todo_open,
          cx.listener(|this, _, _, cx| {
            this.todo_open = !this.todo_open;
            cx.notify();
          }),
          &plan_focus,
          cx,
        )
      }))
      .children(widgets(WidgetPlacement::AboveEditor))
      .child(
        v_flex()
          .w_full()
          .min_w_0()
          .when(expanded, |container| container.flex_1().min_h_0())
          .capture_action(cx.listener(|this, _: &gpui_kit::component::input::Escape, _, cx| {
            if this.dictation_active(cx) {
              this.cancel_dictation(cx);
            } else {
              cx.propagate();
            }
          }))
          .child(self.composer.clone()),
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
          reading_lane(max_width)
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
            .child(
              h_flex().items_stretch().size_full().child(div().w_10().flex_shrink_0()).child(
                div()
                  .id(self.id("transcript-viewport"))
                  .test_support()
                  .flex_1()
                  .min_w_0()
                  .overflow_x_hidden()
                  .child(transcript::transcript(
                    live,
                    &self.scroller,
                    self.rows.clone(),
                    &self.transcript,
                    max_width,
                  )),
              ),
            )
            .child(self.timeline.clone()),
        )
        .child(
          h_flex()
            .w_full()
            .pl_10()
            .child(h_flex().flex_1().min_w_0().justify_center().px_10().child(dock)),
        )
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
        if this.dictation_active(cx) {
          this.cancel_dictation(cx);
        } else if this.composer.read(cx).expanded() {
          this.composer.update(cx, |composer, cx| composer.toggle(window, cx));
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
    self.composer.read(cx).editor().read(cx).value().to_string()
  }

  pub(crate) fn dictation_active_for_test(&self, cx: &App) -> bool {
    self.dictation_active(cx)
  }

  pub(crate) fn set_dictation_input_for_test(&mut self, input: Rc<dyn gpui_kit::component::speech::AudioInput>, cx: &mut Context<Self>) {
    self.dictation.update(cx, |dictation, _| dictation.set_input(input));
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
  async fn current_plan_updates_in_place_and_clears(cx: &mut TestAppContext) {
    let opened = open_workspace(cx).await;
    let session = live_session("/work/api");
    cx.update(|cx| opened.model.update(cx, |model, cx| model.seed_open(&session, cx)));
    cx.update_window(opened.handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("current-plan-toggle").is_none());
    })
    .unwrap();
    let events: Vec<alvin_protocol::Event> =
      serde_json::from_str(include_str!("../../alvin-conversation/fixtures/real_todo_transitions.events.json")).unwrap();
    for mut event in events {
      event.live = session.live;
      cx.update(|cx| opened.model.update(cx, |model, cx| model.seed_event(&event, cx)));
    }
    cx.update_window(opened.handle, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find_all("current-plan-toggle").len(), 1);
      assert_eq!(window.find("current-plan-toggle").label(), Some("Plan · 3/3 completed"));
      window.click("current-plan-toggle", cx);
      assert!(window.find("current-plan-details").visible());
    })
    .unwrap();
    feed(
      cx,
      &opened,
      &session,
      serde_json::json!({"kind": "agent", "frame": {"type": "todo_auto_clear"}}),
    );
    cx.update_window(opened.handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("current-plan-toggle").is_none());
      assert!(window.try_find("current-plan-details").is_none());
    })
    .unwrap();
  }

  #[test_context::test_context(crate::test_support::RealGateway)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn dictation_streams_into_an_editable_undoable_draft(fixture: &mut crate::test_support::RealGateway, cx: &mut TestAppContext) {
    use std::path::PathBuf;

    use alvin_client::{ChannelPair, Client, Endpoint};
    use alvin_protocol::{EventBody, LiveApiClient, MessagesRequest, ProjectsApiClient, SubscribeRequest};
    use gpui_kit::component::speech::SpeechEvent;
    use gpui_kit::{Focusable as _, WindowOptions};

    use super::SessionView;
    use super::dictation::{Dictation, test_support};
    use crate::model::{Model, ModelEvent};

    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    tokio::task::coop::unconstrained(async {
      executor.block_test(async {
        cx.update(gpui_kit::init);
        cx.update(|cx| {
          crate::init(
            PathBuf::from(std::env::var_os("ALVIN_TEST_SPEECH_CACHE").expect("run tests through mise test")),
            cx,
          );
        });
        let (_preparation_probe, prepared) = cx.update(test_support::watch_preparation);
        let client = Client::connect(Endpoint::Channel(ChannelPair::from(fixture.gateway.connect_channel(64).unwrap())))
          .await
          .unwrap();
        let projects = ProjectsApiClient::list(&client).await.unwrap();
        let project = projects.first().unwrap();
        let model = cx.update(|cx| cx.new(|cx| Model::new(client.clone(), cx)));
        let opening = cx.update(|cx| {
          model.update(cx, |model, cx| {
            model.new_session(project.id.clone(), project.checkouts.first().unwrap().id.clone(), cx)
          })
        });
        opening.await;
        let session = LiveApiClient::list(&client).await.unwrap().into_iter().next().unwrap();
        assert!(
          session
            .file
            .starts_with(fixture.directory.path().canonicalize().unwrap().join("sessions"))
        );
        let mut events = client
          .events(SubscribeRequest {
            live: session.live,
            after: None,
          })
          .await
          .unwrap();
        let (input, delivered) = test_support::file_input();
        let (final_tx, final_rx) = async_channel::bounded(1);
        let (partial_tx, partial_rx) = async_channel::unbounded();
        let (closed_tx, closed_rx) = async_channel::bounded(1);
        let (handle, view) = cx.update(|cx| {
          let (handle, view) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
              let dictation = cx.new(|cx| Dictation::with_input(input, cx));
              let mut view = SessionView::with_dictation(model.clone(), session.live, dictation.clone(), window, cx);
              view._subscriptions.push(cx.subscribe(&model, move |_, _, event: &ModelEvent, _| {
                let result = match event {
                  ModelEvent::Closed(live) if *live == session.live => Ok(()),
                  ModelEvent::Failed { message, .. } => Err(message.clone()),
                  _ => return,
                };
                closed_tx.try_send(result).unwrap();
              }));
              view
                ._subscriptions
                .push(cx.subscribe(&dictation, move |_, _, event: &SpeechEvent, _| {
                  let result = match event {
                    SpeechEvent::Final(_) => Ok(()),
                    SpeechEvent::Error(error) => Err(error.clone()),
                    SpeechEvent::Partial(text) => {
                      partial_tx.try_send(text.clone()).unwrap();
                      return;
                    }
                    SpeechEvent::Started | SpeechEvent::Cancelled => return,
                  };
                  final_tx.try_send(result).unwrap();
                }));
              view
            })
          })
          .unwrap();
          (handle, view)
        });
        let id = session.live.as_uuid();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          let editor = window.find_by_label("Message").bounds().center();
          window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
          window.input("before REPLACE after", cx);
          for _ in 0..6 {
            window.press("left", cx);
          }
          for _ in 0..7 {
            window.press("shift-left", cx);
          }
          window.click(format!("composer-dictation-{id}"), cx);
        })
        .unwrap();
        prepared.recv().await.unwrap().expect("speech model preparation failed");
        tokio::select! {
          result = delivered.recv() => result.expect("file delivery channel closed"),
          result = final_rx.recv() => panic!("speech ended before audio delivery: {:?}", result.unwrap()),
        }
        let partial = partial_rx.recv().await.unwrap();
        assert!(!partial.is_empty());
        cx.update(|cx| {
          let draft = view.read(cx).composer_text(cx);
          assert!(draft.starts_with("before "));
          assert!(draft.ends_with(" after"));
          assert!(!draft.contains("REPLACE"));
          assert_eq!(model.read(cx).conversation().live(session.live).unwrap().total_messages, 0);
        });
        cx.update_window(handle, |_, window, cx| {
          let composer = view.read(cx).composer.read(cx).editor().clone();
          composer.update(cx, |composer, cx| composer.focus(window, cx));
          window.input("[edited]", cx);
          assert!(composer.read(cx).value().contains("[edited]"));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert_eq!(window.find(format!("composer-dictation-{id}")).label(), Some("Stop dictation"));
          window.click(format!("composer-dictation-{id}"), cx);
        })
        .unwrap();
        final_rx.recv().await.unwrap().expect("speech recognition failed");
        let text = cx
          .update_window(handle, |_, window, cx| {
            let composer = view.read(cx).composer.read(cx).editor().clone();
            let text = composer.read(cx).value().to_string();
            assert!(text.starts_with("before "));
            assert!(text.ends_with(" after"));
            assert!(text.to_lowercase().contains("ask not what your country can do for you"));
            assert!(!text.contains("REPLACE"));
            assert!(text.contains("[edited]"));
            assert!(composer.focus_handle(cx).is_focused(window));
            assert_eq!(model.read(cx).conversation().live(session.live).unwrap().total_messages, 0);
            window.press("secondary-z", cx);
            assert_ne!(composer.read(cx).value().as_ref(), text);
            window.press(if cfg!(target_os = "macos") { "cmd-shift-z" } else { "ctrl-y" }, cx);
            assert_eq!(composer.read(cx).value().as_ref(), text);
            text
          })
          .unwrap();
        assert_eq!(LiveApiClient::state(&client, session.live).await.unwrap().message_count, 0);
        for (input, delivered) in [test_support::empty_file_input(), test_support::short_file_input()] {
          cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| view.set_dictation_input_for_test(input, cx));
            window.render_frame(cx);
            assert_eq!(window.find(format!("composer-dictation-{id}")).label(), Some("Start dictation"));
            window.click(format!("composer-dictation-{id}"), cx);
          })
          .unwrap();
          tokio::select! {
            result = delivered.recv() => result.expect("file delivery channel closed"),
            result = final_rx.recv() => panic!("speech ended before audio delivery: {:?}", result.unwrap()),
          }
          cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find(format!("composer-dictation-{id}")).label(), Some("Stop dictation"));
            window.click(format!("composer-dictation-{id}"), cx);
          })
          .unwrap();
          final_rx.recv().await.unwrap().expect("speech recognition failed");
          cx.update(|cx| {
            assert_eq!(view.read(cx).composer_text(cx), text);
            assert_eq!(model.read(cx).conversation().live(session.live).unwrap().total_messages, 0);
          });
          assert_eq!(LiveApiClient::state(&client, session.live).await.unwrap().message_count, 0);
        }
        let (input, delivered) = test_support::file_input();
        cx.update(|cx| view.update(cx, |view, cx| view.set_dictation_input_for_test(input, cx)));
        cx.update_window(handle, |_, window, cx| {
          window.click(format!("session-send-{id}"), cx);
          assert_eq!(view.read(cx).composer_text(cx), "");
        })
        .unwrap();
        loop {
          let event = events.next().await.unwrap().unwrap();
          if matches!(event.body, EventBody::PromptResult(_)) {
            break;
          }
        }
        let page = LiveApiClient::messages(
          &client,
          MessagesRequest {
            live: session.live,
            cursor: None,
            limit: None,
          },
        )
        .await
        .unwrap();
        assert!(format!("{:?}", page.messages).contains(&text), "{page:?}");
        let (started_tx, started_rx) = async_channel::bounded(1);
        let (release_tx, release_rx) = async_channel::bounded(1);
        let (finished_tx, finished_rx) = async_channel::bounded(1);
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          let editor = window.find_by_label("Message").bounds().center();
          window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
          window.input("keep this draft", cx);
          let dictation = view.read(cx).dictation.clone();
          dictation.update(cx, |dictation, _| {
            dictation.gate_inference(test_support::InferenceGate {
              started: started_tx,
              release: release_rx,
              finished: finished_tx,
            });
          });
          window.click(format!("composer-dictation-{id}"), cx);
        })
        .unwrap();
        delivered.recv().await.unwrap();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          window.click(format!("composer-dictation-{id}"), cx);
        })
        .unwrap();
        started_rx.recv().await.unwrap();
        cx.update(|cx| {
          model.update(cx, |model, cx| {
            model.seed_event(
              &event(
                session.live,
                serde_json::json!({ "kind": "availableCommandsUpdate", "frame": { "commands": [
                { "name": "model", "source": "builtin", "description": "Pick a model" }
              ] } }),
              ),
              cx,
            );
          });
        });
        cx.update_window(handle, |_, window, cx| {
          let editor = window.find_by_label("Message").bounds().center();
          window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
          window.press("home", cx);
          window.press("shift-end", cx);
          window.input("/mo", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert_eq!(view.read(cx).composer_text(cx), "/mo");
          assert!(window.find_by_label("/model").visible());
          let editor = window.find_by_label("Message").bounds().center();
          window.pointer_down(editor, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(editor, gpui_kit::MouseButton::Left, cx);
          window.press("escape", cx);
          assert!(!view.read(cx).dictation_active_for_test(cx));
          window.input(" changed", cx);
          assert_eq!(view.read(cx).composer_text(cx), "/mo changed");
        })
        .unwrap();
        release_tx.send(()).await.unwrap();
        finished_rx.recv().await.unwrap();
        cx.run_until_parked();
        cx.update(|cx| assert_eq!(view.read(cx).composer_text(cx), "/mo changed"));
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          window.click(format!("composer-dictation-{id}"), cx);
        })
        .unwrap();
        delivered.recv().await.unwrap();
        cx.update(|cx| {
          model.update(cx, |model, cx| {
            model.seed_event(&event(session.live, serde_json::json!({ "kind": "processExited" })), cx);
          });
        });
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert!(view.read(cx).dictation_active_for_test(cx));
          let draft = view.read(cx).composer_text(cx);
          let banner = window.find(format!("session-banner-{id}")).bounds();
          let close = gpui_kit::point(banner.right() - gpui_kit::px(26.), banner.center().y);
          window.pointer_down(close, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(close, gpui_kit::MouseButton::Left, cx);
          assert!(!view.read(cx).dictation_active_for_test(cx));
          assert_eq!(view.read(cx).composer_text(cx), draft);
        })
        .unwrap();
        closed_rx.recv().await.unwrap().expect("session close failed");
        cx.update(crate::shutdown).await;
      });
    })
    .await;
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
    let speech_cache = tempfile::tempdir().unwrap();
    cx.update(|cx| crate::init(speech_cache.path().to_path_buf(), cx));
    cx.update(|cx| cx.set_reduce_motion(true));
    let directory = tempfile::tempdir().unwrap();
    let gateway = Gateway::new(
      GatewayConfig::builder()
        .sessions_dir(directory.path().to_path_buf())
        .omp(HostConfig::builder().omp(PathBuf::from("omp")).build())
        .build(),
    );
    let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64).unwrap())))
      .await
      .unwrap();
    let session = live_session(directory.path().to_str().unwrap());
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
      let state = view.read(cx).composer.read(cx).editor().clone();
      let selection = state.read(cx).selected_range();
      window.click(format!("composer-expand-{id}"), cx);
      assert_eq!(*view.read(cx).composer.read(cx).editor(), state);
      assert_eq!(state.read(cx).selected_range(), selection);
      assert!(state.focus_handle(cx).is_focused(window));
      assert!(window.find_by_label("Message").bounds().size.height > editor.size.height);
      assert!(window.find("model-picker").visible());
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
      window.press(if cfg!(target_os = "macos") { "cmd-shift-z" } else { "ctrl-y" }, cx);
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
      assert!(view.read(cx).composer.read(cx).editor().focus_handle(cx).is_focused(window));
    })
    .unwrap();
    let (history, composer, selection) = cx.update(|cx| {
      let view = view.read(cx);
      let editor = view.composer.read(cx).editor().clone();
      let selection = editor.read(cx).selected_range();
      (view.history.clone(), editor, selection)
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
      assert_eq!(*view.read(cx).composer.read(cx).editor(), composer);
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
      assert_eq!(*view.read(cx).composer.read(cx).editor(), composer);
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
      assert_eq!(*view.read(cx).composer.read(cx).editor(), composer);
      assert_eq!(composer.read(cx).selected_range(), selection);
      assert_eq!(composer.read(cx).value().as_ref(), "# PlaN\nsecond line\n");
      assert!(composer.focus_handle(cx).is_focused(window));
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
      let viewport = window.find(format!("transcript-viewport-{id}")).bounds();
      assert_eq!(viewport.left(), content.left() + rem * 2.5);
      assert_eq!(viewport.right(), content.right());
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
      let viewport = window.find(format!("transcript-viewport-{id}")).bounds();
      let rail = window.find("conversation-timeline").bounds();
      assert!(rail.right() <= viewport.left());
      assert_eq!(viewport.right(), content.right());
      let first_id = super::transcript::message_id(model.read(cx).conversation().live(session.live).unwrap(), 0);
      let lane = window.find(format!("{first_id}-reading-lane")).bounds();
      assert!(lane.left() >= viewport.left());
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
      let theme = gpui_kit::component::ActiveTheme::theme(cx);
      let sample = window.text_system().shape_line(
        "0".repeat(120).into(),
        theme.mono_font_size,
        &[gpui_kit::TextRun {
          len: 120,
          font: gpui_kit::font(theme.mono_font_family.clone()),
          color: theme.foreground,
          background_color: None,
          underline: None,
          strikethrough: None,
        }],
        None,
      );
      assert!((normal.size.width - sample.width).abs() < px(1.));
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
      let viewport = window.find(format!("transcript-viewport-{id}")).bounds();
      assert!(row.left() >= viewport.left());
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
    directory.close().unwrap();
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
        assert!(editor.bottom() <= model.top(), "editor {editor:?}, model {model:?}, send {send:?}");
        assert!(editor.bottom() <= send.top(), "editor {editor:?}, model {model:?}, send {send:?}");
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
