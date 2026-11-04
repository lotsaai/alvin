//! Retained assistant content and its independently interactive parts.

mod error;
mod fallback;
mod image;
mod reasoning;
mod redacted;
mod server_tool;
mod text;

use std::collections::HashMap;

use alvin_protocol::AssistantContent;
use gpui_kit::component::v_flex;
use gpui_kit::{
  AnyElement, App, AppContext as _, Context, Entity, IntoElement as _, ParentElement as _, SharedString, Styled as _, Subscription, Window,
};

use super::tool::{ToolBlock, ToolInput};
use reasoning::Reasoning;

pub(super) use error::view as error_view;
pub(super) use image::attachment as image_attachment;

pub(super) struct ContentBlocks {
  id: SharedString,
  parts: Vec<Part>,
}

struct Part {
  _subscription: Option<Subscription>,
  identity: Identity,
  body: Body,
}

enum Identity {
  Text,
  Thinking { narration: Option<bool>, item_id: Option<String> },
  Tool { id: Option<String>, name: Option<String> },
  Other,
}

enum Body {
  Text {
    id: SharedString,
    text: String,
    streaming: bool,
  },
  Reasoning(Entity<Reasoning>),
  Redacted(Option<usize>),
  Image {
    mime: Option<String>,
    size: Option<usize>,
    url: Option<String>,
  },
  Fallback {
    from: String,
    to: String,
  },
  ServerTool(String),
  Tool(Entity<ToolBlock>),
}

impl ContentBlocks {
  pub(super) fn new(id: SharedString) -> Self {
    Self { id, parts: Vec::new() }
  }

  pub(super) fn sync<T: 'static>(
    &mut self,
    content: &[AssistantContent],
    tools: &HashMap<usize, ToolInput>,
    streaming: bool,
    _window: &mut Window,
    cx: &mut Context<T>,
  ) {
    self.parts.truncate(content.len());
    for (position, source) in content.iter().enumerate() {
      let input = tools.get(&position);
      if let Some(part) = self.parts.get_mut(position) {
        if part.compatible(source, cx) {
          part.sync(source, input, streaming, cx);
          continue;
        }
        *part = Part::new(&self.id, position, source, input, streaming, cx);
      } else {
        self.parts.push(Part::new(&self.id, position, source, input, streaming, cx));
      }
    }
  }

  pub(super) fn view(&self, cx: &App) -> AnyElement {
    v_flex()
      .w_full()
      .min_w_0()
      .gap_2()
      .children(self.parts.iter().map(|part| part.view(cx)))
      .into_any_element()
  }
}

