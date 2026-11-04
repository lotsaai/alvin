use super::super::note;
use alvin_protocol::FileMentionMessage;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, message: &FileMentionMessage) -> AnyElement {
  let paths = message
    .files
    .iter()
    .flatten()
    .filter_map(|file| file.path.as_deref())
    .collect::<Vec<_>>()
    .join("\n");
  note(Message::new().id(id.clone()), id, "Files", &paths)
}
