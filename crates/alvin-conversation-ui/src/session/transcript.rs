//! The transcript: committed messages plus the one streaming, in a `MessageScroller`.
mod content;
mod events;
mod lane;
mod owner;
mod tool;

pub(super) use events::{SystemActivity, SystemActivityData};
use gpui_kit::StatefulInteractiveElement as _;
pub(super) use owner::Transcript;

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use alvin_conversation::LiveView;
use alvin_protocol::{AgentMessage, AssistantContent, LiveSessionId, MessageContent, UserContent};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerLoadingStyle};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, Div, ElementId, Entity, Global, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels,
  SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};

pub(super) fn fenced(text: &str) -> String {
  let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or_default();
  let fence = "`".repeat(longest.max(2).saturating_add(1));
  format!("{fence}\n{text}\n{fence}")
}

/// Application-provided colors for user message bubbles.
///
/// Install this global before refreshing windows after a theme change. If absent,
/// user messages use the active theme's primary colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UserMessageColors {
  background: Hsla,
  foreground: Hsla,
}

impl UserMessageColors {
  /// Creates user message colors from a background and its contrasting foreground.
  pub fn new(background: Hsla, foreground: Hsla) -> Self {
    Self { background, foreground }
  }

  /// Returns the user message background.
  pub fn background(&self) -> Hsla {
    self.background
  }

  /// Returns the user message foreground.
  pub fn foreground(&self) -> Hsla {
    self.foreground
  }

  fn current(cx: &App) -> Self {
    cx.try_global::<Self>()
      .copied()
      .unwrap_or_else(|| Self::new(cx.theme().primary, cx.theme().primary_foreground))
  }
}

