//! Read-only message actions and historical response metadata.
//!
//! Footer controls use a named child identity instead of repeating the owning message row's id.

use alvin_protocol::{AgentMessage, AssistantContent, AssistantMessage, MessageContent, UserContent};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::message::MessageFooter;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::{
  AnyElement, App, ClipboardItem, ElementId, Entity, InteractiveElement as _, IntoElement as _, ParentElement as _, SharedString,
  Styled as _, div,
};

/// Copy the visible message text without changing session history or the composer draft.
pub(super) fn message_footer(
  id: impl Into<ElementId>,
  message: &AgentMessage,
  composer: Option<&Entity<EditorState>>,
  collapsed: Option<&Entity<bool>>,
  cx: &App,
) -> MessageFooter {
  let text = SharedString::from(copy_text(message));
  let copy = text.clone();
  let composer = composer.filter(|_| !text.is_empty()).cloned();
  let collapsed = collapsed.map(|state| (state.clone(), *state.read(cx)));
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
          .on_click(move |_, _, cx| {
            cx.stop_propagation();
            cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
          }),
      )
      .child({
        let text = text.clone();
        let composer = composer.clone();
        let collapsed = collapsed.clone();
        Button::new("message-menu")
          .ghost()
          .small()
          .icon(IconName::Ellipsis)
          .accessibility_label("Message menu")
          .dropdown_menu(move |menu, _, cx| {
            let composer = composer.clone().filter(|composer| composer.read(cx).value().is_empty());
            message_menu(menu, text.clone(), composer, collapsed.clone())
          })
      })
      .child(SharedString::from(metadata))
      .context_menu(move |menu, _, cx| {
        let composer = composer.clone().filter(|composer| composer.read(cx).value().is_empty());
        message_menu(menu, text.clone(), composer, collapsed.clone())
      }),
  )
}

pub(super) fn message_context(
  id: &SharedString,
  body: AnyElement,
  message: &AgentMessage,
  composer: Option<&Entity<EditorState>>,
  collapsed: Option<&Entity<bool>>,
  cx: &App,
) -> AnyElement {
  let text = SharedString::from(copy_text(message));
  let composer = composer.filter(|_| !text.is_empty()).cloned();
  let collapsed = collapsed.map(|state| (state.clone(), *state.read(cx)));
  div()
    .id((ElementId::from(id.clone()), "context"))
    .child(body)
    .context_menu(move |menu, _, cx| {
      let composer = composer.clone().filter(|composer| composer.read(cx).value().is_empty());
      message_menu(menu, text.clone(), composer, collapsed.clone())
    })
    .into_any_element()
}

