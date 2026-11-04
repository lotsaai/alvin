//! Read-only message actions and historical response metadata.
//!
//! Footer controls use a named child identity instead of repeating the owning message row's id.

use alvin_protocol::{AgentMessage, AssistantContent, AssistantMessage, MessageContent, UserContent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::message::MessageFooter;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::{App, ClipboardItem, ElementId, InteractiveElement as _, ParentElement as _, SharedString, Styled as _};

/// Copy the visible message text without changing session history or the composer draft.
pub(super) fn message_footer(id: impl Into<ElementId>, message: &AgentMessage, cx: &App) -> MessageFooter {
  let text = SharedString::from(copy_text(message));
  let copy = text.clone();
  let metadata = match message {
    AgentMessage::Assistant(message) => metadata(message),
    _ => String::new(),
  };
  MessageFooter::new().content_inset(false).child(
    h_flex()
      .id((id.into(), "footer"))
      .min_w_0()
      .gap_2()
      .flex_wrap()
      .text_color(cx.theme().muted_foreground)
      .child(
        Button::new("copy")
          .ghost()
          .small()
          .icon(IconName::Copy)
          .accessibility_label("Copy message")
          .tooltip("Copy message")
          .opacity(0.)
          .group_hover("message-actions", |style| style.opacity(1.))
          .focus(|style| style.opacity(1.))
          .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
          }),
      )
      .child(SharedString::from(metadata))
      .context_menu(move |menu, _, _| {
        let text = text.clone();
        menu.item(PopupMenuItem::new("Copy message").on_click(move |_, _, cx| {
          cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        }))
      }),
  )
}

fn copy_text(message: &AgentMessage) -> String {
  match message {
    AgentMessage::User(message) => match &message.content {
      Some(MessageContent::String(text)) => text.clone(),
      Some(MessageContent::UserContentList(parts)) => {
        let mut text = String::new();
        for (index, part) in parts
          .iter()
          .filter_map(|part| match part {
            UserContent::Text(text) => text.text.as_deref(),
            UserContent::Image(_) => None,
          })
          .enumerate()
        {
          if index > 0 {
            text.push('\n');
          }
          text.push_str(part);
        }
        text
      }
      None => String::new(),
    },
    AgentMessage::Assistant(message) => match message.content.as_ref().and_then(|parts| parts.last()) {
      Some(AssistantContent::Text(text)) => text.text.clone().unwrap_or_default(),
      _ => String::new(),
    },
    _ => String::new(),
  }
}

