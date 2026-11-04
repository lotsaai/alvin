//! User-message navigation over the transcript's cached presentation rows.
//!
//! The session header owns the narrow query menu; only the wide rail overlays the transcript.

use std::rc::Rc;

use alvin_conversation::LiveView;
use alvin_protocol::{AgentMessage, LiveSessionId, MessageContent, UserContent, UserMessage};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  AnyElement, App, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, Role,
  ScrollStrategy, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, UniformListScrollHandle, Window, actions,
  canvas, div, rems, uniform_list,
};

use super::transcript::Row;
use crate::model::Model;

const KEY_CONTEXT: &str = "ConversationTimeline";
/// ZCode's 864px conversation breakpoint, expressed in the application's zoom scale.
const RAIL_MIN_WIDTH_REM: f32 = 54.;
const PREVIEW_CHARS: usize = 320;

actions!(alvin_timeline, [PreviousQuery, NextQuery, FirstQuery, LastQuery, JumpToQuery]);

/// Registers keyboard commands once, during conversation UI initialization.
pub(crate) fn init(cx: &mut App) {
  cx.bind_keys([
    KeyBinding::new("up", PreviousQuery, Some(KEY_CONTEXT)),
    KeyBinding::new("down", NextQuery, Some(KEY_CONTEXT)),
    KeyBinding::new("home", FirstQuery, Some(KEY_CONTEXT)),
    KeyBinding::new("end", LastQuery, Some(KEY_CONTEXT)),
    KeyBinding::new("enter", JumpToQuery, Some(KEY_CONTEXT)),
    KeyBinding::new("space", JumpToQuery, Some(KEY_CONTEXT)),
  ]);
}

/// A source position is not a durable protocol message ID. History replacement invalidates its projection.
#[derive(Clone, Debug, PartialEq)]
struct TimelineAnchor {
  source_ix: usize,
  row_ix: usize,
  id: SharedString,
  preview: SharedString,
  user: UserMessage,
}

impl TimelineAnchor {
  fn user_rows<'a>(view: &'a LiveView, rows: &'a [Row]) -> impl Iterator<Item = (usize, usize, &'a UserMessage)> {
    rows.iter().enumerate().filter_map(|(row_ix, row)| {
      let Row::Message(source_ix) = row else { return None };
      let AgentMessage::User(user) = view.transcript.get(*source_ix)? else {
        return None;
      };
      Some((*source_ix, row_ix, user))
    })
  }

  fn project(live: LiveSessionId, view: &LiveView, rows: &[Row], previous: &Rc<Vec<Rc<Self>>>) -> Rc<Vec<Rc<Self>>> {
    let mut existing = previous.iter();
    let unchanged = Self::user_rows(view, rows).all(|(source_ix, row_ix, user)| {
      existing
        .next()
        .is_some_and(|anchor| anchor.source_ix == source_ix && anchor.row_ix == row_ix && &anchor.user == user)
    }) && existing.next().is_none();
    if unchanged {
      return previous.clone();
    }
    let mut existing = previous.iter().peekable();
    Rc::new(
      Self::user_rows(view, rows)
        .map(|(source_ix, row_ix, user)| {
          while existing.peek().is_some_and(|anchor| anchor.source_ix < source_ix) {
            existing.next();
          }
          if let Some(anchor) = existing.peek()
            && anchor.source_ix == source_ix
            && anchor.row_ix == row_ix
            && &anchor.user == user
          {
            return Rc::clone(anchor);
          }
          Rc::new(Self {
            source_ix,
            row_ix,
            id: format!("timeline-message-{}-{source_ix}", live.as_uuid()).into(),
            preview: Self::preview(user),
            user: user.clone(),
          })
        })
        .collect(),
    )
  }

  fn preview(user: &UserMessage) -> SharedString {
    let text = match &user.content {
      Some(MessageContent::String(text)) => text.chars().take(PREVIEW_CHARS).collect::<String>(),
      Some(MessageContent::UserContentList(content)) => content
        .iter()
        .filter_map(|content| match content {
          UserContent::Text(text) => text.text.as_deref(),
          UserContent::Image(_) => None,
        })
        .flat_map(|text| text.chars().chain(std::iter::once(' ')))
        .take(PREVIEW_CHARS)
        .collect(),
      None => String::new(),
    };
    if text.trim().is_empty() {
      "User message without text".into()
    } else {
      text.trim().to_owned().into()
    }
  }

  fn active(anchors: &[Rc<Self>], top_row_ix: usize) -> Option<usize> {
    anchors.partition_point(|anchor| anchor.row_ix <= top_row_ix).checked_sub(1)
  }
}