impl Part {
  fn new<T: 'static>(
    id: &SharedString,
    position: usize,
    source: &AssistantContent,
    input: Option<&ToolInput>,
    streaming: bool,
    cx: &mut Context<T>,
  ) -> Self {
    let mut subscription = None;
    let id = SharedString::from(format!("{id}-part-{position}"));
    let body = match source {
      AssistantContent::Text(value) => Body::Text {
        id,
        text: value.text.clone().unwrap_or_default(),
        streaming,
      },
      AssistantContent::Thinking(value) if value.narration == Some(true) => Body::Text {
        id,
        text: value.thinking.clone().unwrap_or_default(),
        streaming,
      },
      AssistantContent::Thinking(value) => {
        let entity = cx.new(|_| Reasoning::new(id, value.thinking.as_deref().unwrap_or_default()));
        subscription = Some(cx.observe(&entity, |_, _, cx| cx.notify()));
        Body::Reasoning(entity)
      }
      AssistantContent::RedactedThinking(value) => Body::Redacted(value.data.as_ref().map(String::len)),
      AssistantContent::Image(value) => Body::Image {
        mime: value.mime_type.clone(),
        size: value.data.as_ref().map(String::len),
        url: value.url.clone(),
      },
      AssistantContent::Fallback(value) => Body::Fallback {
        from: fallback::model(&value.from).to_owned(),
        to: fallback::model(&value.to).to_owned(),
      },
      AssistantContent::AnthropicServerTool(value) => Body::ServerTool(server_tool::name(value).to_owned()),
      AssistantContent::ToolCall(call) => {
        let fallback;
        let input = match input {
          Some(input) => input,
          None => {
            fallback = call_input(call);
            &fallback
          }
        };
        let entity = cx.new(|cx| ToolBlock::new(id, input.clone(), cx));
        subscription = Some(cx.observe(&entity, |_, _, cx| cx.notify()));
        Body::Tool(entity)
      }
    };
    let identity = match source {
      AssistantContent::Text(_) => Identity::Text,
      AssistantContent::Thinking(value) => Identity::Thinking {
        narration: value.narration,
        item_id: value.item_id.clone(),
      },
      AssistantContent::ToolCall(value) => Identity::Tool {
        id: value.id.clone().filter(|id| !id.is_empty()),
        name: value.name.clone(),
      },
      _ => Identity::Other,
    };
    Self {
      identity,
      body,
      _subscription: subscription,
    }
  }

  fn compatible(&self, next: &AssistantContent, cx: &App) -> bool {
    match (&self.identity, &self.body, next) {
      (Identity::Text, Body::Text { text, .. }, AssistantContent::Text(value)) => grows(text, value.text.as_deref().unwrap_or_default()),
      (Identity::Thinking { narration, item_id }, body, AssistantContent::Thinking(value)) => {
        let old = match body {
          Body::Text { text, .. } => text.as_str(),
          Body::Reasoning(entity) => entity.read(cx).text(),
          _ => return false,
        };
        *narration == value.narration
          && (item_id.is_none() || value.item_id.is_none() || *item_id == value.item_id)
          && grows(old, value.thinking.as_deref().unwrap_or_default())
      }
      (Identity::Tool { id, name }, Body::Tool(_), AssistantContent::ToolCall(value)) => {
        let next_id = value.id.as_deref().filter(|id| !id.is_empty());
        (id.is_none() || next_id.is_none() || id.as_deref() == next_id) && (name.is_none() || value.name.is_none() || *name == value.name)
      }
      (Identity::Other, Body::Redacted(_), AssistantContent::RedactedThinking(_))
      | (Identity::Other, Body::Image { .. }, AssistantContent::Image(_))
      | (Identity::Other, Body::Fallback { .. }, AssistantContent::Fallback(_))
      | (Identity::Other, Body::ServerTool(_), AssistantContent::AnthropicServerTool(_)) => true,
      _ => false,
    }
  }

  fn sync<T: 'static>(&mut self, source: &AssistantContent, input: Option<&ToolInput>, streaming: bool, cx: &mut Context<T>) {
    match (&mut self.body, source) {
      (Body::Text { text, .. }, AssistantContent::Text(value)) => {
        let next = value.text.as_deref().unwrap_or_default();
        if text.as_str() != next {
          text.clear();
          text.push_str(next);
        }
      }
      (Body::Text { text, .. }, AssistantContent::Thinking(value)) => {
        let next = value.thinking.as_deref().unwrap_or_default();
        if text.as_str() != next {
          text.clear();
          text.push_str(next);
        }
      }
      (Body::Reasoning(entity), AssistantContent::Thinking(value)) => entity.update(cx, |reasoning, cx| {
        reasoning.sync(value.thinking.as_deref().unwrap_or_default(), cx)
      }),
      (Body::Redacted(size), AssistantContent::RedactedThinking(value)) => *size = value.data.as_ref().map(String::len),
      (Body::Image { mime, size, url }, AssistantContent::Image(value)) => {
        mime.clone_from(&value.mime_type);
        *size = value.data.as_ref().map(String::len);
        url.clone_from(&value.url);
      }
      (Body::Fallback { from, to }, AssistantContent::Fallback(value)) => {
        let next_from = fallback::model(&value.from);
        let next_to = fallback::model(&value.to);
        if from.as_str() != next_from {
          *from = next_from.to_owned();
        }
        if to.as_str() != next_to {
          *to = next_to.to_owned();
        }
      }
      (Body::ServerTool(name), AssistantContent::AnthropicServerTool(value)) => {
        let next = server_tool::name(value);
        if name.as_str() != next {
          *name = next.to_owned();
        }
      }
      _ => {}
    }
    match (&mut self.identity, source) {
      (Identity::Thinking { item_id, .. }, AssistantContent::Thinking(value)) if value.item_id.is_some() => {
        item_id.clone_from(&value.item_id)
      }
      (Identity::Tool { id, name }, AssistantContent::ToolCall(value)) => {
        if let Some(next) = value.id.as_ref().filter(|next| !next.is_empty())
          && id.as_ref() != Some(next)
        {
          *id = Some(next.clone());
        }
        if value.name.is_some() {
          name.clone_from(&value.name);
        }
      }
      _ => {}
    }
    if let Body::Text { streaming: current, .. } = &mut self.body {
      *current = streaming;
    }
    if let (Body::Tool(entity), AssistantContent::ToolCall(call)) = (&self.body, source) {
      if let Some(input) = input {
        entity.update(cx, |tool, cx| tool.sync(input, cx));
      } else {
        let input = call_input(call);
        entity.update(cx, |tool, cx| tool.sync(&input, cx));
      }
    }
  }

  fn view(&self, cx: &App) -> AnyElement {
    match &self.body {
      Body::Text { id, text, streaming } => text::view(id, text, *streaming),
      Body::Reasoning(entity) => entity.clone().into_any_element(),
      Body::Redacted(size) => redacted::view(*size, cx),
      Body::Image { mime, size, url } => image::attachment_parts(mime.as_deref(), *size, url.as_deref()),
      Body::Fallback { from, to } => fallback::view(from, to, cx),
      Body::ServerTool(name) => server_tool::view(name, cx),
      Body::Tool(entity) => entity.clone().into_any_element(),
    }
  }
}

