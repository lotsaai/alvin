use super::super::{content_text, note};
use alvin_protocol::HookMessage;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, message: &HookMessage) -> AnyElement {
  note(
    Message::new().id(id.clone()),
    id,
    message.custom_type.as_deref().unwrap_or("hook"),
    &message.content.as_ref().map(content_text).unwrap_or_default(),
  )
}
