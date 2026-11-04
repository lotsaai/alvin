use super::super::note;
use alvin_protocol::PythonExecutionMessage;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, message: &PythonExecutionMessage) -> AnyElement {
  note(
    Message::new().id(id.clone()),
    id,
    &format!("python {}", message.code.as_deref().unwrap_or_default()),
    message.output.as_deref().unwrap_or_default(),
  )
}