impl Global for UserMessageColors {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Row {
  Message(usize),
  Response { start: usize, end: usize },
}

fn message_at(view: &LiveView, index: usize) -> Option<(&AgentMessage, bool)> {
  view.transcript.get(index).map(|message| (message, false)).or_else(|| {
    (index == view.transcript.len())
      .then(|| view.streaming.as_ref().map(|streaming| (&streaming.message, true)))
      .flatten()
  })
}

pub(super) fn message_id(view: &LiveView, index: usize) -> SharedString {
  SharedString::from(format!(
    "message-{}-snapshot-{}-{index}",
    view.live.as_uuid(),
    view.snapshot_generation
  ))
}

pub(super) fn synthetic_directive(message: &AgentMessage) -> bool {
  let (synthetic, attribution, user_initiated) = match message {
    AgentMessage::User(message) => (
      message.synthetic,
      message.attribution,
      message.extra.get("userInitiated").and_then(serde_json::Value::as_bool),
    ),
    AgentMessage::Developer(message) => (message.synthetic, message.attribution, message.user_initiated),
    _ => return false,
  };
  synthetic == Some(true) && attribution == Some(alvin_protocol::Attribution::Agent) && user_initiated != Some(true)
}

fn ancillary(message: &AgentMessage) -> bool {
  matches!(
    message,
    AgentMessage::Custom(_) | AgentMessage::Developer(_) | AgentMessage::HookMessage(_) | AgentMessage::FileMention(_)
  )
}

fn response_message(message: &AgentMessage) -> bool {
  matches!(message, AgentMessage::Assistant(_) | AgentMessage::ToolResult(_)) || ancillary(message) || synthetic_directive(message)
}
fn hidden(message: &AgentMessage) -> bool {
  match message {
    AgentMessage::Custom(message) => message.display == Some(false),
    AgentMessage::HookMessage(message) => message.display == Some(false),
    _ => false,
  }
}

fn append_read_group(body: Div, reads: &mut Vec<(SharedString, AnyElement)>) -> Div {
  if reads.is_empty() {
    return body;
  }
  let multiple = reads.len() > 1;
  let title = reads.first().map(|(id, _)| id.clone());
  body.child(
    v_flex()
      .w_full()
      .min_w_0()
      .gap_1()
      .when(multiple, |group| {
        group.when_some(title, |group, id| {
          group.child(
            div()
              .id((ElementId::from(id), "read-group-title"))
              .test_support()
              .aria_label("Reads")
              .text_sm()
              .child("Reads"),
          )
        })
      })
      .children(std::mem::take(reads).into_iter().map(|(_, view)| view)),
  )
}

/// Group response messages and injected context after a visible user message.
pub(crate) fn rows(view: &LiveView) -> Vec<Row> {
  let count = view.transcript.len() + usize::from(view.streaming.is_some());
  let mut rows = Vec::new();
  let mut index = 0;
  let mut after_user = false;
  while index < count {
    if message_at(view, index).is_some_and(|(message, _)| hidden(message)) {
      index += 1;
      continue;
    }
    match message_at(view, index).map(|(message, _)| message) {
      Some(message @ AgentMessage::User(_)) if !synthetic_directive(message) => {
        rows.push(Row::Message(index));
        after_user = true;
        index += 1;
      }
      Some(message) if after_user && response_message(message) => {
        let start = index;
        let mut has_assistant = false;
        while let Some((message, _)) = message_at(view, index) {
          if hidden(message) {
            index += 1;
            continue;
          }
          if !response_message(message) {
            break;
          }
          has_assistant |= matches!(message, AgentMessage::Assistant(_));
          index += 1;
        }
        if has_assistant {
          rows.push(Row::Response { start, end: index });
        } else {
          rows.extend(
            (start..index)
              .filter(|index| message_at(view, *index).is_some_and(|(message, _)| !hidden(message)))
              .map(Row::Message),
          );
        }
        after_user = false;
      }
      Some(_) => {
        rows.push(Row::Message(index));
        after_user = false;
        index += 1;
      }
      None => break,
    }
  }
  rows
}

#[derive(Default)]
struct ToolResults {
  calls: HashMap<(usize, usize), usize>,
  results: HashMap<usize, (usize, usize)>,
}

impl ToolResults {
  fn new(view: &LiveView) -> Self {
    let mut pending: HashMap<&str, VecDeque<(usize, usize)>> = HashMap::new();
    let mut paired = Self::default();
    for index in 0..view.transcript.len() + usize::from(view.streaming.is_some()) {
      match message_at(view, index).map(|(message, _)| message) {
        Some(AgentMessage::User(_)) => pending.clear(),
        Some(AgentMessage::Assistant(message)) => {
          for (position, block) in message.content.iter().flatten().enumerate() {
            if let AssistantContent::ToolCall(call) = block
              && let Some(id) = call.id.as_deref().filter(|id| !id.is_empty())
            {
              pending.entry(id).or_default().push_back((index, position));
            }
          }
        }
        Some(AgentMessage::ToolResult(result)) => {
          if let Some(call) = result
            .tool_call_id
            .as_deref()
            .and_then(|id| pending.get_mut(id))
            .and_then(VecDeque::pop_front)
          {
            paired.calls.insert(call, index);
            paired.results.insert(index, call);
          }
        }
        _ => {}
      }
    }
    paired
  }
}

pub(crate) fn transcript(
  live: LiveSessionId,
  scroller: &Entity<MessageScrollerState>,
  rows: Rc<Vec<Row>>,
  transcript: &owner::Transcript,
  max_width: Option<Pixels>,
) -> MessageScroller {
  let blocks = transcript.blocks();
  let transcript_generation = transcript.generation();
  let process_open = transcript.process_open();
  let scroller = scroller.clone();
  MessageScroller::new(
    ElementId::from(SharedString::from(format!("transcript-{}", live.as_uuid()))),
    scroller.clone(),
    move |index, _: &mut Window, cx: &mut App| {
      let Some(row) = rows.get(index) else {
        return div().into_any_element();
      };
      let range = match row {
        Row::Message(index) => *index..index.saturating_add(1),
        Row::Response { start, end } => *start..*end,
      };
      let timeline = range
        .clone()
        .filter_map(|index| blocks.get(index).and_then(|(block, _)| block.as_ref()))
        .any(|block| block.is_timeline());
      let continues = rows.get(index.saturating_add(1)).is_some_and(|next| {
        let start = match next {
          Row::Message(index) => *index,
          Row::Response { start, .. } => *start,
        };
        blocks
          .get(start)
          .and_then(|(block, _)| block.as_ref())
          .is_some_and(|block| block.is_timeline())
      });
      let mut body = v_flex().w_full().gap_2();
      if matches!(row, Row::Response { .. })
        || matches!(blocks.get(range.start), Some((Some(block), _)) if matches!(block.as_ref(), events::Block::Assistant(_)))
      {
        let last_assistant = range
          .clone()
          .rev()
          .find(|index| matches!(blocks.get(*index), Some((Some(block), _)) if matches!(block.as_ref(), events::Block::Assistant(_))));
        let open = process_open.borrow().get(&range.start).copied().unwrap_or(true);
        let mut process = open.then(|| v_flex().w_full().gap_2());
        let mut final_content = v_flex().w_full().gap_2();
        let mut reads = Vec::new();
        let mut footers = Vec::new();
        let mut has_process = false;
        for index in range.clone() {
          let Some((block, streaming)) = blocks.get(index) else { continue };
          let Some(block) = block else { continue };
          if let events::Block::Assistant(entity) = block.as_ref() {
            let assistant = entity.read(cx);
            let parts = assistant.parts(cx);
            let empty = parts.is_some_and(|parts| parts.is_empty());
            let is_last = last_assistant == Some(index);
            let eligible = is_last && assistant.may_have_answer();
            let collapsed_answer = eligible && assistant.collapsed_answer(cx);
            let mut has_answer = collapsed_answer;
            if let Some(parts) = parts {
              let suffix = if eligible {
                parts.iter().rposition(|part| !part.is_text()).map_or(0, |position| position + 1)
              } else {
                parts.len()
              };
              for (position, part) in parts.iter().enumerate() {
                if eligible && position >= suffix && part.is_text() {
                  if open {
                    process = process.map(|process| append_read_group(process, &mut reads).children(std::mem::take(&mut footers)));
                  }
                  final_content = final_content.child(part.projected_view(cx));
                  has_answer = true;
                } else {
                  has_process = true;
                  if open {
                    if part.is_read() {
                      reads.push((part.id().clone(), part.projected_view(cx)));
                    } else {
                      process = process.map(|process| {
                        append_read_group(process, &mut reads)
                          .children(std::mem::take(&mut footers))
                          .child(part.projected_view(cx))
                      });
                    }
                  }
                }
              }
            } else if collapsed_answer {
              if open {
                process = process.map(|process| append_read_group(process, &mut reads).children(std::mem::take(&mut footers)));
              }
              final_content = final_content.child(div().child("Message collapsed"));
            } else {
              has_process = true;
              if open {
                process = process.map(|process| {
                  append_read_group(process, &mut reads)
                    .children(std::mem::take(&mut footers))
                    .child(div().child("Message collapsed"))
                });
              }
            }
            if let Some(diagnostic) = assistant.diagnostic() {
              if open {
                process = process.map(|process| append_read_group(process, &mut reads).children(std::mem::take(&mut footers)));
              }
              if !range
                .clone()
                .skip(index.saturating_add(1).saturating_sub(range.start))
                .any(|index| blocks.get(index).is_some_and(|(block, _)| block.is_some()))
              {
                final_content = final_content.child(diagnostic.clone());
                has_answer = true;
              } else {
                has_process = true;
                if open {
                  process = process.map(|process| process.child(diagnostic.clone()));
                }
              }
            }
            if has_answer || (is_last && eligible && empty) {
              final_content = final_content.child(assistant.footer(cx));
            } else {
              has_process = true;
              if open {
                let footer = assistant.footer(cx);
                if reads.is_empty() {
                  process = process.map(|process| process.child(footer));
                } else {
                  footers.push(footer);
                }
              }
            }
          } else {
            has_process = true;
            if open {
              process = process.map(|process| {
                let process = append_read_group(process, &mut reads).children(std::mem::take(&mut footers));
                if let events::Block::Custom { id, .. } = block.as_ref() {
                  process.child(lane::item(id.clone(), gpui_kit::component::IconName::Info, block.view(), cx))
                } else {
                  process.child(block.view())
                }
              });
            }
          }
          if *streaming {
            has_process = true;
            if open {
              process = process.map(|process| {
                append_read_group(process, &mut reads).children(std::mem::take(&mut footers)).child(
                  Marker::new()
                    .loading(true)
                    .with_loading_style(MarkerLoadingStyle::Shimmer)
                    .content(MarkerContent::new().text("Working")),
                )
              });
            }
          }
        }
        process = process.map(|process| append_read_group(process, &mut reads).children(footers));
        if has_process {
          let state = process_open.clone();
          let scroller = scroller.clone();
          let start = range.start;
          body = body.child(
            Button::new(SharedString::from(format!(
              "process-{}-snapshot-{}-{start}",
              live.as_uuid(),
              transcript_generation
            )))
            .ghost()
            .small()
            .label(if open { "Hide process" } else { "Show process" })
            .on_click(move |_, window, cx| {
              let mut state = state.borrow_mut();
              let value = state.entry(start).or_insert(true);
              *value = !*value;
              drop(state);
              scroller.update(cx, |scroller, cx| scroller.remeasure(cx));
              window.refresh();
            }),
          );
          if let Some(process) = process {
            body = body.child(process);
          }
        }
        body = body.child(final_content);
      } else {
        for index in range.clone() {
          if let Some((block, streaming)) = blocks.get(index) {
            if let Some(block) = block {
              body = body.child(block.view());
            }
            if *streaming {
              body = body.child(
                Marker::new()
                  .loading(true)
                  .with_loading_style(MarkerLoadingStyle::Shimmer)
                  .content(MarkerContent::new().text("Working")),
              );
            }
          }
        }
      }
      let id = format!("message-{}-snapshot-{}-{}", live.as_uuid(), transcript_generation, range.start);
      h_flex()
        .group("message-actions")
        .w_full()
        .justify_center()
        .px_10()
        .child(
          super::reading_lane(max_width)
            .id(SharedString::from(format!("{id}-reading-lane")))
            .test_support()
            .child(
              div()
                .relative()
                .w_full()
                .when(timeline, |this| {
                  this.child(
                    div()
                      .absolute()
                      .left(gpui_kit::rems(-2.))
                      .top_0()
                      .bottom(if continues { gpui_kit::rems(-2.) } else { gpui_kit::rems(0.) })
                      .w_6()
                      .flex()
                      .justify_center()
                      .child(
                        div()
                          .id(SharedString::from(format!("{id}-lane-connector")))
                          .test_support()
                          .h_full()
                          .w(gpui_kit::px(1.))
                          .bg(cx.theme().border),
                      ),
                  )
                })
                .child(body),
            ),
        )
        .into_any_element()
    },
  )
  .with_row_style(gpui_kit::StyleRefinement::default().px_0())
  .w_full()
  .flex_1()
}

fn content_text(content: &MessageContent) -> String {
  match content {
    MessageContent::String(text) => text.clone(),
    MessageContent::UserContentList(parts) => parts.iter().map(user_content_text).collect::<Vec<_>>().join("\n"),
  }
}

fn user_content_text(content: &UserContent) -> String {
  match content {
    UserContent::Text(text) => text.text.clone().unwrap_or_default(),
    UserContent::Image(_) => "[image]".to_owned(),
  }
}

#[cfg(test)]
mod tests {

