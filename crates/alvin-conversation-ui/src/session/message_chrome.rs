//! Read-only message actions and historical response metadata.
//!
//! Footer controls use a named child identity instead of repeating the owning message row's id.

use alvin_protocol::{AgentMessage, AssistantContent, AssistantMessage, MessageContent, UserContent};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::message::MessageFooter;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  Anchor, AnyElement, App, ClipboardItem, ElementId, Entity, InteractiveElement as _, IntoElement as _, ParentElement as _, ScrollHandle,
  SharedString, StatefulInteractiveElement as _, Styled as _, div,
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
  let (summary, details) = match message {
    AgentMessage::Assistant(message) => (metadata(message, false), metadata(message, true)),
    _ => (String::new(), String::new()),
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
      .when(!details.is_empty(), |footer| {
        let details = SharedString::from(details);
        footer.child(
          Popover::new("message-metadata")
            .anchor(Anchor::BottomLeft)
            .trigger(
              Button::new("message-metadata-trigger")
                .ghost()
                .small()
                .icon(IconName::Info)
                .accessibility_label("Show message details"),
            )
            .content(move |_, window, cx| {
              let scroll = window
                .use_keyed_state("metadata-scroll", cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone();
              let focus = window
                .use_keyed_state("metadata-focus", cx, |_, cx| cx.focus_handle().tab_stop(true))
                .read(cx)
                .clone();
              div()
                .id("message-metadata-details")
                .test_support()
                .track_focus(&focus)
                .track_scroll(&scroll)
                .w((window.rem_size() * 16.).min((window.viewport_size().width - window.rem_size() * 3.).max(gpui_kit::px(0.))))
                .max_h((window.rem_size() * 16.).min((window.viewport_size().height - window.rem_size() * 3.).max(gpui_kit::px(0.))))
                .overflow_y_scroll()
                .overflow_x_scroll()
                .text_sm()
                .text_color(cx.theme().popover_foreground)
                .on_key_down(move |event, window, cx| {
                  let mut offset = scroll.offset();
                  let step = window.rem_size() * 3.;
                  match event.keystroke.key.as_str() {
                    "down" => offset.y = (offset.y - step).max(-scroll.max_offset().y),
                    "up" => offset.y = (offset.y + step).min(gpui_kit::px(0.)),
                    "right" => offset.x = (offset.x - step).max(-scroll.max_offset().x),
                    "left" => offset.x = (offset.x + step).min(gpui_kit::px(0.)),
                    "home" => offset.y = gpui_kit::px(0.),
                    "end" => offset.y = -scroll.max_offset().y,
                    _ => return,
                  }
                  scroll.set_offset(offset);
                  window.refresh();
                  cx.stop_propagation();
                })
                .child(details.clone())
            }),
        )
      })
      .child(div().flex_1().min_w_0().text_xs().truncate().child(SharedString::from(summary)))
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

fn metadata(message: &AssistantMessage, detailed: bool) -> String {
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
    if detailed {
      for (value, label) in [
        (usage.input, "input"),
        (usage.output, "output"),
        (usage.cache_read, "cache read"),
        (usage.cache_write, "cache write"),
        (usage.context_tokens, "context"),
        (usage.reasoning_tokens, "reasoning"),
      ] {
        if let Some(value) = value.filter(|value| *value >= 0) {
          fields.push(format!("{value} {label} tokens"));
        }
      }
      if let Some(cost) = &usage.cost {
        for (value, label) in [
          (cost.input, "input cost"),
          (cost.output, "output cost"),
          (cost.cache_read, "cache read cost"),
          (cost.cache_write, "cache write cost"),
          (cost.total, "total cost"),
        ] {
          if let Some(value) = value.filter(|value| value.is_finite() && *value >= 0.) {
            fields.push(format!("{label}: ${value}"));
          }
        }
        if !cost.extra.is_empty() {
          fields.push(format!("other cost: {}", serde_json::Value::Object(cost.extra.clone())));
        }
      }
      if let Some(value) = usage.premium_requests.filter(|value| value.is_finite() && *value >= 0.) {
        fields.push(format!("{value} premium requests"));
      }
      for (value, label) in [
        (&usage.orchestration, "orchestration"),
        (&usage.cttl, "cttl"),
        (&usage.server, "server"),
        (&usage.credits, "credits"),
      ] {
        if let Some(value) = value {
          fields.push(format!("{label}: {}", serde_json::Value::Object(value.clone())));
        }
      }
      if !usage.extra.is_empty() {
        fields.push(format!("other usage: {}", serde_json::Value::Object(usage.extra.clone())));
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
  if detailed && let Some(ttft) = message.ttft.filter(|value| value.is_finite() && *value >= 0.) {
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
    assert!(metadata(&message, true).is_empty());
    message.usage = Some(Usage {
      input: Some(-1),
      output: Some(0),
      cache_read: Some(4),
      ..Usage::default()
    });
    let details = metadata(&message, true);
    assert!(!details.contains("input"));
    assert!(details.contains("0 output"));
    assert!(details.contains("4 cache read"));
    assert!(!metadata(&message, false).contains("output"));
    message.ttft = Some(-1.);
    message.usage = Some(Usage {
      input: Some(-1),
      ..Usage::default()
    });
    assert!(metadata(&message, true).is_empty());
  }

  #[test]
  fn request_duration_is_milliseconds_and_never_inferred_from_timestamps() {
    let mut message = AssistantMessage {
      timestamp: Some(1_700_000_000_000.),
      completed_at: Some(1_700_000_099_000.),
      ..AssistantMessage::default()
    };
    assert!(metadata(&message, true).is_empty());
    for (duration, expected) in [(0., "0 ms"), (999., "999 ms"), (1_000., "1.0 s"), (60_000., "60.0 s")] {
      message.duration = Some(duration);
      assert_eq!(metadata(&message, true), expected);
    }
    for duration in [-1., f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
      message.duration = Some(duration);
      assert!(metadata(&message, true).is_empty());
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

  #[gpui_kit::test]
  fn details_open_and_restore_keyboard_focus(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| FooterFixture {
          input: cx.new(|cx| InputState::new(window, cx)),
          message: AgentMessage::Assistant(Box::new(AssistantMessage {
            model: Some("model".to_owned()),
            usage: Some(Usage {
              input: Some(0),
              context_tokens: Some(123),
              ..Usage::default()
            }),
            ..AssistantMessage::default()
          })),
        })
      })
      .expect("open footer window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("composer", cx);
      window.press("tab", cx);
      window.press("tab", cx);
      window.press("tab", cx);
      assert_eq!(window.find("message-metadata-trigger").focused(), Some(true));
      window.press("enter", cx);
    })
    .expect("open message details");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("message-metadata-trigger").focused(), Some(false));
      window.press("tab", cx);
      window.press("down", cx);
      window.press("escape", cx);
    })
    .expect("dismiss message details");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("message-metadata-trigger").focused(), Some(true));
    })
    .expect("restore details trigger focus");
  }

  #[gpui_kit::test]
  fn details_open_with_pointer_and_close_with_keyboard(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| FooterFixture {
          input: cx.new(|cx| InputState::new(window, cx)),
          message: AgentMessage::Assistant(Box::new(AssistantMessage {
            usage: Some(Usage {
              context_tokens: Some(123),
              ..Usage::default()
            }),
            ..AssistantMessage::default()
          })),
        })
      })
      .expect("open footer window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("message-metadata-details").is_none());
      window.click("message-metadata-trigger", cx);
    })
    .expect("click message details");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.find("message-metadata-details").visible());
      window.press("tab", cx);
      assert_eq!(window.find("message-metadata-details").focused(), Some(true));
      window.press("escape", cx);
    })
    .expect("navigate and dismiss message details");
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("message-metadata-details").is_none());
    })
    .expect("confirm message details dismissed");
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
