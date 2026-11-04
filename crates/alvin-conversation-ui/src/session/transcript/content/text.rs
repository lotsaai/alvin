use gpui_kit::component::text::TextView;
use gpui_kit::{AnyElement, IntoElement as _, SharedString};

pub(super) fn view(id: &SharedString, text: &str, streaming: bool) -> AnyElement {
  TextView::markdown(id.clone(), text.to_owned())
    .stream_fade(streaming)
    .into_any_element()
}
