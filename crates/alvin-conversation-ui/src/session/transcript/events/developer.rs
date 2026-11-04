use super::super::{content_text, note};
use alvin_protocol::DeveloperMessage;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, message: &DeveloperMessage) -> AnyElement {
  note(
    Message::new().id(id.clone()),
    id,
    "Developer",
    &message.content.as_ref().map(content_text).unwrap_or_default(),
  )
}
