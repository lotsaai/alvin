use std::collections::HashMap;
use std::rc::Rc;

use alvin_protocol::AgentMessage;
use gpui_kit::component::bubble::{Bubble, BubbleVariant};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::message::{Message, MessageContent};
use gpui_kit::{AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, Render, SharedString, Styled as _, Window};

use super::super::content::{ContentBlocks, Part};
use super::super::tool::ToolInput;
use super::custom::{SystemActivity, SystemActivityData};
use crate::session::message_chrome::{message_context, message_footer};

pub(in crate::session::transcript) struct AssistantBlock {
  id: SharedString,
  message: Rc<AgentMessage>,
  composer: Entity<EditorState>,
  collapsed: Entity<bool>,
  content: ContentBlocks,
  diagnostic: Option<Entity<SystemActivity>>,
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
    let diagnostic = match message.as_ref() {
      AgentMessage::Assistant(message) => SystemActivityData::assistant(message)
        .map(|data| cx.new(|cx| SystemActivity::new_activity(format!("{id}-diagnostic").into(), data, cx))),
      _ => None,
    };
    Self {
      content: ContentBlocks::new(id.clone()),
      id,
      message,
      composer,
      collapsed,
      diagnostic,
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
    if !Rc::ptr_eq(&self.message, &message) && self.message.as_ref() != message.as_ref() {
      let data = match message.as_ref() {
        AgentMessage::Assistant(message) => SystemActivityData::assistant(message),
        _ => None,
      };
      match (&self.diagnostic, data) {
        (Some(diagnostic), Some(data)) => diagnostic.update(cx, |activity, cx| activity.sync_activity(data, cx)),
        (None, Some(data)) => {
          self.diagnostic = Some(cx.new(|cx| SystemActivity::new_activity(format!("{}-diagnostic", self.id).into(), data, cx)));
        }
        (_, None) => self.diagnostic = None,
      }
    }
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

  pub(in crate::session::transcript) fn parts(&self, cx: &gpui_kit::App) -> Option<&[Part]> {
    (!*self.collapsed.read(cx)).then(|| self.content.parts())
  }

  pub(in crate::session::transcript) fn may_have_answer(&self) -> bool {
    match self.message.as_ref() {
      AgentMessage::Assistant(message) => self.streaming || message.stop_reason != Some(alvin_protocol::StopReason::ToolUse),
      _ => false,
    }
  }

  pub(in crate::session::transcript) fn collapsed_answer(&self, cx: &gpui_kit::App) -> bool {
    *self.collapsed.read(cx)
      && matches!(self.message.as_ref(), AgentMessage::Assistant(message)
        if message.content.as_ref().is_some_and(|parts|
          parts.iter().any(|part| matches!(part, alvin_protocol::AssistantContent::Text(_)))))
  }

  pub(in crate::session::transcript) fn diagnostic(&self) -> Option<&Entity<SystemActivity>> {
    self.diagnostic.as_ref()
  }

  pub(in crate::session::transcript) fn footer(&self, cx: &gpui_kit::App) -> gpui_kit::AnyElement {
    let footer = message_footer(
      self.id.clone(),
      self.message.as_ref(),
      Some(&self.composer),
      Some(&self.collapsed),
      cx,
    )
    .into_any_element();
    message_context(
      &self.id,
      footer,
      self.message.as_ref(),
      Some(&self.composer),
      Some(&self.collapsed),
      cx,
    )
  }
}

impl Render for AssistantBlock {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let message = self.message.as_ref();
    let content = if *self.collapsed.read(cx) {
      gpui_kit::div().child("Message collapsed").into_any_element()
    } else {
      gpui_kit::component::v_flex()
        .w_full()
        .gap_2()
        .child(self.content.view(cx))
        .children(self.diagnostic.iter().cloned())
        .into_any_element()
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

#[cfg(test)]
mod tests {
  use std::collections::HashMap;
  use std::rc::Rc;

  use alvin_protocol::AgentMessage;
  use gpui_kit::component::input::EditorState;
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{AppContext as _, TestAppContext, WindowOptions};
  use serde_json::json;

  #[gpui_kit::test]
  fn diagnostic_entity_keeps_disclosure_when_message_changes(cx: &mut TestAppContext) {
    let message = |reason: &str| -> Rc<AgentMessage> {
      Rc::new(
        serde_json::from_value(json!({
          "role": "assistant", "stopReason": "aborted", "errorMessage": reason
        }))
        .unwrap(),
      )
    };
    cx.update(gpui_kit::init);
    let (window, block) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        let composer = cx.new(|cx| EditorState::new(window, cx));
        cx.new(|cx| super::AssistantBlock::new("answer".into(), message("first"), composer, cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let original = block.read(cx).diagnostic().unwrap().entity_id();
      window.click("answer-diagnostic-details", cx);
      assert!(window.find("answer-diagnostic-body-container").visible());
      block.update(cx, |block, cx| block.sync(message("updated"), &HashMap::new(), false, window, cx));
      window.render_frame(cx);
      assert_eq!(block.read(cx).diagnostic().unwrap().entity_id(), original);
      assert!(window.find("answer-diagnostic-body-container").visible());
      window.click("answer-diagnostic-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("updated".to_owned()));
    })
    .expect("inspect assistant");
  }
}
