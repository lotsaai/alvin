use std::rc::Rc;

use alvin_protocol::{AgentMessage, MessageContent, UserContent};
use gpui_kit::component::bubble::{Bubble, BubbleContent, BubbleVariant};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::message::{Message, MessageAlignment, MessageContent as MessageBody};
use gpui_kit::component::text::TextView;
use gpui_kit::{AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, Render, SharedString, Styled as _, Window};

use super::super::content::image::PreparedImage;
use super::super::{UserMessageColors, content_text};
use crate::session::message_chrome::{message_context, message_footer};

pub(in crate::session::transcript) struct UserBlock {
  id: SharedString,
  message: Rc<AgentMessage>,
  text: SharedString,
  composer: Entity<EditorState>,
  collapsed: Entity<bool>,
  images: Vec<PreparedImage>,
}

impl UserBlock {
  fn images(message: &AgentMessage, previous: Vec<PreparedImage>) -> Vec<PreparedImage> {
    let AgentMessage::User(user) = message else { return Vec::new() };
    let mut previous = previous.into_iter();
    user
      .content
      .iter()
      .filter_map(|content| match content {
        MessageContent::UserContentList(parts) => Some(parts),
        MessageContent::String(_) => None,
      })
      .flatten()
      .filter_map(|part| match part {
        UserContent::Image(image) => Some(match previous.next() {
          Some(mut prepared) => {
            prepared.sync(image);
            prepared
          }
          None => PreparedImage::new(image),
        }),
        UserContent::Text(_) => None,
      })
      .collect()
  }

  fn text(message: &AgentMessage) -> SharedString {
    match message {
      AgentMessage::User(user) => user.content.as_ref().map(content_text).unwrap_or_default().into(),
      _ => SharedString::default(),
    }
  }

  pub(in crate::session::transcript) fn new(
    id: SharedString,
    message: Rc<AgentMessage>,
    composer: Entity<EditorState>,
    cx: &mut Context<Self>,
  ) -> Self {
    let collapsed = cx.new(|_| false);
    cx.observe(&collapsed, |_, _, cx| cx.notify()).detach();
    let text = Self::text(&message);
    let images = Self::images(&message, Vec::new());
    Self {
      id,
      message,
      text,
      composer,
      collapsed,
      images,
    }
  }

  pub(in crate::session::transcript) fn sync(&mut self, message: Rc<AgentMessage>, cx: &mut Context<Self>) {
    if !Rc::ptr_eq(&self.message, &message) {
      self.text = Self::text(&message);
      self.images = Self::images(&message, std::mem::take(&mut self.images));
      self.message = message;
      cx.notify();
    }
  }
}

impl Render for UserBlock {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let message = self.message.as_ref();
    let mut bubble = Bubble::new().with_variant(BubbleVariant::Filled);
    if *self.collapsed.read(cx) {
      bubble = bubble.child("Message collapsed");
    } else if let AgentMessage::User(_) = message {
      let colors = UserMessageColors::current(cx);
      bubble = bubble
        .content(BubbleContent::new().bg(colors.background()).text_color(colors.foreground()))
        .child(TextView::markdown(
          SharedString::from(format!("{}-text", self.id)),
          self.text.clone(),
        ))
        .children(
          self
            .images
            .iter()
            .enumerate()
            .map(|(index, image)| image.view(SharedString::from(format!("{}-image-{index}", self.id)))),
        );
    }
    let body = Message::new()
      .id(self.id.clone())
      .alignment(MessageAlignment::End)
      .content(MessageBody::new().bubble(bubble))
      .footer(message_footer(
        self.id.clone(),
        message,
        Some(&self.composer),
        Some(&self.collapsed),
        cx,
      ))
      .into_any_element();
    message_context(&self.id, body, message, Some(&self.composer), Some(&self.collapsed), cx)
  }
}
