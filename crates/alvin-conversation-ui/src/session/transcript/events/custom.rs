use super::super::{content_text, note};
use alvin_protocol::CustomMessage;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, message: &CustomMessage) -> AnyElement {
  note(
    Message::new().id(id.clone()),
    id,
    message.custom_type.as_deref().unwrap_or("custom"),
    &message.content.as_ref().map(content_text).unwrap_or_default(),
  )
}