/// Retains only navigation, focus, and the immutable user-boundary projection.
pub(super) struct ConversationTimeline {
  scroller: Entity<MessageScrollerState>,
  anchors: Rc<Vec<Rc<TimelineAnchor>>>,
  rail: UniformListScrollHandle,
  focus: FocusHandle,
  cursor: Option<usize>,
  hovered: Option<usize>,
  last_active: Option<usize>,
  wide: bool,
  _subscriptions: Vec<Subscription>,
}

impl ConversationTimeline {
  pub(super) fn new(scroller: Entity<MessageScrollerState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let focus = cx.focus_handle().tab_stop(true);
    let subscriptions = vec![
      cx.observe(&scroller, |this, _, cx| {
        let active = this.active(cx);
        if this.last_active != active {
          this.last_active = active;
          if let Some(ix) = active {
            this.rail.scroll_to_item(ix, ScrollStrategy::Nearest);
          }
        }
        cx.notify();
      }),
      cx.on_focus(&focus, window, |this, _, cx| {
        this.cursor = this.active(cx).or_else(|| (!this.anchors.is_empty()).then_some(0));
        cx.notify();
      }),
      cx.on_blur(&focus, window, |_, _, cx| cx.notify()),
    ];
    Self {
      scroller,
      anchors: Rc::new(Vec::new()),
      rail: UniformListScrollHandle::new(),
      focus,
      cursor: None,
      hovered: None,
      last_active: None,
      wide: false,
      _subscriptions: subscriptions,
    }
  }

  pub(super) fn sync(&mut self, live: LiveSessionId, model: &Entity<Model>, rows: &Rc<Vec<Row>>, cx: &mut Context<Self>) {
    let anchors = match model.read(cx).conversation().live(live) {
      Some(view) => TimelineAnchor::project(live, view, rows, &self.anchors),
      None => Rc::new(Vec::new()),
    };
    if !Rc::ptr_eq(&anchors, &self.anchors) {
      self.anchors = anchors;
      self.cursor = None;
      self.hovered = None;
      self.last_active = self.active(cx);
      if let Some(ix) = self.last_active {
        self.rail.scroll_to_item(ix, ScrollStrategy::Nearest);
      }
      cx.notify();
    }
  }

  fn active(&self, cx: &App) -> Option<usize> {
    TimelineAnchor::active(&self.anchors, self.scroller.read(cx).logical_scroll_top().item_ix)
  }

  fn jump(&mut self, snapshot: &Rc<Vec<Rc<TimelineAnchor>>>, ix: usize, cx: &mut Context<Self>) -> bool {
    // Append preserves anchors; replaced history invalidates its old menu and virtual-row closures.
    let Some(requested) = snapshot.get(ix) else { return false };
    let Some(anchor) = self.anchors.iter().find(|anchor| Rc::ptr_eq(anchor, requested)) else {
      return false;
    };
    self.scroller.update(cx, |state, cx| state.scroll_to_item(anchor.row_ix, cx))
  }

  fn move_cursor(&mut self, direction: i8, cx: &mut Context<Self>) {
    let Some(last) = self.anchors.len().checked_sub(1) else { return };
    let current = self.cursor.or_else(|| self.active(cx)).unwrap_or(0);
    self.cursor = Some(if direction < 0 {
      current.saturating_sub(1)
    } else {
      current.saturating_add(1).min(last)
    });
    if let Some(ix) = self.cursor {
      self.rail.scroll_to_item(ix, ScrollStrategy::Nearest);
    }
    cx.notify();
  }

