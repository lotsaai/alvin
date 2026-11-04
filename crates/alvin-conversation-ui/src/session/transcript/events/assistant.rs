use std::collections::HashMap;
use std::rc::Rc;

use alvin_protocol::AgentMessage;
use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::message::{Message, MessageContent};
use gpui_kit::{AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, Render, SharedString, Styled as _, Window};

use super::super::content::ContentBlocks;
use super::super::tool::ToolInput;
use crate::session::message_chrome::{message_context, message_footer};

pub(in crate::session::transcript) struct AssistantBlock {
  id: SharedString,
  message: Rc<AgentMessage>,
  composer: Entity<EditorState>,
  collapsed: Entity<bool>,
  content: ContentBlocks,
  streaming: bool,
}

impl AssistantBlock {
  pub(in crate::session::transcript) fn new(
    id: SharedString,
    message: Rc<AgentMessage>,
    composer: Entity<EditorState>,
    cx: &mut Context<Self>,
  ) -> Self {
    let collapsed = cx.new(|_| false);
    cx.observe(&collapsed, |_, _, cx| cx.notify()).detach();
    Self {
      content: ContentBlocks::new(id.clone()),
      id,
      message,
      composer,
      collapsed,
      streaming: false,
    }
  }

  pub(in crate::session::transcript) fn sync(
    &mut self,
    message: Rc<AgentMessage>,
    tools: &HashMap<usize, ToolInput>,
    streaming: bool,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    if !Rc::ptr_eq(&self.message, &message) || self.streaming != streaming {
      self.message = message;
      self.streaming = streaming;
      cx.notify();
    }
    if let AgentMessage::Assistant(message) = self.message.as_ref() {
      self
        .content
        .sync(message.content.as_deref().unwrap_or_default(), tools, streaming, window, cx);
    }
  }
}

impl Render for AssistantBlock {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let message = self.message.as_ref();
    let content = if *self.collapsed.read(cx) {
      gpui_kit::div().child("Message collapsed").into_any_element()
    } else {
      let mut content = gpui_kit::component::v_flex().w_full().gap_2().child(self.content.view(cx));
      if let AgentMessage::Assistant(assistant) = message {
        if let Some(error) = assistant.error_message.as_deref() {
          content = content.child(super::super::content::error_view(error, cx));
        }
        if let Some(status) = assistant.error_status {
          content = content.child(super::super::content::error_view(&format!("Error code {status}"), cx));
        }
      }
      content.into_any_element()
    };
    let row = Message::new()
      .id(self.id.clone())
      .content(MessageContent::new().bubble(Bubble::new().with_variant(BubbleVariant::Ghost).child(content)))
      .footer(message_footer(
        self.id.clone(),
        message,
        Some(&self.composer),
        Some(&self.collapsed),
        cx,
      ))
      .into_any_element();
    message_context(&self.id, row, message, Some(&self.composer), Some(&self.collapsed), cx)
  }
}