fn metadata(message: &AssistantMessage) -> String {
  let mut fields = Vec::new();
  if let Some(model) = message.model.as_deref().filter(|model| !model.is_empty()) {
    fields.push(match message.provider.as_deref().filter(|provider| !provider.is_empty()) {
      Some(provider) => format!("{provider}/{model}"),
      None => model.to_owned(),
    });
  } else if let Some(provider) = message.provider.as_deref().filter(|provider| !provider.is_empty()) {
    fields.push(provider.to_owned());
  }
  if let Some(usage) = &message.usage {
    if let Some(total) = usage.total_tokens.filter(|total| *total >= 0) {
      fields.push(format!("{total} tokens"));
    } else {
      if let Some(input) = usage.input.filter(|input| *input >= 0) {
        fields.push(format!("{input} input tokens"));
      }
      if let Some(output) = usage.output.filter(|output| *output >= 0) {
        fields.push(format!("{output} output tokens"));
      }
    }
  }
  // omp's duration is request latency in milliseconds, not wall-clock timestamps or whole-turn elapsed time.
  if let Some(duration) = message.duration.filter(|duration| duration.is_finite() && *duration >= 0.) {
    fields.push(if duration < 1_000. {
      format!("{duration:.0} ms")
    } else {
      format!("{:.1} s", duration / 1_000.)
    });
  }
  fields.join(" · ")
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{AgentMessage, AssistantMessage, MessageContent, Usage, UserMessage};
  use gpui_kit::component::h_flex;
  use gpui_kit::component::input::{Input, InputState};
  use gpui_kit::test::{TestEventExt as _, TestWindowExt as _};
  use gpui_kit::{
    AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, WindowOptions,
  };

  use super::{copy_text, message_footer, metadata};

  #[test]
  fn historical_metadata_uses_only_reported_values() {
    let mut message = AssistantMessage::default();
    assert_eq!(metadata(&message), "");
    message.model = Some("historical-model".into());
    message.provider = Some("provider".into());
    message.usage = Some(Usage {
      total_tokens: Some(124),
      ..Usage::default()
    });
    message.duration = Some(1_250.);
    assert_eq!(metadata(&message), "provider/historical-model · 124 tokens · 1.2 s");
    message.usage = Some(Usage {
      input: Some(12),
      output: Some(0),
      ..Usage::default()
    });
    assert_eq!(
      metadata(&message),
      "provider/historical-model · 12 input tokens · 0 output tokens · 1.2 s"
    );
    message.usage = Some(Usage {
      total_tokens: Some(-1),
      input: Some(-2),
      ..Usage::default()
    });
    message.duration = None;
    assert_eq!(metadata(&message), "provider/historical-model");
  }

  #[test]
  fn request_duration_is_milliseconds_and_never_inferred_from_timestamps() {
    let mut message = AssistantMessage {
      timestamp: Some(1_700_000_000_000.),
      completed_at: Some(1_700_000_099_000.),
      ..AssistantMessage::default()
    };
    assert_eq!(metadata(&message), "");
    for (duration, expected) in [(0., "0 ms"), (999., "999 ms"), (1_000., "1.0 s"), (60_000., "60.0 s")] {
      message.duration = Some(duration);
      assert_eq!(metadata(&message), expected);
    }
    for duration in [-1., f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
      message.duration = Some(duration);
      assert_eq!(metadata(&message), "");
    }
  }

  #[test]
  fn assistant_copy_preserves_final_markdown_without_process_text() {
    let message: AgentMessage = serde_json::from_value(serde_json::json!({
      "role": "assistant",
      "content": [
        {"type": "thinking", "thinking": "private reasoning"},
        {"type": "text", "text": "Earlier process text"},
        {"type": "toolCall", "id": "call", "name": "read", "arguments": {}},
        {"type": "text", "text": "## Result\n\n```rust\nfn main() {}\n```\n\nFinished. 中文"}
      ]
    }))
    .expect("assistant message");
    assert_eq!(copy_text(&message), "## Result\n\n```rust\nfn main() {}\n```\n\nFinished. 中文");
  }

  struct FooterFixture {
    message: AgentMessage,
    input: Entity<InputState>,
  }

  impl Render for FooterFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
      h_flex()
        .group("message-actions")
        .p_4()
        .gap_4()
        .child(Input::new(&self.input).id("composer").w_48())
        .child(message_footer("footer", &self.message, cx))
    }
  }

  #[gpui_kit::test]
  fn copy_shortcut_writes_exact_text_and_remains_keyboard_reachable(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let text = "Draft **markdown**\n\n中文";
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| FooterFixture {
          input: cx.new(|cx| InputState::new(window, cx)),
          message: AgentMessage::User(UserMessage {
            content: Some(MessageContent::String(text.to_owned())),
            ..UserMessage::default()
          }),
        })
      })
      .expect("open footer window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("composer", cx);
    })
    .expect("focus composer");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("composer").focused(), Some(true));
      assert_eq!(window.find("copy").focused(), Some(false));
      window.press("tab", cx);
      assert_eq!(window.find("copy").focused(), Some(true));
      assert_eq!(window.find("composer").focused(), Some(false));
      assert!(window.find("copy").visible());
      window.press("enter", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text.to_owned()));
      assert_eq!(window.find("copy").focused(), Some(true));
      cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("replaced".to_owned()));
      window.hover("copy", cx);
      assert!(window.find("copy").visible());
      window.click("copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text.to_owned()));
      assert_eq!(window.find("copy").focused(), Some(true));
      assert_eq!(window.find("composer").focused(), Some(false));
      cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("replaced again".to_owned()));
      window.right_click("copy", cx);
    })
    .expect("interact with footer");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("popup-menu").focused(), Some(true));
      assert_eq!(window.find("copy").focused(), Some(false));
      assert_eq!(window.find("composer").focused(), Some(false));
      let mut menu = window.within("popup-menu");
      menu.press("down", cx);
      assert_eq!(menu.find(0usize).selected(), Some(true));
      window.key_down("enter", false, cx);
    })
    .expect("activate native copy menu");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.key_up("enter", cx);
      assert!(window.try_find("popup-menu").is_none());
      assert_eq!(window.find("copy").focused(), Some(true));
      assert_eq!(window.find("composer").focused(), Some(false));
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text.to_owned()));
    })
    .expect("restore footer focus after menu copy");
  }
}
