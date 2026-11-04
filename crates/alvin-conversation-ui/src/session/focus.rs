use gpui_kit::{App, ElementId, FocusHandle, Window};

pub(super) fn tab_stop(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> FocusHandle {
  window
    .use_keyed_state(id.into(), cx, |_, cx| cx.focus_handle().tab_stop(true))
    .read(cx)
    .clone()
}