  use alvin_conversation::{Conversation, Streaming};
  use alvin_protocol::{AgentMessage, MessagesPage};
  use gpui_kit::component::ActiveTheme as _;
  use gpui_kit::{AppContext as _, TestAppContext};

  use super::{Row, UserMessageColors, rows};
  use crate::test_support::live_session;

  /// An omp turn that called every built-in tool once, as omp recorded it.
  const TOOLS_TURN: &str = include_str!("../../fixtures/tools_turn.messages.json");

  #[gpui_kit::test]
  fn live_message_identity_survives_commit_and_snapshot_replaces_it(_cx: &mut TestAppContext) {
    let events: Vec<alvin_protocol::Event> =
      serde_json::from_str(include_str!("../../../alvin-conversation/fixtures/plain_turn.events.json")).unwrap();
    let mut session = live_session("/work/api");
    session.live = events[0].live;
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut committed = 0;
    for event in &events {
      let before = conversation.live(session.live).unwrap();
      let pending = before
        .streaming
        .as_ref()
        .map(|_| (before.transcript.len(), super::message_id(before, before.transcript.len())));
      conversation.apply(event);
      let after = conversation.live(session.live).unwrap();
      if let Some((index, id)) = pending
        && after.streaming.is_none()
        && after.transcript.len() == index + 1
      {
        assert_eq!(id, super::message_id(after, index));
        committed += 1;
      }
    }
    assert!(committed > 0);
    let before = super::message_id(conversation.live(session.live).unwrap(), 0);
    let page: MessagesPage = serde_json::from_str(include_str!("../../../alvin-conversation/fixtures/plain_turn.messages.json")).unwrap();
    conversation.load_page(session.live, page);
    assert_ne!(before, super::message_id(conversation.live(session.live).unwrap(), 0));
  }
  #[gpui_kit::test]
  fn constructed_synthetic_directives_do_not_reclassify_user_text(_cx: &mut TestAppContext) {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role": "user", "content": "<system-interrupt>example</system-interrupt>"},
      {"role": "user", "content": "<system-interrupt>redirect</system-interrupt>", "synthetic": true, "attribution": "agent"},
      {"role": "developer", "content": "directive", "synthetic": true, "attribution": "agent"},
      {"role": "assistant", "content": [{"type": "text", "text": "answer"}]},
      {"role": "user", "content": "interjection", "steering": true},
      {"role": "developer", "content": "user requested", "synthetic": true, "userInitiated": true, "attribution": "agent"}
    ]))
    .unwrap();
    assert!(!super::synthetic_directive(&view.transcript[0]));
    assert!(super::synthetic_directive(&view.transcript[1]));
    assert!(super::synthetic_directive(&view.transcript[2]));
    assert!(!super::synthetic_directive(&view.transcript[4]));
    assert!(!super::synthetic_directive(&view.transcript[5]));
    assert_eq!(
      rows(&view),
      vec![
        Row::Message(0),
        Row::Response { start: 1, end: 4 },
        Row::Message(4),
        Row::Message(5),
      ]
    );
  }

  #[gpui_kit::test]
  fn injected_context_stays_with_response_and_final_answer(_cx: &mut TestAppContext) {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role": "user", "content": "first"},
      {"role": "custom", "customType": "pi-dredd-skill", "content": "skill instructions"},
      {"role": "assistant", "content": [
        {"type": "thinking", "thinking": "considering"},
        {"type": "toolCall", "id": "call", "name": "read", "arguments": {"path": "file"}}
      ]},
      {"role": "toolResult", "toolCallId": "call", "toolName": "read", "content": [{"type": "text", "text": "file contents"}]},
      {"role": "assistant", "content": [{"type": "text", "text": "final answer"}]},
      {"role": "hookMessage", "customType": "hook", "content": "hook output"},
      {"role": "user", "content": "next"},
      {"role": "developer", "content": "context"},
      {"role": "assistant", "content": [{"type": "text", "text": "next answer"}]}
    ]))
    .unwrap();
    assert_eq!(
      rows(&view),
      vec![
        Row::Message(0),
        Row::Response { start: 1, end: 6 },
        Row::Message(6),
        Row::Response { start: 7, end: 9 },
      ]
    );
    assert!(matches!(view.transcript.get(1), Some(AgentMessage::Custom(_))));
    assert!(matches!(view.transcript.get(3), Some(AgentMessage::ToolResult(_))));
    assert!(matches!(view.transcript.get(5), Some(AgentMessage::HookMessage(_))));

    view.transcript.truncate(2);
    assert_eq!(rows(&view), vec![Row::Message(0), Row::Message(1)]);
  }

  #[gpui_kit::test]
  fn response_groups_follow_user_boundaries_and_streaming(_cx: &mut TestAppContext) {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    let page: MessagesPage = serde_json::from_str(TOOLS_TURN).unwrap();
    view.transcript = page.messages;
    let end = view.transcript.len();
    assert_eq!(rows(&view), vec![Row::Message(0), Row::Response { start: 1, end }]);
    let user: AgentMessage = serde_json::from_value(serde_json::json!({"role": "user", "content": "next"})).unwrap();
    let assistant: AgentMessage = serde_json::from_value(serde_json::json!({
      "role": "assistant", "content": [{"type": "thinking", "thinking": "working"}]
    }))
    .unwrap();
    view.transcript.push(user);
    view.streaming = Some(Streaming {
      message_id: Some("stream".into()),
      message: assistant,
    });
    assert_eq!(
      rows(&view),
      vec![
        Row::Message(0),
        Row::Response { start: 1, end },
        Row::Message(end),
        Row::Response {
          start: end + 1,
          end: end + 2
        },
      ]
    );
    view.streaming.as_mut().unwrap().message = serde_json::from_value(serde_json::json!({
      "role": "assistant", "content": [
        {"type": "thinking", "thinking": "working"}, {"type": "text", "text": "answer"}
      ]
    }))
    .unwrap();
    view.transcript.remove(0);
    assert_eq!(rows(&view).first(), Some(&Row::Message(0)));
  }

  #[gpui_kit::test]
  fn repeated_tool_ids_pair_in_order_and_orphans_stay_unpaired(_cx: &mut TestAppContext) {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"toolResult","toolCallId":"same","content":[{"type":"text","text":"orphan"}]},
      {"role":"assistant","content":[
        {"type":"toolCall","id":"same","name":"read","arguments":{"path":"first"}},
        {"type":"toolCall","id":"same","name":"read","arguments":{"path":"second"}}
      ]},
      {"role":"toolResult","toolCallId":"same","content":[{"type":"text","text":"first result"}]},
      {"role":"toolResult","toolCallId":"same","isError":true,"content":[{"type":"text","text":"second result"}]}
    ]))
    .unwrap();
    let results = super::ToolResults::new(&view);
    assert_eq!(results.results.get(&0), None);
    assert_eq!(results.results.get(&2), Some(&(1, 0)));
    assert_eq!(results.results.get(&3), Some(&(1, 1)));
  }

  #[gpui_kit::test]
  fn user_message_colors_follow_global_or_primary_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      let default = UserMessageColors::current(cx);
      assert_eq!(default.background(), cx.theme().primary);
      assert_eq!(default.foreground(), cx.theme().primary_foreground);

      let chosen = UserMessageColors::new(cx.theme().foreground, cx.theme().background);
      cx.set_global(chosen);
      assert_eq!(UserMessageColors::current(cx), chosen);

      cx.set_global(default);
      assert_eq!(UserMessageColors::current(cx), default);
    });
  }

  struct CaptureTranscript {
    transcript: super::owner::Transcript,
    composer: gpui_kit::Entity<gpui_kit::component::input::EditorState>,
    scroller: gpui_kit::Entity<gpui_kit::component::message_scroller::MessageScrollerState>,
  }

  impl CaptureTranscript {
    fn new(view: alvin_conversation::LiveView, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) -> Self {
      let composer = cx.new(|cx| gpui_kit::component::input::EditorState::new(window, cx));
      let scroller = cx.new(|cx| gpui_kit::component::message_scroller::MessageScrollerState::new(0, cx));
      let mut transcript = super::owner::Transcript::default();
      let prepared = transcript.prepare(&view);
      transcript.apply(prepared, &composer, &scroller, window, cx);
      Self {
        transcript,
        composer,
        scroller,
      }
    }

    fn sync(&mut self, view: &alvin_conversation::LiveView, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) {
      let prepared = self.transcript.prepare(view);
      self.transcript.apply(prepared, &self.composer, &self.scroller, window, cx);
      cx.notify();
    }
  }

  impl gpui_kit::Render for CaptureTranscript {
    fn render(&mut self, _: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
      use gpui_kit::{ParentElement as _, Styled as _};
      gpui_kit::component::v_flex()
        .size_full()
        .bg(cx.theme().background)
        .p_4()
        .gap_2()
        .children(
          self
            .transcript
            .blocks()
            .iter()
            .filter_map(|(block, _)| block.as_ref().map(|block| block.view())),
        )
    }
  }

  struct VirtualTranscript {
    fixture: gpui_kit::Entity<CaptureTranscript>,
    live: alvin_protocol::LiveSessionId,
    rows: std::rc::Rc<Vec<super::Row>>,
  }

  impl gpui_kit::Render for VirtualTranscript {
    fn render(&mut self, _: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
      use gpui_kit::{ParentElement as _, Styled as _};
      let fixture = self.fixture.read(cx);
      gpui_kit::div().size_full().flex().flex_col().child(super::transcript(
        self.live,
        &fixture.scroller,
        self.rows.clone(),
        &fixture.transcript,
        None,
      ))
    }
  }

  #[gpui_kit::test]
  fn mixed_read_groups_and_process_folding_preserve_disclosures(cx: &mut TestAppContext) {
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"user","content":"inspect"},
      {"role":"assistant","stopReason":"toolUse","usage":{"input":12,"output":3},"content":[
        {"type":"text","text":"Checking files"},
        {"type":"toolCall","id":"a","name":"read","arguments":{"path":"a"}}
      ]},
      {"role":"assistant","stopReason":"toolUse","content":[
        {"type":"toolCall","id":"b","name":"read","arguments":{"path":"b"}},
        {"type":"thinking","thinking":"considering"},
        {"type":"toolCall","id":"c","name":"read","arguments":{"path":"c"}}
      ]},
      {"role":"assistant","stopReason":"toolUse","content":[
        {"type":"toolCall","id":"d","name":"read","arguments":{"path":"d"}},
        {"type":"thinking","narration":true,"thinking":"Next"},
        {"type":"toolCall","id":"e","name":"read","arguments":{"path":"e"}},
        {"type":"toolCall","id":"shell","name":"bash","arguments":{"command":"pwd"}},
        {"type":"toolCall","id":"f","name":"read","arguments":{"path":"f"}}
      ]},
      {"role":"custom","customType":"notice","content":"context"},
      {"role":"assistant","stopReason":"toolUse","content":[
        {"type":"toolCall","id":"g","name":"read","arguments":{"path":"g"}}
      ]},
      {"role":"assistant","stopReason":"stop","usage":{"input":9,"output":2},"content":[{"type":"text","text":"Answer"}]}
    ]))
    .unwrap();
    let first = super::message_id(&view, 1);
    let answer = super::message_id(&view, 6);
    let process = format!("process-{}-snapshot-{}-1", session.live.as_uuid(), view.snapshot_generation);
    let rows = std::rc::Rc::new(super::rows(&view));
    let (window, root) = cx.update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1000.), px(2400.))))),
          ..Default::default()
        },
        cx,
        |window, cx| {
          let fixture = cx.new(|cx| CaptureTranscript::new(view.clone(), window, cx));
          fixture
            .read(cx)
            .scroller
            .clone()
            .update(cx, |scroller, cx| scroller.reset(rows.len(), cx));
          cx.new(|_| VirtualTranscript {
            fixture,
            live: session.live,
            rows,
          })
        },
      )
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find_all_by_label("Reads").len(), 2);
      assert_eq!(window.find_all("message-metadata-trigger").len(), 2);
      window.click(format!("{first}-part-1-toggle"), cx);
      assert!(window.find(format!("{first}-part-1-details")).visible());
      window.click(process.clone(), cx);
      assert!(window.try_find(format!("{first}-part-1-toggle")).is_none());
      assert!(window.find(format!("{answer}-part-0-lane-content")).visible());
      assert_eq!(window.find_all("message-metadata-trigger").len(), 1);
      view.transcript[3] = serde_json::from_value(serde_json::json!({
        "role":"assistant","stopReason":"toolUse","content":[
          {"type":"toolCall","id":"d","name":"read","arguments":{"path":"d"}},
          {"type":"toolCall","id":"new","name":"read","arguments":{"path":"new"}},
          {"type":"thinking","narration":true,"thinking":"Next"},
          {"type":"toolCall","id":"e","name":"read","arguments":{"path":"e"}},
          {"type":"toolCall","id":"shell","name":"bash","arguments":{"command":"pwd"}},
          {"type":"toolCall","id":"f","name":"read","arguments":{"path":"f"}}
        ]
      }))
      .unwrap();
      let fixture = root.read(cx).fixture.clone();
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      window.click(process.clone(), cx);
      assert_eq!(window.find_all_by_label("Reads").len(), 2);
      assert!(window.find(format!("{first}-part-1-details")).visible());
      assert_eq!(window.find_all("message-metadata-trigger").len(), 2);
      view.streaming = Some(Streaming {
        message_id: None,
        message: view.transcript.pop().unwrap(),
      });
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      window.click(process.clone(), cx);
      assert!(window.find(format!("{answer}-part-0-lane-content")).visible());
      view.streaming.as_mut().unwrap().message = serde_json::from_value(serde_json::json!({
        "role":"assistant","stopReason":"stop","content":[
          {"type":"text","text":"Answer"},
          {"type":"toolCall","id":"later","name":"read","arguments":{"path":"later"}}
        ]
      }))
      .unwrap();
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      assert!(window.try_find(format!("{answer}-part-0-lane-content")).is_none());
      window.click(process.clone(), cx);
      assert!(window.find(format!("{answer}-part-0-lane-content")).visible());
      assert!(window.find(format!("{first}-part-1-details")).visible());
      view.streaming = None;
      view.transcript.push(
        serde_json::from_value(serde_json::json!({
          "role":"assistant","stopReason":"error","errorMessage":"Interrupted","content":[]
        }))
        .unwrap(),
      );
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      window.click(process, cx);
      assert!(window.try_find(format!("{answer}-part-0-lane-content")).is_none());
      assert!(window.find(format!("{answer}-diagnostic-group")).visible());
      window.click(format!("{answer}-diagnostic-details"), cx);
      assert!(window.find(format!("{answer}-diagnostic-body-container")).visible());
    })
    .unwrap();
  }
  #[gpui_kit::test]
  fn timeline_marker_and_content_edges_align_at_window_widths(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(gpui_kit::init);
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"user","content":"inspect"},
      {"role":"assistant","content":[
        {"type":"thinking","thinking":"considering"},
        {"type":"toolCall","id":"a","name":"read","arguments":{"path":"a"}},
        {"type":"text","text":format!("{}\n```rust\n{}\n```", "長い内容".repeat(80), "let_unicode_identifier_".repeat(80))}
      ]}
    ]))
    .unwrap();
    let id = super::message_id(&view, 1);
    let rows = std::rc::Rc::new(super::rows(&view));
    for width in [480., 1000., 1800.] {
      let view = view.clone();
      let rows = rows.clone();
      let (window, _) = cx.update(|cx| {
        gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(width), px(1400.))))),
            ..Default::default()
          },
          cx,
          |window, cx| {
            let fixture = cx.new(|cx| CaptureTranscript::new(view, window, cx));
            let scroller = fixture.read(cx).scroller.clone();
            scroller.update(cx, |scroller, cx| scroller.reset(rows.len(), cx));
            cx.new(|_| VirtualTranscript {
              fixture,
              live: session.live,
              rows,
            })
          },
        )
        .unwrap()
      });
      cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let first_marker = window.find(format!("{id}-part-0-lane-marker")).bounds();
        let first_content = window.find(format!("{id}-part-0-lane-content")).bounds();
        let reading_lane = window.find(format!("{id}-reading-lane")).bounds();
        assert_eq!(first_content.left(), reading_lane.left());
        assert_eq!(first_content.right(), reading_lane.right());
        for part in 1..3 {
          let marker = window.find(format!("{id}-part-{part}-lane-marker")).bounds();
          let content = window.find(format!("{id}-part-{part}-lane-content")).bounds();
          assert_eq!(marker.center().x, first_marker.center().x);
          assert_eq!(content.left(), first_content.left());
          assert!(marker.right() <= content.left());
          assert_eq!(content.right(), reading_lane.right());
        }
      })
      .unwrap();
    }
  }

  #[gpui_kit::test]
  fn timeline_connects_across_virtual_rows_until_user_boundary(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(gpui_kit::init);
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"assistant","content":[{"type":"text","text":"first"}]},
      {"role":"assistant","content":[{"type":"text","text":"second"}]},
      {"role":"user","content":"next"}
    ]))
    .unwrap();
    let first = super::message_id(&view, 0);
    let second = super::message_id(&view, 1);
    let user = super::message_id(&view, 2);
    let rows = std::rc::Rc::new(super::rows(&view));
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1000.), px(1400.))))),
          ..Default::default()
        },
        cx,
        |window, cx| {
          let fixture = cx.new(|cx| CaptureTranscript::new(view, window, cx));
          let scroller = fixture.read(cx).scroller.clone();
          scroller.update(cx, |scroller, cx| scroller.reset(rows.len(), cx));
          cx.new(|_| VirtualTranscript {
            fixture,
            live: session.live,
            rows,
          })
        },
      )
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let first_line = window.find(format!("{first}-lane-connector")).bounds();
      let second_line = window.find(format!("{second}-lane-connector")).bounds();
      assert_eq!(first_line.left(), second_line.left());
      assert_eq!(first_line.bottom(), second_line.top());
      assert!(window.try_find(format!("{user}-lane-connector")).is_none());
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn tool_state_survives_virtual_row_eviction(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_global(crate::ConversationPreferences::new(true, true, false));
    });
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript.push(
      serde_json::from_value(serde_json::json!({
        "role":"assistant","content":[{"type":"toolCall","id":"a","name":"read","arguments":{"path":"a"}}]
      }))
      .unwrap(),
    );
    for index in 0..100 {
      view
        .transcript
        .push(serde_json::from_value(serde_json::json!({"role":"user","content":format!("Message {index}")})).unwrap());
    }
    let id = super::message_id(&view, 0);
    let rows = std::rc::Rc::new(super::rows(&view));
    let count = rows.len();
    let (window, virtual_view) = cx.update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(800.), px(500.))))),
          ..Default::default()
        },
        cx,
        |window, cx| {
          let fixture = cx.new(|cx| CaptureTranscript::new(view, window, cx));
          let scroller = fixture.read(cx).scroller.clone();
          scroller.update(cx, |scroller, cx| scroller.reset(count, cx));
          cx.new(|_| VirtualTranscript {
            fixture,
            live: session.live,
            rows,
          })
        },
      )
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      let scroller = virtual_view.read(cx).fixture.read(cx).scroller.clone();
      scroller.update(cx, |scroller, cx| {
        assert!(scroller.scroll_to_item(0, cx));
      });
      window.render_frame(cx);
      let toggle = format!("{id}-part-0-toggle");
      window.click(toggle.clone(), cx);
      assert_eq!(window.find(toggle.clone()).checked(), Some(false));
      scroller.update(cx, |scroller, cx| {
        assert!(scroller.scroll_to_item(count - 1, cx));
      });
      window.render_frame(cx);
      assert!(window.try_find(toggle.clone()).is_none());
      scroller.update(cx, |scroller, cx| {
        assert!(scroller.scroll_to_item(0, cx));
      });
      window.render_frame(cx);
      assert_eq!(window.find(toggle).checked(), Some(false));
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn tool_state_survives_late_id_and_commit_but_resets_with_snapshot(cx: &mut TestAppContext) {
    use gpui_kit::WindowOptions;
    use gpui_kit::test::TestWindowExt as _;

    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_global(crate::ConversationPreferences::new(true, true, false));
    });
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.streaming = Some(Streaming {
      message_id: None,
      message: serde_json::from_value(serde_json::json!({
        "role":"assistant","content":[{"type":"toolCall","name":"read","arguments":{"path":"a"}}]
      }))
      .unwrap(),
    });
    let id = super::message_id(&view, 0);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| CaptureTranscript::new(view.clone(), window, cx))
      })
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let toggle = format!("{id}-part-0-toggle");
      window.click(toggle.clone(), cx);
      assert_eq!(window.find(toggle.clone()).checked(), Some(false));
      let streaming = view.streaming.as_mut().unwrap();
      streaming.message_id = Some("message-id".into());
      if let AgentMessage::Assistant(message) = &mut streaming.message
        && let Some(alvin_protocol::AssistantContent::ToolCall(call)) = message.content.as_mut().and_then(|parts| parts.first_mut())
      {
        call.id = Some("call-id".into());
      }
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      assert_eq!(window.find(toggle.clone()).checked(), Some(false));
      let streaming = view.streaming.take().unwrap();
      view.transcript.push(streaming.message);
      view.message_ids.push(streaming.message_id);
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      assert_eq!(window.find(toggle).checked(), Some(false));
      view.snapshot_generation += 1;
      fixture.update(cx, |fixture, cx| fixture.sync(&view, window, cx));
      window.render_frame(cx);
      let reset_id = super::message_id(&view, 0);
      assert_eq!(window.find(format!("{reset_id}-part-0-toggle")).checked(), Some(true));
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn concurrent_results_update_calls_in_call_order(cx: &mut TestAppContext) {
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_global(crate::ConversationPreferences::new(true, true, false));
    });
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"toolResult","toolCallId":"orphan","toolName":"read","content":[{"type":"text","text":"orphan"}]},
      {"role":"assistant","content":[
        {"type":"toolCall","id":"a","name":"read","arguments":{"path":"a"}},
        {"type":"toolCall","id":"b","name":"read","arguments":{"path":"b"}}
      ]},
      {"role":"toolResult","toolCallId":"b","toolName":"read","content":[{"type":"text","text":"B"}]},
      {"role":"toolResult","toolCallId":"a","toolName":"read","content":[{"type":"text","text":"A"}]}
    ]))
    .unwrap();
    let orphan_id = super::message_id(&view, 0);
    let id = super::message_id(&view, 1);
    let (window, _) = cx
      .update(|cx| {
        gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1000.), px(1800.))))),
            ..Default::default()
          },
          cx,
          |window, cx| cx.new(|cx| CaptureTranscript::new(view, window, cx)),
        )
      })
      .unwrap();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let a = window.find(format!("{id}-part-0-toggle")).bounds().top();
      let b = window.find(format!("{id}-part-1-toggle")).bounds().top();
      assert!(window.find(format!("{orphan_id}-toggle")).bounds().top() < a);
      assert!(a < b);
      assert_eq!(window.find_all_by_label("Collapse tool card").len(), 3);
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn reasoning_and_edit_disclosures_reopen_below_their_headers(cx: &mut TestAppContext) {
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, WindowBounds, WindowOptions, px, size};

    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(serde_json::json!([
      {"role":"assistant","content":[
        {"type":"thinking","thinking":"Consider this change"},
        {"type":"toolCall","id":"edit-1","name":"edit","arguments":{"path":"main.rs"}}
      ]},
      {"role":"toolResult","toolCallId":"edit-1","toolName":"edit",
       "content":[{"type":"text","text":"Edited main.rs"}],
       "details":{"diff":"-1|fn old() {}\n+1|fn new() {}"}}
    ]))
    .unwrap();
    let id = super::message_id(&view, 0);
    let (window, _) = cx
      .update(|cx| {
        gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1000.), px(1400.))))),
            ..Default::default()
          },
          cx,
          |window, cx| cx.new(|cx| CaptureTranscript::new(view, window, cx)),
        )
      })
      .unwrap();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let reasoning = format!("{id}-part-0-toggle");
      let reasoning_body = format!("{id}-part-0-details");
      let edit = format!("{id}-part-1-toggle");
      let edit_body = format!("{id}-part-1-details");
      assert!(window.try_find(reasoning_body.clone()).is_none());
      assert!(window.find(edit_body.clone()).visible());
      window.click(edit.clone(), cx);
      assert!(window.try_find(edit_body.clone()).is_none());
      for _ in 0..2 {
        window.click(reasoning.clone(), cx);
        let header = window.find(reasoning.clone()).bounds();
        let body = window.find(reasoning_body.clone()).bounds();
        assert!(body.top() >= header.bottom());
        assert_eq!(body.size.width, header.size.width);
        window.click(reasoning.clone(), cx);
        assert!(window.try_find(reasoning_body.clone()).is_none());

        window.click(edit.clone(), cx);
        let header = window.find(format!("{id}-part-1-header")).bounds();
        let body = window.find(edit_body.clone()).bounds();
        assert!(body.top() >= header.bottom());
        assert_eq!(body.size.width, header.size.width);
        assert!(window.find_by_label("fn new() {}").visible());
        window.click(edit.clone(), cx);
        assert!(window.try_find(edit_body.clone()).is_none());
      }
      window.click(edit.clone(), cx);
      window.click("collapse-file", cx);
      assert!(window.try_find_by_label("fn new() {}").is_none());
      window.click(edit.clone(), cx);
      window.click(edit.clone(), cx);
      assert!(window.try_find_by_label("fn new() {}").is_none());
      window.click("collapse-file", cx);
      assert!(window.find_by_label("fn new() {}").visible());
    })
    .unwrap();
  }
}
