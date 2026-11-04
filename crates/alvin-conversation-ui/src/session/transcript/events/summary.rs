use super::super::boundary;
use gpui_kit::component::message::Message;
use gpui_kit::{AnyElement, SharedString};

pub(super) fn view(id: &SharedString, title: &str, summary: &str) -> AnyElement {
  boundary(Message::new().id(id.clone()), id, title, summary)
}
