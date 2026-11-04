//! The transcript: committed messages plus the one streaming, in a `MessageScroller`.

use std::rc::Rc;

use alvin_conversation::LiveView;
use alvin_protocol::{AgentMessage, AssistantContent, LiveSessionId, MessageContent, UserContent};
use gpui_kit::base::motion::{MotionReveal, spring};
use gpui_kit::component::bubble::{Bubble, BubbleContent, BubbleVariant};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerLoadingStyle, MarkerVariant};
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent as MessageBody, MessageHeader};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, ElementId, Entity, Global, Hsla, InteractiveElement as _, IntoElement as _, ParentElement as _, RenderOnce,
  SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use serde_json::Value;

use super::tool_card::{ToolCard, fenced};
use crate::model::Model;

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

fn ancillary(message: &AgentMessage) -> bool {
  matches!(
    message,
    AgentMessage::Custom(_) | AgentMessage::Developer(_) | AgentMessage::HookMessage(_) | AgentMessage::FileMention(_)
  )
}

fn response_message(message: &AgentMessage) -> bool {
  matches!(message, AgentMessage::Assistant(_) | AgentMessage::ToolResult(_)) || ancillary(message)
}

/// Group response messages and injected context after a visible user message.
pub(crate) fn rows(view: &LiveView) -> Vec<Row> {
  let count = view.transcript.len() + usize::from(view.streaming.is_some());
  let mut rows = Vec::new();
  let mut index = 0;
  let mut after_user = false;
  while index < count {
    match message_at(view, index).map(|(message, _)| message) {
      Some(AgentMessage::User(_)) => {
        rows.push(Row::Message(index));
        after_user = true;
        index += 1;
      }
      Some(message) if after_user && response_message(message) => {
        let start = index;
        let mut has_assistant = false;
        while let Some((message, _)) = message_at(view, index) {
          if !response_message(message) {
            break;
          }
          has_assistant |= matches!(message, AgentMessage::Assistant(_));
          index += 1;
        }
        if has_assistant {
          rows.push(Row::Response { start, end: index });
        } else {
          rows.extend((start..index).map(Row::Message));
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

/// The scroller of one live session. Rows are read from the model when the list asks for them, so no transcript is cloned.
pub(crate) fn transcript(
  model: &Entity<Model>,
  live: LiveSessionId,
  scroller: &Entity<MessageScrollerState>,
  rows: Rc<Vec<Row>>,
) -> MessageScroller {
  let model = model.clone();
  MessageScroller::new(
    ElementId::from(SharedString::from(format!("transcript-{}", live.as_uuid()))),
    scroller.clone(),
    move |index, _: &mut Window, cx: &mut App| row(&model, live, rows.get(index), cx),
  )
  .w_full()
  .flex_1()
}

fn row(model: &Entity<Model>, live: LiveSessionId, row: Option<&Row>, cx: &App) -> AnyElement {
  let Some(view) = model.read(cx).conversation().live(live) else {
    return div().into_any_element();
  };
  let Some(row) = row else {
    return div().into_any_element();
  };
  let (index, message, streaming) = match row {
    Row::Message(index) => {
      let Some((message, streaming)) = message_at(view, *index) else {
        return div().into_any_element();
      };
      (*index, Some(message), streaming)
    }
    Row::Response { start, .. } => (*start, None, false),
  };
  let id = SharedString::from(format!("message-{}-{index}", live.as_uuid()));
  let body = match (row, message) {
    (Row::Response { start, end }, _) => response(&id, model, live, view, *start, *end, cx),
    (_, Some(message)) => message_row(&id, message, view, streaming, cx),
    _ => return div().into_any_element(),
  };
  let body = if !streaming {
    body
  } else {
    v_flex()
      .w_full()
      .child(body)
      .child(
        Marker::new()
          .id(SharedString::from(format!("{id}-working")))
          .loading(true)
          .with_loading_style(MarkerLoadingStyle::Shimmer)
          .content(MarkerContent::new().text("Working")),
      )
      .into_any_element()
  };
  h_flex()
    .group("message-actions")
    .w_full()
    .justify_center()
    .px_1()
    .child(div().w_full().max_w(gpui_kit::rems(42.)).min_w_0().child(body))
    .into_any_element()
}

fn final_block(view: &LiveView, end: usize) -> Option<(usize, usize)> {
  for index in (0..end).rev() {
    let (message, _) = message_at(view, index)?;
    if ancillary(message) {
      continue;
    }
    let AgentMessage::Assistant(assistant) = message else {
      return None;
    };
    let blocks = assistant.content.as_ref()?;
    let position = blocks.len().checked_sub(1)?;
    return matches!(blocks.get(position), Some(AssistantContent::Text(_))).then_some((index, position));
  }
  None
}

fn process_children(id: &SharedString, view: &LiveView, start: usize, end: usize, cx: &App) -> Vec<AnyElement> {
  let final_block = final_block(view, end);
  let mut process = Vec::new();
  let mut calls = std::collections::HashSet::new();
  for index in start..end {
    let Some((message, is_streaming)) = message_at(view, index) else {
      continue;
    };
    let message_id = SharedString::from(format!("{id}-{index}"));
    match message {
      AgentMessage::Assistant(assistant) => {
        if let Some(error) = &assistant.error_message {
          process.push(
            div()
              .text_color(cx.theme().danger)
              .child(SharedString::from(error.clone()))
              .into_any_element(),
          );
        }
        for (position, block) in assistant.content.iter().flatten().enumerate() {
          if final_block == Some((index, position)) {
            continue;
          }
          match block {
            AssistantContent::Thinking(thinking) => {
              process.push(
                div()
                  .text_sm()
                  .text_color(cx.theme().muted_foreground)
                  .child(SharedString::from(thinking.thinking.clone().unwrap_or_default()))
                  .into_any_element(),
              );
            }
            AssistantContent::ToolCall(call) => {
              let call_id = call.id.as_deref().unwrap_or_default();
              let result = if call_id.is_empty() {
                None
              } else {
                (index + 1..end).find_map(|next| match message_at(view, next) {
                  Some((AgentMessage::ToolResult(result), _)) if result.tool_call_id.as_deref() == Some(call_id) => Some(result),
                  _ => None,
                })
              };
              if result.is_some() {
                calls.insert(call_id.to_owned());
              }
              let card_id = if call_id.is_empty() {
                format!("{message_id}-{position}")
              } else {
                call_id.to_owned()
              };
              let mut card = ToolCard::of_call(
                &card_id,
                call.name.as_deref().unwrap_or("tool"),
                call.intent.as_deref(),
                call.arguments.clone().map(Value::Object),
                view.tools.get(call_id),
              );
              card.expanded = true;
              if let Some(result) = result {
                card.result = Some(serde_json::json!({ "content": result.content, "details": result.details }));
                card.running = false;
                card.failed |= result.is_error.unwrap_or(false);
              }
              process.push(card.into_any_element());
            }
            _ => process.push(assistant_block(&message_id, position, block, view, is_streaming)),
          }
        }
      }
      AgentMessage::ToolResult(result) => {
        if !result.tool_call_id.as_ref().is_some_and(|call| calls.remove(call)) {
          process.push(message_row(&message_id, message, view, is_streaming, cx));
        }
      }
      _ => process.push(message_row(&message_id, message, view, is_streaming, cx)),
    }
  }
  process
}

fn response(
  id: &SharedString,
  model: &Entity<Model>,
  live: LiveSessionId,
  view: &LiveView,
  start: usize,
  end: usize,
  cx: &App,
) -> AnyElement {
  let final_block = final_block(view, end);
  let mut steps = 0usize;
  let mut failed = false;
  let mut has_process = false;
  let mut streaming = false;
  for index in start..end {
    let Some((message, is_streaming)) = message_at(view, index) else {
      continue;
    };
    streaming |= is_streaming;
    match message {
      AgentMessage::Assistant(assistant) => {
        failed |= assistant.error_message.is_some();
        has_process |= assistant.error_message.is_some();
        for (position, block) in assistant.content.iter().flatten().enumerate() {
          if final_block != Some((index, position)) {
            has_process = true;
          }
          if let AssistantContent::ToolCall(call) = block {
            steps += 1;
            failed |= call
              .id
              .as_deref()
              .and_then(|id| view.tools.get(id))
              .is_some_and(|execution| execution.outcome.as_ref().is_some_and(|outcome| outcome.is_error));
          }
        }
      }
      AgentMessage::ToolResult(result) => {
        has_process = true;
        failed |= result.is_error.unwrap_or(false);
      }
      _ => has_process = true,
    }
  }
  let answer = final_block.and_then(|(index, position)| {
    let (message @ AgentMessage::Assistant(assistant), is_streaming) = message_at(view, index)? else {
      return None;
    };
    let block = assistant.content.as_ref()?.get(position)?;
    let message_id = SharedString::from(format!("{id}-{index}"));
    let content = assistant_block(&message_id, position, block, view, is_streaming);
    Some(
      Message::new()
        .id(ElementId::from(message_id.clone()))
        .content(MessageBody::new().bubble(Bubble::new().with_variant(BubbleVariant::Ghost).child(content)))
        .footer(super::message_chrome::message_footer(message_id, message, cx)),
    )
  });
  let mut body = v_flex().w_full().gap_2();
  if has_process {
    body = body.child(Process {
      id: id.clone(),
      model: model.clone(),
      live,
      start,
      end,
      steps,
      failed,
      initially_open: answer.is_none(),
    });
  }
  if let Some(answer) = answer {
    body = body.child(answer);
  }
  if streaming {
    body = body.child(
      Marker::new()
        .id(SharedString::from(format!("{id}-working")))
        .loading(true)
        .with_loading_style(MarkerLoadingStyle::Shimmer)
        .content(MarkerContent::new().text("Working")),
    );
  }
  body.into_any_element()
}

#[derive(gpui_kit::IntoElement)]
struct Process {
  id: SharedString,
  model: Entity<Model>,
  live: LiveSessionId,
  start: usize,
  end: usize,
  steps: usize,
  failed: bool,
  initially_open: bool,
}

impl RenderOnce for Process {
  fn render(self, window: &mut Window, cx: &mut App) -> impl gpui_kit::IntoElement {
    let open = window.use_keyed_state(ElementId::from(SharedString::from(format!("{}-process", self.id))), cx, |_, _| {
      self.initially_open
    });
    let is_open = *open.read(cx);
    let label = if self.steps == 0 {
      "Process".to_owned()
    } else if self.steps == 1 {
      "Ran 1 step".to_owned()
    } else {
      format!("Ran {} steps", self.steps)
    };
    let label = if self.failed { format!("{label} · failed") } else { label };
    let toggle = Button::new(SharedString::from(format!("{}-process-toggle", self.id)))
      .ghost()
      .xsmall()
      .label(label)
      .accessibility_label(if is_open { "Hide process" } else { "Show process" })
      .on_click(move |_, _, cx| {
        open.update(cx, |open, cx| {
          *open = !*open;
          cx.notify();
        })
      });
    let progress = spring(
      (ElementId::from(self.id.clone()), "process-reveal"),
      if is_open { 1. } else { 0. },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    let children = (is_open || progress > 0.).then(|| {
      self
        .model
        .read(cx)
        .conversation()
        .live(self.live)
        .map(|view| process_children(&self.id, view, self.start, self.end, cx))
        .unwrap_or_default()
    });
    v_flex()
      .w_full()
      .gap_2()
      .child(h_flex().child(toggle))
      .when_some(children, |this, children| {
        this.child(MotionReveal::new(
          ElementId::from(SharedString::from(format!("{}-process-reveal", self.id))),
          progress,
          v_flex().gap_2().children(children).into_any_element(),
        ))
      })
  }
}

fn message_row(id: &SharedString, message: &AgentMessage, view: &LiveView, streaming: bool, cx: &App) -> AnyElement {
  let row = Message::new().id(ElementId::from(id.clone()));
  match message {
    AgentMessage::User(user) => {
      let text = user.content.as_ref().map(content_text).unwrap_or_default();
      let colors = UserMessageColors::current(cx);
      row
        .alignment(MessageAlignment::End)
        .content(
          MessageBody::new().bubble(
            Bubble::new()
              .with_variant(BubbleVariant::Filled)
              .content(BubbleContent::new().bg(colors.background()).text_color(colors.foreground()))
              .child(TextView::markdown(SharedString::from(format!("{id}-text")), text)),
          ),
        )
        .footer(super::message_chrome::message_footer(id.clone(), message, cx))
        .into_any_element()
    }
    AgentMessage::Assistant(assistant) => {
      let blocks = assistant
        .content
        .iter()
        .flatten()
        .enumerate()
        .map(|(position, block)| assistant_block(id, position, block, view, streaming))
        .collect::<Vec<_>>();
      row
        .content(
          MessageBody::new().bubble(
            Bubble::new()
              .with_variant(BubbleVariant::Ghost)
              .child(v_flex().gap_2().children(blocks)),
          ),
        )
        .when(
          matches!(
            assistant.content.as_ref().and_then(|blocks| blocks.last()),
            Some(AssistantContent::Text(_))
          ),
          |row| row.footer(super::message_chrome::message_footer(id.clone(), message, cx)),
        )
        .into_any_element()
    }
    AgentMessage::ToolResult(result) => {
      let text = result
        .content
        .iter()
        .flatten()
        .map(user_content_text)
        .collect::<Vec<_>>()
        .join("\n");
      let name = result.tool_name.clone().unwrap_or_else(|| "tool".to_owned());
      note(row, id, &format!("{name} result"), &text)
    }
    AgentMessage::BashExecution(bash) => {
      let command = bash.command.clone().unwrap_or_default();
      let output = bash.output.clone().unwrap_or_default();
      note(row, id, &format!("$ {command}"), &output)
    }
    AgentMessage::BranchSummary(summary) => boundary(row, id, "Branch summary", summary.summary.as_deref().unwrap_or_default()),
    AgentMessage::CompactionSummary(summary) => boundary(row, id, "Compaction summary", summary.summary.as_deref().unwrap_or_default()),
    AgentMessage::Custom(custom) => note(
      row,
      id,
      custom.custom_type.as_deref().unwrap_or("custom"),
      &custom.content.as_ref().map(content_text).unwrap_or_default(),
    ),
    AgentMessage::Developer(developer) => note(
      row,
      id,
      "Developer",
      &developer.content.as_ref().map(content_text).unwrap_or_default(),
    ),
    AgentMessage::PythonExecution(python) => note(
      row,
      id,
      &format!("python {}", python.code.as_deref().unwrap_or_default()),
      python.output.as_deref().unwrap_or_default(),
    ),
    AgentMessage::HookMessage(hook) => note(
      row,
      id,
      hook.custom_type.as_deref().unwrap_or("hook"),
      &hook.content.as_ref().map(content_text).unwrap_or_default(),
    ),
    AgentMessage::FileMention(mention) => note(
      row,
      id,
      "Files",
      &mention
        .files
        .iter()
        .flatten()
        .filter_map(|file| file.path.as_deref())
        .collect::<Vec<_>>()
        .join("\n"),
    ),
  }
}

/// One block of an assistant message.
fn assistant_block(row: &SharedString, position: usize, block: &AssistantContent, view: &LiveView, streaming: bool) -> AnyElement {
  let part = SharedString::from(format!("{row}-{position}"));
  let marker = |part: SharedString, text: &'static str| Marker::new().id(part).content(MarkerContent::new().text(text)).into_any_element();
  match block {
    AssistantContent::Text(text) => TextView::markdown(part, text.text.clone().unwrap_or_default())
      .stream_fade(streaming)
      .into_any_element(),
    AssistantContent::Thinking(thinking) => Reasoning {
      id: part,
      text: thinking.thinking.clone().unwrap_or_default(),
    }
    .into_any_element(),
    AssistantContent::RedactedThinking(_) => marker(part, "Reasoning hidden by the provider"),
    AssistantContent::ToolCall(call) => {
      let call_id = call.id.as_deref().unwrap_or_default();
      // A call without an id is still one card of its own, named by its place in the message.
      let card_id = if call_id.is_empty() { part.to_string() } else { call_id.to_owned() };
      ToolCard::of_call(
        &card_id,
        call.name.as_deref().unwrap_or("tool"),
        call.intent.as_deref(),
        call.arguments.clone().map(Value::Object),
        view.tools.get(call_id),
      )
      .into_any_element()
    }
    AssistantContent::Image(_) => marker(part, "Image"),
    AssistantContent::Fallback(_) => marker(part, "Fallback provider block"),
    AssistantContent::AnthropicServerTool(_) => marker(part, "Server tool block"),
  }
}

#[derive(gpui_kit::IntoElement)]
struct Reasoning {
  id: SharedString,
  text: String,
}

impl RenderOnce for Reasoning {
  fn render(self, window: &mut Window, cx: &mut App) -> impl gpui_kit::IntoElement {
    let open = window.use_keyed_state(ElementId::from(self.id.clone()), cx, |_, _| false);
    let is_open = *open.read(cx);
    let toggle = Button::new(SharedString::from(format!("{}-toggle", self.id)))
      .ghost()
      .xsmall()
      .label(if is_open { "Hide reasoning" } else { "Show reasoning" })
      .on_click(move |_, _, cx| {
        open.update(cx, |open, cx| {
          *open = !*open;
          cx.notify();
        })
      });
    let progress = spring(
      (ElementId::from(self.id.clone()), "reasoning-reveal"),
      if is_open { 1. } else { 0. },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    v_flex()
      .w_full()
      .child(h_flex().child(toggle))
      .when(is_open || progress > 0., |this| {
        this.child(MotionReveal::new(
          ElementId::from(SharedString::from(format!("{}-reveal", self.id))),
          progress,
          div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(self.text))
            .into_any_element(),
        ))
      })
  }
}

/// A titled block of text that is not the user's and not the assistant's: output, results, custom messages.
fn note(row: Message, id: &SharedString, title: &str, text: &str) -> AnyElement {
  row
    .header(MessageHeader::new().child(SharedString::from(title.to_owned())))
    .content(
      MessageBody::new().bubble(
        Bubble::new()
          .with_variant(BubbleVariant::Muted)
          .child(TextView::markdown(SharedString::from(format!("{id}-text")), fenced(text))),
      ),
    )
    .into_any_element()
}

/// A point in the transcript that stands for what came before: a separator, and the summary under it.
fn boundary(row: Message, id: &SharedString, title: &str, summary: &str) -> AnyElement {
  v_flex()
    .w_full()
    .child(
      Marker::new()
        .id(SharedString::from(format!("{id}-marker")))
        .with_variant(MarkerVariant::Separator)
        .content(MarkerContent::new().text(SharedString::from(title.to_owned()))),
    )
    .child(
      row.content(
        MessageBody::new().bubble(
          Bubble::new()
            .with_variant(BubbleVariant::Ghost)
            .child(TextView::markdown(SharedString::from(format!("{id}-text")), summary.to_owned())),
        ),
      ),
    )
    .into_any_element()
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
  use std::time::Duration;

  use alvin_conversation::{Conversation, Streaming};
  use alvin_protocol::{AgentMessage, MessagesPage};
  use gpui_kit::component::ActiveTheme as _;
  use gpui_kit::test::{TestAppContextExt as _, TestEventExt as _, TestQueryExt as _};
  use gpui_kit::{AppContext as _, MouseButton, TestAppContext};

  use super::{Row, UserMessageColors, final_block, rows};
  use crate::test_support::{live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  /// An omp turn that called every built-in tool once, as omp recorded it.
  const TOOLS_TURN: &str = include_str!("../../fixtures/tools_turn.messages.json");

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
    assert_eq!(final_block(&view, 6), Some((4, 0)));
    assert_eq!(final_block(&view, 9), Some((8, 0)));
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
    assert!(final_block(&view, end).is_some());
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
    assert_eq!(final_block(&view, end + 2), None);
    view.streaming.as_mut().unwrap().message = serde_json::from_value(serde_json::json!({
      "role": "assistant", "content": [
        {"type": "thinking", "thinking": "working"}, {"type": "text", "text": "answer"}
      ]
    }))
    .unwrap();
    assert_eq!(final_block(&view, end + 2), Some((end + 1, 1)));
    view.transcript.remove(0);
    assert_eq!(rows(&view).first(), Some(&Row::Message(0)));
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

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn process_discloses_all_tool_cards_together(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let session = live_session("/work/api");
      let page: MessagesPage = serde_json::from_str(TOOLS_TURN).unwrap();
      cx.update(|cx| {
        opened.model.update(cx, |model, cx| {
          model.seed_open(&session, cx);
          model.seed_messages(session.live, page, cx);
        });
      });
      cx.wait_for(opened.handle, BUDGET, |window, _| {
        !window.find_all_by_label("Show process").is_empty()
      })
      .await;

      cx.update_window(opened.handle, |_, window, cx| {
        assert_eq!(window.find_all_by_label("Show process").len(), 1);
        assert!(window.find_all_by_label("Show tool details").is_empty());
        let toggle = window.find_all_by_label("Show process")[0].bounds().center();
        window.pointer_down(toggle, MouseButton::Left, cx);
        window.pointer_up(toggle, MouseButton::Left, cx);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| {
        !window.find_all_by_label("Hide process").is_empty()
      })
      .await;

      cx.update_window(opened.handle, |_, window, _| {
        assert_eq!(window.find_all_by_label("Hide process").len(), 1);
        assert!(window.find_all_by_label("Show tool details").is_empty());
      })
      .unwrap();
    });
  }
}