  fn marker(&self, ix: usize, cx: &Context<Self>) -> Option<Button> {
    let anchor = self.anchors.get(ix)?;
    let snapshot = self.anchors.clone();
    let active = self.active(cx) == Some(ix);
    let color = if active {
      cx.theme().foreground
    } else {
      cx.theme().muted_foreground.opacity(0.45)
    };
    Some(
      Button::new(anchor.id.clone())
        .ghost()
        .xsmall()
        .compact()
        .w_full()
        .h_3()
        .tab_stop(false)
        .accessibility_label(SharedString::from(format!("Jump to query: {}", anchor.preview)))
        .selected(active)
        .child(
          div()
            .h_0p5()
            .bg(color)
            .when(active, |tick| tick.w_4())
            .when(!active, |tick| tick.w_3()),
        )
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
          this.hovered = hovered.then_some(ix);
          cx.notify();
        }))
        .on_click(cx.listener(move |this, _, _, cx| {
          this.jump(&snapshot, ix, cx);
        })),
    )
  }

  /// The header's menu, using this same retained projection and actual transcript-width policy.
  pub(super) fn compact_menu(&self, owner: &Entity<Self>) -> Option<AnyElement> {
    if self.wide || self.anchors.is_empty() {
      return None;
    }
    let snapshot = self.anchors.clone();
    let owner = owner.clone();
    Some(
      Button::new("conversation-timeline-menu")
        .ghost()
        .xsmall()
        .label("Queries")
        .accessibility_label("Conversation timeline")
        .tooltip("Jump to a user query")
        .dropdown_menu(move |mut menu, _, _| {
          for (ix, anchor) in snapshot.iter().enumerate() {
            let owner = owner.clone();
            let snapshot = snapshot.clone();
            menu = menu.item(PopupMenuItem::new(anchor.preview.clone()).on_click(move |_, _, cx| {
              owner.update(cx, |this, cx| {
                this.jump(&snapshot, ix, cx);
              });
            }));
          }
          menu.scrollable(true)
        })
        .into_any_element(),
    )
  }
}

impl Render for ConversationTimeline {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let owner = cx.entity();
    let wide = self.wide;
    let measure = canvas(
      move |bounds, window, _| {
        let next = bounds.size.width >= window.rem_size() * RAIL_MIN_WIDTH_REM;
        if next != wide {
          window.on_next_frame(move |_, cx| {
            owner.update(cx, |this, cx| {
              if this.wide != next {
                this.wide = next;
                cx.notify();
              }
            })
          });
        }
      },
      |_, (), _, _| {},
    )
    .absolute()
    .size_full();
    let preview_ix = self
      .hovered
      .or_else(|| self.focus.is_focused(window).then_some(self.cursor).flatten());
    let preview = preview_ix.and_then(|ix| self.anchors.get(ix)).map(|anchor| anchor.preview.clone());
    let owner = cx.entity();
    let count = self.anchors.len();
    let visible_rows = f32::from(u8::try_from(count.min(32)).unwrap_or(32));
    div()
      .absolute()
      .inset_0()
      .child(measure)
      .when(count > 0 && wide, |root| {
        root.child(
          div()
            .id("conversation-timeline")
            .test_support()
            .role(Role::Group)
            .aria_label("Conversation timeline. Up and Down choose a query. Enter jumps to it.")
            .absolute()
            .left_1()
            .top_8()
            .bottom_8()
            .w_8()
            .flex()
            .flex_col()
            .justify_center()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .focus_visible(|style| style.bg(cx.theme().muted).rounded(cx.theme().radius))
            .on_action(cx.listener(|this, _: &PreviousQuery, _, cx| this.move_cursor(-1, cx)))
            .on_action(cx.listener(|this, _: &NextQuery, _, cx| this.move_cursor(1, cx)))
            .on_action(cx.listener(|this, _: &FirstQuery, _, cx| {
              this.cursor = Some(0);
              this.rail.scroll_to_item(0, ScrollStrategy::Nearest);
              cx.notify();
            }))
            .on_action(cx.listener(|this, _: &LastQuery, _, cx| {
              this.cursor = this.anchors.len().checked_sub(1);
              if let Some(ix) = this.cursor {
                this.rail.scroll_to_item(ix, ScrollStrategy::Nearest);
              }
              cx.notify();
            }))
            .on_action(cx.listener(|this, _: &JumpToQuery, _, cx| {
              if let Some(ix) = this.cursor {
                this.jump(&this.anchors.clone(), ix, cx);
              }
            }))
            .child(
              uniform_list("conversation-timeline-markers", count, move |range, _, cx| {
                owner.update(cx, |timeline, cx| {
                  range.filter_map(|ix| timeline.marker(ix, cx)).collect::<Vec<_>>()
                })
              })
              .track_scroll(&self.rail)
              .w_full()
              .h(rems(visible_rows * 0.75))
              .max_h_full(),
            ),
        )
      })
      .when(wide, |root| {
        root.when_some(preview, |root, preview| {
          root.child(
            div()
              .absolute()
              .left_10()
              .top_8()
              .bottom_8()
              .w_72()
              .flex()
              .flex_col()
              .justify_center()
              .child(
                div()
                  .p_3()
                  .max_h_full()
                  .overflow_hidden()
                  .rounded(cx.theme().radius)
                  .bg(cx.theme().popover)
                  .text_color(cx.theme().popover_foreground)
                  .text_sm()
                  .shadow_md()
                  .child(preview),
              ),
          )
        })
      })
  }
}