fn message_menu(
  mut menu: PopupMenu,
  text: SharedString,
  composer: Option<Entity<EditorState>>,
  collapsed: Option<(Entity<bool>, bool)>,
) -> PopupMenu {
  let copy = text.clone();
  menu = menu.item(PopupMenuItem::new("Copy message").on_click(move |_, _, cx| {
    cx.write_to_clipboard(ClipboardItem::new_string(copy.to_string()));
  }));
  if let Some(composer) = composer {
    menu = menu.item(PopupMenuItem::new("Restore to composer").on_click(move |_, window, cx| {
      composer.update(cx, |composer, cx| {
        if composer.value().is_empty() {
          composer.replace_all(text.clone(), window, cx);
          composer.focus(window, cx);
        }
      });
    }));
  }
  if let Some((state, is_collapsed)) = collapsed {
    menu = menu.item(
      PopupMenuItem::new(if is_collapsed { "Expand message" } else { "Collapse message" }).on_click(move |_, _, cx| {
        state.update(cx, |collapsed, cx| {
          *collapsed = !*collapsed;
          cx.notify();
        });
      }),
    );
  }
  menu
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
    AgentMessage::Assistant(message) => message
      .content
      .iter()
      .flatten()
      .filter_map(|part| match part {
        AssistantContent::Text(text) => text.text.as_deref(),
        AssistantContent::Thinking(thinking) if thinking.narration == Some(true) => thinking.thinking.as_deref(),
        _ => None,
      })
      .collect::<Vec<_>>()
      .join("\n"),
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
    for (value, label) in [
      (usage.input, "input"),
      (usage.output, "output"),
      (usage.cache_read, "cache read"),
      (usage.cache_write, "cache write"),
    ] {
      if let Some(value) = value.filter(|value| *value >= 0) {
        fields.push(format!("{value} {label} tokens"));
      }
    }
    if let Some(total) = usage.total_tokens.filter(|total| *total >= 0) {
      fields.push(format!("{total} tokens"));
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
  if let Some(ttft) = message.ttft.filter(|value| value.is_finite() && *value >= 0.) {
    fields.push(format!("{ttft:.0} ms to first token"));
  }
  fields.join(" · ")
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{AgentMessage, AssistantMessage, MessageContent, Usage, UserMessage};
  use gpui_kit::component::h_flex;
  use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
  use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
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
      input: Some(4_883),
      output: Some(29),
      cache_read: Some(704),
      cache_write: Some(0),
      total_tokens: Some(5_616),
      ..Usage::default()
    });
    message.ttft = Some(1_000.);
    assert_eq!(
      metadata(&message),
      "provider/historical-model · 4883 input tokens · 29 output tokens · 704 cache read tokens · 0 cache write tokens · 5616 tokens · 1.2 s · 1000 ms to first token"
    );
    message.ttft = None;
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
    assert!(metadata(&message).is_empty());
    for (duration, expected) in [(0., "0 ms"), (999., "999 ms"), (1_000., "1.0 s"), (60_000., "60.0 s")] {
      message.duration = Some(duration);
      assert_eq!(metadata(&message), expected);
    }
    for duration in [-1., f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
      message.duration = Some(duration);
      assert!(metadata(&message).is_empty());
    }
  }

  #[test]
  fn assistant_copy_preserves_all_visible_markdown() {
    let message: AgentMessage = serde_json::from_value(serde_json::json!({
      "role": "assistant",
      "content": [
        {"type": "thinking", "thinking": "private reasoning"},
        {"type": "thinking", "thinking": "Visible narration", "narration": true},
        {"type": "text", "text": "Earlier process text"},
        {"type": "toolCall", "id": "call", "name": "read", "arguments": {}},
        {"type": "text", "text": "## Result\n\n```rust\nfn main() {}\n```\n\nFinished. 中文"}
      ]
    }))
    .expect("assistant message");
    assert_eq!(
      copy_text(&message),
      "Visible narration\nEarlier process text\n## Result\n\n```rust\nfn main() {}\n```\n\nFinished. 中文"
    );
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
        .child(message_footer("footer", &self.message, None, None, cx))
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
  struct ActionFixture {
    message: AgentMessage,
    composer: Entity<EditorState>,
    collapsed: Entity<bool>,
    changes: usize,
    _subscription: gpui_kit::Subscription,
  }

  impl Render for ActionFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
      h_flex().p_4().child(Editor::new(&self.composer).w_48()).child(message_footer(
        "actions",
        &self.message,
        Some(&self.composer),
        Some(&self.collapsed),
        cx,
      ))
    }
  }

  #[gpui_kit::test]
  fn menu_restores_text_and_toggles_only_local_collapse(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| {
          let composer = cx.new(|cx| EditorState::new(window, cx));
          let subscription = cx.subscribe(&composer, |this: &mut ActionFixture, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
              this.changes += 1;
              cx.notify();
            }
          });
          ActionFixture {
            message: AgentMessage::User(UserMessage {
              content: Some(MessageContent::String("First\nSecond".to_owned())),
              ..UserMessage::default()
            }),
            composer,
            collapsed: cx.new(|_| false),
            changes: 0,
            _subscription: subscription,
          }
        })
      })
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("message-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      let mut menu = window.within("popup-menu");
      menu.press("down", cx);
      menu.press("down", cx);
      menu.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
      assert_eq!(fixture.read(cx).composer.read(cx).value().as_ref(), "First\nSecond");
      assert_eq!(fixture.read(cx).changes, 1);
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("message-menu", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      assert!(window.try_find_by_label("Restore to composer").is_none());
      let mut menu = window.within("popup-menu");
      menu.press("down", cx);
      menu.press("down", cx);
      menu.press("enter", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update(|cx| {
      let fixture = fixture.read(cx);
      assert!(*fixture.collapsed.read(cx));
      assert_eq!(fixture.composer.read(cx).value().as_ref(), "First\nSecond");
    });
  }
}
