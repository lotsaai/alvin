//! The transcript: committed messages plus the one streaming, in a `MessageScroller`.

use alvin_conversation::{LiveView, Streaming};
use alvin_protocol::{AgentMessage, AssistantContent, LiveSessionId, MessageContent, UserContent};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{
  AnyElement, App, ElementId, Entity, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window,
  div, prelude::FluentBuilder as _,
};
use serde_json::Value;

use crate::model::Model;
use crate::tool_card::ToolCard;

/// How many rows the transcript has: every committed message and, while one is being built, the streaming message.
pub(crate) fn row_count(view: &LiveView) -> usize {
  view.transcript.len() + usize::from(view.streaming.is_some())
}

/// The scroller of one live session. Rows are read from the model when the list asks for them, so no transcript is cloned.
pub(crate) fn transcript(model: &Entity<Model>, live: LiveSessionId, scroller: &Entity<MessageScrollerState>) -> MessageScroller {
  let model = model.clone();
  MessageScroller::new(
    ElementId::from(SharedString::from(format!("transcript-{live:?}"))),
    scroller.clone(),
    move |index, _window: &mut Window, cx: &mut App| row(&model, live, index, cx),
  )
  .w_full()
  .flex_1()
}

fn row(model: &Entity<Model>, live: LiveSessionId, index: usize, cx: &App) -> AnyElement {
  let model = model.read(cx);
  let Some(view) = model.conversation().live(live) else {
    return div().into_any_element();
  };
  match view.transcript.get(index) {
    Some(message) => message_row(live, index, message, view, cx),
    None => match &view.streaming {
      Some(streaming) => streaming_row(live, streaming, view, cx),
      None => div().into_any_element(),
    },
  }
}

fn message_row(live: LiveSessionId, index: usize, message: &AgentMessage, view: &LiveView, cx: &App) -> AnyElement {
  // Messages are append-only, so a message's position names it for as long as the transcript is loaded.
  let id = ElementId::from(SharedString::from(format!("message-{live:?}-{index}")));
  frame(id, message_body(message, view, cx), cx)
}

fn streaming_row(live: LiveSessionId, streaming: &Streaming, view: &LiveView, cx: &App) -> AnyElement {
  let key = streaming.message_id.clone().unwrap_or_else(|| "pending".to_owned());
  let id = ElementId::from(SharedString::from(format!("streaming-{live:?}-{key}")));
  frame(id, message_body(&streaming.message, view, cx), cx)
}

fn frame(id: ElementId, body: AnyElement, cx: &App) -> AnyElement {
  v_flex()
    .id(id)
    .w_full()
    .px_4()
    .py_2()
    .gap_2()
    .text_color(cx.theme().foreground)
    .child(body)
    .into_any_element()
}

fn message_body(message: &AgentMessage, view: &LiveView, cx: &App) -> AnyElement {
  match message {
    AgentMessage::User(user) => labelled("You", user.content.as_ref().map(content_text).unwrap_or_default(), cx),
    AgentMessage::Assistant(assistant) => {
      let blocks = assistant.content.iter().flatten().enumerate().map(|(position, block)| match block {
        AssistantContent::Text(text) => {
          let text = text.text.clone().unwrap_or_default();
          TextView::markdown(SharedString::from(format!("text-{position}")), text).into_any_element()
        }
        AssistantContent::Thinking(thinking) => div()
          .text_color(cx.theme().muted_foreground)
          .child(SharedString::from(thinking.thinking.clone().unwrap_or_default()))
          .into_any_element(),
        AssistantContent::ToolCall(call) => {
          let id = call.id.clone().unwrap_or_default();
          let execution = view.tools.get(&id);
          ToolCard::of_call(
            &id,
            call.name.as_deref().unwrap_or("tool"),
            call.intent.as_deref(),
            call.arguments.clone().map(Value::Object),
            execution,
          )
          .render(cx)
          .into_any_element()
        }
        other => div()
          .text_color(cx.theme().muted_foreground)
          .child(SharedString::from(format!("{other:?}")))
          .into_any_element(),
      });
      v_flex()
        .gap_2()
        .child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("omp"))
        .children(blocks)
        .into_any_element()
    }
    AgentMessage::ToolResult(result) => {
      let text: String = result
        .content
        .iter()
        .flatten()
        .map(user_content_text)
        .collect::<Vec<_>>()
        .join("\n");
      let name = result.tool_name.clone().unwrap_or_else(|| "tool".to_owned());
      labelled(&format!("{name} result"), text, cx)
    }
    AgentMessage::BashExecution(bash) => {
      let command = bash.command.clone().unwrap_or_default();
      let output = bash.output.clone().unwrap_or_default();
      labelled(&format!("$ {command}"), output, cx)
    }
    AgentMessage::BranchSummary(summary) => labelled("Branch summary", summary.summary.clone().unwrap_or_default(), cx),
    AgentMessage::CompactionSummary(summary) => labelled("Compaction summary", summary.summary.clone().unwrap_or_default(), cx),
    AgentMessage::Custom(custom) => labelled(
      custom.custom_type.as_deref().unwrap_or("custom"),
      custom.content.as_ref().map(content_text).unwrap_or_default(),
      cx,
    ),
    AgentMessage::Developer(_) | AgentMessage::PythonExecution(_) | AgentMessage::HookMessage(_) | AgentMessage::FileMention(_) => {
      div().into_any_element()
    }
  }
}

fn labelled(label: &str, text: String, cx: &App) -> AnyElement {
  v_flex()
    .gap_1()
    .child(
      div()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(SharedString::from(label.to_owned())),
    )
    .when(!text.is_empty(), |this| this.child(div().child(SharedString::from(text))))
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
