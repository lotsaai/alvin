//! The transcript: committed messages plus the one streaming, in a `MessageScroller`.

use alvin_conversation::LiveView;
use alvin_protocol::{AgentMessage, AssistantContent, LiveSessionId, MessageContent, UserContent};
use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerLoadingStyle, MarkerVariant};
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent as MessageBody, MessageHeader};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{AnyElement, App, ElementId, Entity, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div};
use serde_json::Value;

use crate::model::Model;
use crate::tool_card::{ToolCard, fenced};

/// How many rows the transcript has: every committed message and, while one is being built, the streaming message.
pub(crate) fn row_count(view: &LiveView) -> usize {
  view.transcript.len() + usize::from(view.streaming.is_some())
}

/// The scroller of one live session. Rows are read from the model when the list asks for them, so no transcript is cloned.
pub(crate) fn transcript(model: &Entity<Model>, live: LiveSessionId, scroller: &Entity<MessageScrollerState>) -> MessageScroller {
  let model = model.clone();
  MessageScroller::new(
    ElementId::from(SharedString::from(format!("transcript-{}", live.as_uuid()))),
    scroller.clone(),
    move |index, _: &mut Window, cx: &mut App| row(&model, live, index, cx),
  )
  .w_full()
  .flex_1()
}

fn row(model: &Entity<Model>, live: LiveSessionId, index: usize, cx: &App) -> AnyElement {
  let Some(view) = model.read(cx).conversation().live(live) else {
    return div().into_any_element();
  };
  // Messages are append-only, so a message's position names it for as long as the transcript is loaded.
  let (id, message, streaming) = match view.transcript.get(index) {
    Some(message) => (format!("message-{}-{index}", live.as_uuid()), message, false),
    None => {
      let Some(streaming) = view.streaming.as_ref() else {
        return div().into_any_element();
      };
      let key = streaming.message_id.as_deref().unwrap_or("pending");
      (format!("streaming-{}-{key}", live.as_uuid()), &streaming.message, true)
    }
  };
  let id = SharedString::from(id);
  let body = message_row(&id, message, view, cx);
  if !streaming {
    return body;
  }
  v_flex()
    .w_full()
    .child(body)
    .child(
      Marker::new()
        .id(SharedString::from(format!("{id}-working")))
        .px_4()
        .loading(true)
        .with_loading_style(MarkerLoadingStyle::Shimmer)
        .content(MarkerContent::new().text("Working")),
    )
    .into_any_element()
}

fn message_row(id: &SharedString, message: &AgentMessage, view: &LiveView, cx: &App) -> AnyElement {
  let row = Message::new().id(ElementId::from(id.clone()));
  match message {
    AgentMessage::User(user) => {
      let text = user.content.as_ref().map(content_text).unwrap_or_default();
      row
        .alignment(MessageAlignment::End)
        .content(MessageBody::new().bubble(Bubble::new().with_variant(BubbleVariant::Filled).child(SharedString::from(text))))
        .into_any_element()
    }
    AgentMessage::Assistant(assistant) => {
      let blocks = assistant
        .content
        .iter()
        .flatten()
        .enumerate()
        .map(|(position, block)| assistant_block(id, position, block, view, cx))
        .collect::<Vec<_>>();
      row
        .header(MessageHeader::new().child("omp"))
        .content(
          MessageBody::new().bubble(
            Bubble::new()
              .with_variant(BubbleVariant::Ghost)
              .child(v_flex().gap_2().children(blocks)),
          ),
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
fn assistant_block(row: &SharedString, position: usize, block: &AssistantContent, view: &LiveView, cx: &App) -> AnyElement {
  let part = SharedString::from(format!("{row}-{position}"));
  let marker = |part: SharedString, text: &'static str| Marker::new().id(part).content(MarkerContent::new().text(text)).into_any_element();
  match block {
    AssistantContent::Text(text) => TextView::markdown(part, text.text.clone().unwrap_or_default()).into_any_element(),
    AssistantContent::Thinking(thinking) => div()
      .text_sm()
      .text_color(cx.theme().muted_foreground)
      .child(SharedString::from(thinking.thinking.clone().unwrap_or_default()))
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

  use alvin_protocol::MessagesPage;
  use gpui_kit::test::{TestAppContextExt as _, TestEventExt as _, TestQueryExt as _};
  use gpui_kit::{AppContext as _, MouseButton, TestAppContext};

  use crate::test_support::{live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  /// An omp turn that called every built-in tool once, as omp recorded it.
  const TOOLS_TURN: &str = include_str!("../fixtures/tools_turn.messages.json");

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn a_tool_call_is_a_card_that_opens_on_its_toggle(cx: &mut TestAppContext) {
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
        !window.find_all_by_label("Show tool details").is_empty()
      })
      .await;

      cx.update_window(opened.handle, |_, window, cx| {
        let closed = window.find_all_by_label("Show tool details").len();
        let toggle = window.find_all_by_label("Show tool details")[0].bounds().center();
        window.pointer_down(toggle, MouseButton::Left, cx);
        window.pointer_up(toggle, MouseButton::Left, cx);
        // The card opened, and only that one.
        assert_eq!(window.find_all_by_label("Hide tool details").len(), 1);
        assert_eq!(window.find_all_by_label("Show tool details").len(), closed - 1);
      })
      .unwrap();
    });
  }
}