fn call_input(call: &alvin_protocol::ToolCall) -> ToolInput {
  ToolInput {
    name: call.name.clone().unwrap_or_else(|| "tool".into()),
    intent: call.intent.clone(),
    args: call.arguments.clone().map(serde_json::Value::Object),
    result: None,
    stream: Vec::new(),
    running: false,
    failed: false,
    exit_code: None,
    timestamp: None,
  }
}

fn grows(old: &str, new: &str) -> bool {
  new.starts_with(old)
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use alvin_protocol::{AssistantContent, ThinkingContent, ToolCall};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{AppContext as _, Context, IntoElement, Render, SharedString, TestAppContext, Window, WindowOptions};

  use super::ContentBlocks;

  struct Fixture {
    blocks: ContentBlocks,
  }

  impl Fixture {
    fn sync(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
      let parts = [AssistantContent::Thinking(ThinkingContent {
        thinking: Some(text.to_owned()),
        ..ThinkingContent::default()
      })];
      self.blocks.sync(&parts, &HashMap::new(), true, window, cx);
      cx.notify();
    }

    fn sync_parts(&mut self, parts: &[AssistantContent], streaming: bool, window: &mut Window, cx: &mut Context<Self>) {
      self.blocks.sync(parts, &HashMap::new(), streaming, window, cx);
      cx.notify();
    }
  }

  impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
      self.blocks.view(cx)
    }
  }

  #[gpui_kit::test]
  fn streaming_update_preserves_disclosure_and_replacement_resets_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| {
          let mut fixture = Fixture {
            blocks: ContentBlocks::new(SharedString::from("message")),
          };
          fixture.sync("First thought", window, cx);
          fixture
        })
      })
      .expect("open test window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("message-part-0-toggle", cx);
      assert!(window.find("message-part-0-details").visible());
      fixture.update(cx, |fixture, cx| fixture.sync("First thought grows", window, cx));
      window.render_frame(cx);
      assert!(window.find("message-part-0-details").visible());
      fixture.update(cx, |fixture, cx| fixture.sync("Unrelated thought", window, cx));
      window.render_frame(cx);
      assert!(window.try_find("message-part-0-details").is_none());
    })
    .expect("update test window");
  }

  #[gpui_kit::test]
  fn tool_calls_keep_independent_state_across_late_and_missing_ids(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let call = |id: Option<&str>, command: &str| {
      AssistantContent::ToolCall(ToolCall {
        id: id.map(str::to_owned),
        name: Some("bash".into()),
        arguments: Some(serde_json::json!({"command": command}).as_object().unwrap().clone()),
        ..ToolCall::default()
      })
    };
    let first_command = "first ".repeat(500);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| {
          let mut fixture = Fixture {
            blocks: ContentBlocks::new(SharedString::from("tools")),
          };
          fixture.sync_parts(&[call(None, &first_command), call(Some("second"), "second")], true, window, cx);
          fixture
        })
      })
      .expect("open test window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("tools-part-0-copy", cx);
      assert_eq!(cx.read_from_clipboard().unwrap().text(), Some(format!("$ {first_command}")));
      window.click("tools-part-0-toggle", cx);
      assert!(window.try_find("tools-part-0-details").is_none());
      assert!(window.find("tools-part-1-details").visible());
      fixture.update(cx, |fixture, cx| {
        fixture.sync_parts(
          &[call(Some("first"), "first grows"), call(Some("second"), "second")],
          true,
          window,
          cx,
        )
      });
      window.render_frame(cx);
      assert!(window.try_find("tools-part-0-details").is_none());
      fixture.update(cx, |fixture, cx| {
        fixture.sync_parts(&[call(None, "first committed"), call(Some("second"), "second")], false, window, cx)
      });
      window.render_frame(cx);
      assert!(window.try_find("tools-part-0-details").is_none());
      fixture.update(cx, |fixture, cx| {
        fixture.sync_parts(
          &[call(Some("replacement"), "replacement"), call(Some("second"), "second")],
          false,
          window,
          cx,
        )
      });
      window.render_frame(cx);
      assert!(window.find("tools-part-0-details").visible());
      assert!(window.find("tools-part-1-details").visible());
    })
    .expect("update test window");
  }
}