#[cfg(test)]
mod tests {
  use std::rc::Rc;

  use alvin_conversation::{Conversation, LiveView, Streaming};
  use alvin_protocol::{AgentMessage, LiveSessionId};
  use gpui_kit::component::message_scroller::MessageScrollerState;
  use gpui_kit::{AppContext as _, TestAppContext};

  use super::{ConversationTimeline, TimelineAnchor};
  use crate::session::transcript::{Row, rows};
  use crate::test_support::live_session;

  fn conversation() -> (LiveSessionId, LiveView) {
    let session = live_session("/work/timeline");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role": "user", "content": "Inspect the parser"},
      {"role": "assistant", "content": [{"type": "thinking", "thinking": "Inspecting"}]},
      {"role": "toolResult", "toolCallId": "read-parser", "toolName": "read", "content": [{"type": "text", "text": "parser"}]},
      {"role": "assistant", "content": [{"type": "text", "text": "Found the parser"}]},
      {"role": "user", "content": "Fix the parser"},
      {"role": "assistant", "content": [{"type": "text", "text": "Fixed"}]}
    ]))
    .unwrap();
    (session.live, view)
  }

  #[test]
  fn real_user_anchors_keep_source_positions_separate_from_grouped_rows() {
    let (live, view) = conversation();
    let presentation = rows(&view);
    assert_eq!(
      presentation,
      vec![
        Row::Message(0),
        Row::Response { start: 1, end: 4 },
        Row::Message(4),
        Row::Response { start: 5, end: 6 },
      ]
    );
    let anchors = TimelineAnchor::project(live, &view, &presentation, &Rc::new(Vec::new()));
    assert_eq!(
      anchors
        .iter()
        .map(|anchor| (anchor.source_ix, anchor.row_ix, anchor.preview.as_ref()))
        .collect::<Vec<_>>(),
      vec![(0, 0, "Inspect the parser"), (4, 2, "Fix the parser"),]
    );
    assert_eq!(TimelineAnchor::active(&anchors, 0), Some(0));
    assert_eq!(TimelineAnchor::active(&anchors, 1), Some(0));
    assert_eq!(TimelineAnchor::active(&anchors, 2), Some(1));
    assert_eq!(TimelineAnchor::active(&anchors, 3), Some(1));
    assert_eq!(TimelineAnchor::active(&anchors, usize::MAX), Some(1));
  }

  #[test]
  fn projection_reuses_user_boundaries_during_streaming_and_append() {
    let (live, mut view) = conversation();
    let original = TimelineAnchor::project(live, &view, &rows(&view), &Rc::new(Vec::new()));
    view.streaming = Some(Streaming {
      message_id: Some("actual-stream-message".into()),
      message: serde_json::from_value(serde_json::json!({
        "role": "assistant", "content": [{"type": "text", "text": "Streaming more"}]
      }))
      .unwrap(),
    });
    let streaming = TimelineAnchor::project(live, &view, &rows(&view), &original);
    assert!(Rc::ptr_eq(&original, &streaming));
    view
      .transcript
      .push(serde_json::from_value(serde_json::json!({"role":"user","content":"Check it"})).unwrap());
    let appended = TimelineAnchor::project(live, &view, &rows(&view), &streaming);
    assert_eq!(appended.len(), 3);
    assert!(Rc::ptr_eq(&original[0], &appended[0]));
    assert!(Rc::ptr_eq(&original[1], &appended[1]));
    assert_eq!(appended[2].source_ix, 6);
    assert_eq!(appended[2].row_ix, 4);
  }

  #[test]
  fn partial_history_selects_nothing_before_its_first_loaded_user() {
    let (live, mut view) = conversation();
    view.transcript.remove(0);
    let anchors = TimelineAnchor::project(live, &view, &rows(&view), &Rc::new(Vec::new()));
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].source_ix, 3);
    assert_eq!(anchors[0].row_ix, 3);
    assert_eq!(TimelineAnchor::active(&anchors, 2), None);
    assert_eq!(TimelineAnchor::active(&anchors, 3), Some(0));
    view.transcript.truncate(3);
    let empty = TimelineAnchor::project(live, &view, &rows(&view), &anchors);
    assert!(empty.is_empty());
    assert_eq!(TimelineAnchor::active(&empty, 0), None);
  }

  #[gpui_kit::test]
  fn jumps_and_manual_scroll_use_real_scroller_position_and_reject_replaced_queries(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (live, mut view) = conversation();
    let scroller = cx.new(|cx| MessageScrollerState::new(rows(&view).len(), cx));
    let (timeline, cx) = cx.add_window_view({
      let scroller = scroller.clone();
      move |window, cx| ConversationTimeline::new(scroller, window, cx)
    });
    cx.update(|_, cx| {
      timeline.update(cx, |timeline, cx| {
        timeline.anchors = TimelineAnchor::project(live, &view, &rows(&view), &timeline.anchors);
        let snapshot = timeline.anchors.clone();
        assert!(timeline.jump(&snapshot, 1, cx));
        assert_eq!(scroller.read(cx).logical_scroll_top().item_ix, 2);
        assert_eq!(timeline.active(cx), Some(1));
        assert!(!scroller.read(cx).is_following_tail());
        assert!(timeline.jump(&snapshot, 0, cx));
        assert_eq!(timeline.active(cx), Some(0));
        scroller.update(cx, |state, cx| assert!(state.scroll_to_item(3, cx)));
        assert_eq!(timeline.active(cx), Some(1));

        view.transcript.push(AgentMessage::User(
          serde_json::from_value(serde_json::json!({"content":"Review the result"})).unwrap(),
        ));
        timeline.anchors = TimelineAnchor::project(live, &view, &rows(&view), &timeline.anchors);
        scroller.update(cx, |state, cx| assert!(state.append(1, cx)));
        assert!(!scroller.read(cx).is_following_tail());
        assert!(timeline.jump(&snapshot, 0, cx));
        view.transcript[0] = serde_json::from_value(serde_json::json!({"role":"user","content":"Different history"})).unwrap();
        timeline.anchors = TimelineAnchor::project(live, &view, &rows(&view), &timeline.anchors);
        assert!(!timeline.jump(&snapshot, 0, cx));
        assert_eq!(scroller.read(cx).logical_scroll_top().item_ix, 0);
        let current = timeline.anchors.clone();
        assert!(timeline.jump(&current, 2, cx));
        assert_eq!(scroller.read(cx).logical_scroll_top().item_ix, 4);
        assert_eq!(timeline.active(cx), Some(2));
        scroller.update(cx, |state, cx| state.scroll_to_end(cx));
        assert!(scroller.read(cx).is_following_tail());
        assert_eq!(timeline.active(cx), Some(2));
      })
    });
  }
}
