use alvin_protocol::{AgentMessage, AssistantMessage, CustomMessage, HookMessage, MessageContent, StopReason, UserContent};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, text::TextView, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  ClipboardItem, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
  StatefulInteractiveElement as _, Styled as _, Window,
};
use serde_json::Value;

use super::super::content::image::PreparedImage;
use super::super::{content_text, fenced};
use super::job;

pub(in crate::session) struct SystemActivityData {
  pub(in crate::session) title: String,
  pub(in crate::session) summary: String,
  pub(in crate::session) body: String,
  pub(in crate::session) copy: String,
  pub(in crate::session) raw: Option<String>,
  pub(in crate::session) level: Option<alvin_conversation::Level>,
}

impl SystemActivityData {
  pub(in crate::session::transcript) fn assistant(message: &AssistantMessage) -> Option<Self> {
    let interrupted = message.stop_reason == Some(StopReason::Aborted);
    if !interrupted && message.stop_reason != Some(StopReason::Error) && message.error_message.is_none() && message.error_status.is_none() {
      return None;
    }
    let mut body = message.error_message.clone().unwrap_or_default();
    if let Some(status) = message.error_status {
      if !body.is_empty() {
        body.push_str("\n\n");
      }
      body.push_str(&format!("Status: {status}"));
    }
    Some(Self {
      title: if interrupted { "Interrupted" } else { "Error" }.to_owned(),
      summary: message
        .error_message
        .as_deref()
        .and_then(|text| text.lines().next())
        .unwrap_or_default()
        .to_owned(),
      copy: body.clone(),
      body,
      raw: Some(raw(serde_json::to_string_pretty(message))),
      level: (!interrupted).then_some(alvin_conversation::Level::Error),
    })
  }
}

struct Presentation {
  title: SharedString,
  summary: SharedString,
  body: SharedString,
  copy: SharedString,
  raw: SharedString,
  raw_markdown: SharedString,
  has_raw: bool,
  level: Option<alvin_conversation::Level>,
  visible: bool,
  jobs: Option<job::Jobs>,
}

impl Presentation {
  fn custom(message: &CustomMessage) -> Self {
    let kind = message.custom_type.as_deref().unwrap_or("custom");
    let content = message.content.as_ref().map(content_text).unwrap_or_default();
    let system_title = format!("Custom · {kind}");
    let (title, body) = if kind == "irc:incoming" {
      let from = message
        .details
        .as_ref()
        .and_then(|details| details.get("from"))
        .and_then(Value::as_str);
      let body = message
        .details
        .as_ref()
        .and_then(|details| details.get("message"))
        .and_then(Value::as_str);
      (
        from.map_or_else(|| "Internal message".to_owned(), |from| format!("Internal message · {from}")),
        body.unwrap_or(&content).to_owned(),
      )
    } else {
      (system_title, content.clone())
    };
    let mut presentation = Self::activity(SystemActivityData {
      title,
      summary: if kind == "irc:incoming" {
        "Incoming message".to_owned()
      } else {
        "Custom message".to_owned()
      },
      body,
      copy: content,
      raw: Some(raw(serde_json::to_string_pretty(message))),
      level: None,
    });
    presentation.visible = message.display != Some(false);
    presentation.jobs = job::parse(message);
    presentation
  }

  fn hook(message: &HookMessage) -> Self {
    let content = message.content.as_ref().map(content_text).unwrap_or_default();
    let mut presentation = Self::activity(SystemActivityData {
      title: format!("Hook · {}", message.custom_type.as_deref().unwrap_or("hook")),
      summary: "Hook message".to_owned(),
      body: content.clone(),
      copy: content,
      raw: Some(raw(serde_json::to_string_pretty(message))),
      level: None,
    });
    presentation.visible = message.display != Some(false);
    presentation
  }

  fn synthetic(message: &AgentMessage) -> Self {
    let (title, body) = match message {
      AgentMessage::User(message) => (
        "Agent directive · synthetic user",
        message.content.as_ref().map(content_text).unwrap_or_default(),
      ),
      AgentMessage::Developer(message) => ("Developer", message.content.as_ref().map(content_text).unwrap_or_default()),
      AgentMessage::BranchSummary(message) => ("Branch summary", message.summary.clone().unwrap_or_default()),
      AgentMessage::CompactionSummary(message) => ("Compaction summary", message.summary.clone().unwrap_or_default()),
      AgentMessage::FileMention(message) => (
        "Files",
        message
          .files
          .iter()
          .flatten()
          .filter_map(|file| file.path.as_deref())
          .collect::<Vec<_>>()
          .join("\n"),
      ),
      _ => ("System event", String::new()),
    };
    let content = body;
    Self::activity(SystemActivityData {
      title: title.to_owned(),
      summary: "Context available in details".to_owned(),
      body: content.clone(),
      copy: content,
      raw: Some(raw(serde_json::to_string_pretty(message))),
      level: None,
    })
  }

  fn activity(data: SystemActivityData) -> Self {
    let has_raw = data.raw.is_some();
    let raw = data.raw.unwrap_or_default();
    let raw_markdown = if has_raw { fenced(&raw).into() } else { SharedString::default() };
    Self {
      title: data.title.into(),
      summary: data.summary.into(),
      body: data.body.into(),
      copy: data.copy.into(),
      raw: raw.into(),
      raw_markdown,
      has_raw,
      level: data.level,
      visible: true,
      jobs: None,
    }
  }
}

fn raw(value: Result<String, serde_json::Error>) -> String {
  value.unwrap_or_else(|error| format!("Could not serialize event: {error}"))
}

pub(in crate::session) struct SystemActivity {
  id: SharedString,
  presentation: Presentation,
  images: Vec<PreparedImage>,
  details_open: bool,
}

impl SystemActivity {
  fn images(content: Option<&MessageContent>, previous: Vec<PreparedImage>) -> Vec<PreparedImage> {
    let mut previous = previous.into_iter();
    content
      .into_iter()
      .flat_map(|content| match content {
        MessageContent::UserContentList(parts) => parts.as_slice(),
        MessageContent::String(_) => &[],
      })
      .filter_map(|part| match part {
        UserContent::Image(image) => Some(match previous.next() {
          Some(mut prepared) => {
            prepared.sync(image);
            prepared
          }
          None => PreparedImage::new(image),
        }),
        UserContent::Text(_) => None,
      })
      .collect()
  }

  fn message_content(message: &AgentMessage) -> Option<&MessageContent> {
    match message {
      AgentMessage::User(message) => message.content.as_ref(),
      AgentMessage::Developer(message) => message.content.as_ref(),
      _ => None,
    }
  }

  pub(in crate::session) fn new_activity(id: SharedString, data: SystemActivityData, _: &mut Context<Self>) -> Self {
    Self {
      id,
      presentation: Presentation::activity(data),
      images: Vec::new(),
      details_open: false,
    }
  }

  pub(in crate::session::transcript) fn sync_activity(&mut self, data: SystemActivityData, cx: &mut Context<Self>) {
    self.presentation = Presentation::activity(data);
    cx.notify();
  }

  pub(in crate::session::transcript) fn new(id: SharedString, message: &CustomMessage, _: &mut Context<Self>) -> Self {
    Self {
      id,
      presentation: Presentation::custom(message),
      images: Self::images(message.content.as_ref(), Vec::new()),
      details_open: false,
    }
  }

  pub(in crate::session::transcript) fn new_hook(id: SharedString, message: &HookMessage, _: &mut Context<Self>) -> Self {
    Self {
      id,
      presentation: Presentation::hook(message),
      images: Self::images(message.content.as_ref(), Vec::new()),
      details_open: false,
    }
  }

  pub(in crate::session::transcript) fn new_synthetic(id: SharedString, message: &AgentMessage, _: &mut Context<Self>) -> Self {
    Self {
      id,
      presentation: Presentation::synthetic(message),
      images: Self::images(Self::message_content(message), Vec::new()),
      details_open: false,
    }
  }

  pub(in crate::session::transcript) fn sync(&mut self, message: &CustomMessage, cx: &mut Context<Self>) {
    self.presentation = Presentation::custom(message);
    self.images = Self::images(message.content.as_ref(), std::mem::take(&mut self.images));
    cx.notify();
  }

  pub(in crate::session::transcript) fn sync_hook(&mut self, message: &HookMessage, cx: &mut Context<Self>) {
    self.presentation = Presentation::hook(message);
    self.images = Self::images(message.content.as_ref(), std::mem::take(&mut self.images));
    cx.notify();
  }

  pub(in crate::session::transcript) fn sync_synthetic(&mut self, message: &AgentMessage, cx: &mut Context<Self>) {
    self.presentation = Presentation::synthetic(message);
    self.images = Self::images(Self::message_content(message), std::mem::take(&mut self.images));
    cx.notify();
  }
}

impl Render for SystemActivity {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if !self.presentation.visible {
      return gpui_kit::div().into_any_element();
    }
    let details_focus = crate::session::focus::tab_stop((ElementId::View(cx.entity_id()), "details-focus"), window, cx);
    let body = self.presentation.copy.clone();
    let raw = self.presentation.raw.clone();
    let copy_focus = crate::session::focus::tab_stop((ElementId::View(cx.entity_id()), "copy-focus"), window, cx);
    let raw_focus = crate::session::focus::tab_stop((ElementId::View(cx.entity_id()), "raw-focus"), window, cx);
    let mut view = v_flex()
      .id(SharedString::from(format!("{}-group", self.id)))
      .role(gpui_kit::Role::Group)
      .aria_label(self.presentation.title.clone())
      .test_support()
      .w_full()
      .min_w_0()
      .gap_1()
      .child(
        h_flex()
          .w_full()
          .min_w_0()
          .gap_2()
          .child(
            gpui_kit::div()
              .flex_1()
              .min_w_0()
              .truncate()
              .when_some(self.presentation.level, |this, level| {
                this.text_color(match level {
                  alvin_conversation::Level::Info => cx.theme().muted_foreground,
                  alvin_conversation::Level::Warning => cx.theme().warning,
                  alvin_conversation::Level::Error => cx.theme().danger,
                })
              })
              .child(self.presentation.title.clone()),
          )
          .child(
            Button::new(SharedString::from(format!("{}-details", self.id)))
              .ghost()
              .xsmall()
              .track_focus(&details_focus)
              .toggled(self.details_open)
              .label(if self.details_open { "Hide details" } else { "Show details" })
              .on_click(cx.listener(|this, _, _, cx| {
                this.details_open = !this.details_open;
                cx.notify();
              })),
          )
          .when(!self.presentation.copy.is_empty(), |row| {
            row.child(
              Button::new(SharedString::from(format!("{}-copy", self.id)))
                .ghost()
                .xsmall()
                .track_focus(&copy_focus)
                .label("Copy")
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(body.to_string()))),
            )
          }),
      );
    if let Some(jobs) = &self.presentation.jobs {
      view = view.child(job::view(jobs, cx));
    }
    if (!self.details_open || self.presentation.body.is_empty()) && !self.presentation.summary.is_empty() {
      view = view.child(
        gpui_kit::div()
          .id(SharedString::from(format!("{}-summary", self.id)))
          .test_support()
          .w_full()
          .min_w_0()
          .truncate()
          .child(self.presentation.summary.clone()),
      );
    }
    if self.details_open && !self.presentation.body.is_empty() {
      view = view.child(
        gpui_kit::div()
          .id(SharedString::from(format!("{}-body-container", self.id)))
          .test_support()
          .child(TextView::markdown(SharedString::from(format!("{}-body", self.id)), self.presentation.body.clone()).text_sm()),
      );
    }
    if self.details_open {
      view = view.children(
        self
          .images
          .iter()
          .enumerate()
          .map(|(index, image)| image.view(SharedString::from(format!("{}-image-{index}", self.id)))),
      );
    }
    if self.details_open && self.presentation.has_raw {
      view = view.child(
        v_flex()
          .gap_1()
          .text_color(cx.theme().muted_foreground)
          .child(
            Button::new(SharedString::from(format!("{}-copy-raw", self.id)))
              .ghost()
              .xsmall()
              .track_focus(&raw_focus)
              .label("Copy raw")
              .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(raw.to_string()))),
          )
          .child(
            TextView::markdown(
              SharedString::from(format!("{}-raw", self.id)),
              self.presentation.raw_markdown.clone(),
            )
            .text_sm(),
          ),
      );
    }
    view.into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use super::Presentation;
  use alvin_protocol::CustomMessage;
  use gpui_kit::component::{button::Button, v_flex};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TestAppContext,
    Window, WindowOptions,
  };
  use serde_json::json;

  struct KeyboardFixture {
    before: FocusHandle,
    activity: Entity<super::SystemActivity>,
  }

  impl Render for KeyboardFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex()
        .w_full()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .child(self.activity.clone())
    }
  }

  #[test]
  fn assistant_outcome_uses_typed_stop_reason() {
    for reason in ["aborted", "error"] {
      let message: alvin_protocol::AssistantMessage = serde_json::from_value(json!({
        "role": "assistant", "stopReason": reason, "errorMessage": "same diagnostic\nfull detail",
        "errorStatus": 429, "futureField": {"original": true}
      }))
      .unwrap();
      let activity = super::SystemActivityData::assistant(&message).unwrap();
      assert_eq!(activity.title, if reason == "aborted" { "Interrupted" } else { "Error" });
      assert_eq!(
        activity.level,
        if reason == "aborted" {
          None
        } else {
          Some(alvin_conversation::Level::Error)
        }
      );
      assert_eq!(activity.copy, "same diagnostic\nfull detail\n\nStatus: 429");
      let raw: serde_json::Value = serde_json::from_str(activity.raw.as_deref().unwrap()).unwrap();
      assert_eq!(raw["stopReason"], reason);
      assert_eq!(raw["errorStatus"], 429);
      assert_eq!(raw["futureField"]["original"], true);
    }
    let interrupted: alvin_protocol::AssistantMessage = serde_json::from_value(json!({
      "role": "assistant", "stopReason": "aborted", "errorMessage": "cancelled for another reason"
    }))
    .unwrap();
    assert_eq!(super::SystemActivityData::assistant(&interrupted).unwrap().title, "Interrupted");
    let error: alvin_protocol::AssistantMessage = serde_json::from_value(json!({
      "role": "assistant", "errorMessage": "cancelled for another reason"
    }))
    .unwrap();
    assert_eq!(
      super::SystemActivityData::assistant(&error).unwrap().level,
      Some(alvin_conversation::Level::Error)
    );
    assert!(super::SystemActivityData::assistant(&alvin_protocol::AssistantMessage::default()).is_none());
  }

  #[gpui_kit::test]
  fn assistant_diagnostic_disclosure_copy_and_update_preserve_details(cx: &mut TestAppContext) {
    let message: alvin_protocol::AssistantMessage = serde_json::from_value(json!({
      "role": "assistant", "stopReason": "aborted", "errorMessage": "original\ncomplete reason",
      "errorStatus": 503, "extraDiagnostic": "preserved"
    }))
    .unwrap();
    cx.update(gpui_kit::init);
    let (window, activity) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| super::SystemActivity::new_activity("diagnostic".into(), super::SystemActivityData::assistant(&message).unwrap(), cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("diagnostic-group").label(), Some("Interrupted"));
      window.click("diagnostic-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("original\ncomplete reason\n\nStatus: 503".to_owned())
      );
      window.click("diagnostic-details", cx);
      assert!(window.find("diagnostic-body-container").visible());
      window.click("diagnostic-copy-raw", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      let raw: serde_json::Value = serde_json::from_str(&copied).unwrap();
      assert_eq!(raw["errorMessage"], "original\ncomplete reason");
      assert_eq!(raw["errorStatus"], 503);
      assert_eq!(raw["extraDiagnostic"], "preserved");
      activity.update(cx, |activity, cx| {
        activity.sync_activity(super::SystemActivityData::assistant(&message).unwrap(), cx)
      });
      window.render_frame(cx);
      assert!(window.find("diagnostic-body-container").visible());
    })
    .expect("inspect diagnostic");
  }

  #[gpui_kit::test]
  fn assistant_outcome_without_diagnostic_has_no_empty_copy(cx: &mut TestAppContext) {
    let message: alvin_protocol::AssistantMessage = serde_json::from_value(json!({
      "role": "assistant", "stopReason": "aborted"
    }))
    .unwrap();
    cx.update(gpui_kit::init);
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          super::SystemActivity::new_activity(
            "empty-diagnostic".into(),
            super::SystemActivityData::assistant(&message).unwrap(),
            cx,
          )
        })
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("empty-diagnostic-copy").is_none());
      window.click("empty-diagnostic-details", cx);
      window.click("empty-diagnostic-copy-raw", cx);
      let raw = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(serde_json::from_str::<serde_json::Value>(&raw).unwrap()["stopReason"], "aborted");
    })
    .expect("inspect outcome");
  }

  #[test]
  fn constructed_irc_preserves_identity_and_original_content() {
    let message: CustomMessage = serde_json::from_value(json!({
      "role": "custom", "customType": "irc:incoming", "content": "original envelope",
      "details": {"id": "msg-1", "from": "peer", "message": "**hello**", "future": true},
      "attribution": "agent", "display": true
    }))
    .unwrap();
    let view = Presentation::custom(&message);
    assert!(view.title.contains("peer"));
    assert_eq!(view.level, None);
    assert_eq!(view.body.as_ref(), "**hello**");
    let raw: serde_json::Value = serde_json::from_str(&view.raw).unwrap();
    assert_eq!(raw["content"], "original envelope");
    assert_eq!(raw["details"]["id"], "msg-1");
    assert_eq!(raw["details"]["future"], true);
  }

  #[gpui_kit::test]
  fn recorded_unknown_activity_keeps_original_details_and_hidden_visibility(cx: &mut TestAppContext) {
    let recording: serde_json::Value =
      serde_json::from_str(include_str!("../../../../fixtures/real_system_category.messages.json")).unwrap();
    let visible: CustomMessage = serde_json::from_value(recording["messages"][0].clone()).unwrap();
    let hidden: CustomMessage = serde_json::from_value(recording["messages"][1].clone()).unwrap();
    cx.update(gpui_kit::init);
    let (window, root) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        let activity = cx.new(|cx| super::SystemActivity::new("recorded-visible".into(), &visible, cx));
        cx.new(|cx| KeyboardFixture {
          before: cx.focus_handle().tab_stop(true),
          activity,
        })
      })
      .unwrap()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.find("recorded-visible-group").visible());
      assert!(window.try_find("recorded-visible-body-container").is_none());
      window.click("recorded-visible-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
        recording["messages"][0]["content"].as_str()
      );
      window.click("recorded-visible-details", cx);
      assert!(window.find("recorded-visible-body-container").visible());
      window.click("recorded-visible-copy-raw", cx);
      let raw: serde_json::Value = serde_json::from_str(&cx.read_from_clipboard().and_then(|item| item.text()).unwrap()).unwrap();
      assert_eq!(raw["customType"], recording["messages"][0]["customType"]);
      assert_eq!(raw["content"], recording["messages"][0]["content"]);
      assert_eq!(raw["details"], recording["messages"][0]["details"]);
      let activity = cx.new(|cx| super::SystemActivity::new("recorded-hidden".into(), &hidden, cx));
      root.update(cx, |fixture, cx| {
        fixture.activity = activity;
        cx.notify();
      });
      window.render_frame(cx);
      assert!(window.try_find("recorded-hidden-group").is_none());
      assert!(window.try_find("recorded-visible-group").is_none());
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn constructed_agent_directive_keeps_captured_image_behind_disclosure(cx: &mut TestAppContext) {
    let events: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_image_url_read.events.json")).unwrap();
    let image = events
      .as_array()
      .unwrap()
      .iter()
      .filter_map(|event| event.pointer("/body/frame"))
      .find(|frame| frame["type"] == "tool_execution_end" && frame["toolName"] == "read")
      .unwrap()["result"]["content"]
      .as_array()
      .unwrap()
      .iter()
      .find(|part| part["type"] == "image")
      .unwrap()
      .clone();
    let message: alvin_protocol::AgentMessage = serde_json::from_value(json!({
      "role": "user", "synthetic": true, "attribution": "agent",
      "content": [{"type": "text", "text": "Agent context"}, image]
    }))
    .unwrap();
    assert!(super::super::super::synthetic_directive(&message));
    cx.update(gpui_kit::init);
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| super::SystemActivity::new_synthetic(SharedString::from("image-directive"), &message, cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("image-directive-image-0").is_none());
      window.click("image-directive-details", cx);
      assert!(window.find("image-directive-image-0").visible());
      window.click("image-directive-copy-raw", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      let raw: serde_json::Value = serde_json::from_str(&copied).unwrap();
      assert_eq!(raw["content"][1]["mimeType"], "image/webp");
      assert!(raw["content"][1]["data"].as_str().is_some_and(|data| !data.is_empty()));
    })
    .expect("inspect image directive");
  }

  #[gpui_kit::test]
  fn constructed_system_disclosure_and_copy_use_original(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let message: CustomMessage = serde_json::from_value(json!({
      "role": "custom", "customType": "thinking-loop-redirect", "display": true,
      "content": "<system-interrupt>redirect</system-interrupt>", "details": {"reason": "loop"}
    }))
    .unwrap();
    let (window, block) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| super::SystemActivity::new(SharedString::from("system"), &message, cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("system-group").label(), Some("Custom · thinking-loop-redirect"));
      assert!(window.find("system-summary").visible());
      assert!(!block.read(cx).details_open);
      assert!(window.try_find("system-body-container").is_none());
      assert!(window.try_find("system-copy-raw").is_none());
      window.click("system-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("<system-interrupt>redirect</system-interrupt>".to_owned())
      );
      window.click("system-details", cx);
      assert!(block.read(cx).details_open);
      assert!(window.try_find("system-summary").is_none());
      assert!(window.find("system-body-container").visible());
      window.click("system-copy-raw", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap()["details"]["reason"],
        "loop"
      );
    })
    .expect("interact with system event");
  }
  #[gpui_kit::test]
  fn constructed_long_activity_summary_stays_in_one_line(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let title = format!("Long event {}", "path/".repeat(300));
    let data = super::SystemActivityData {
      title: title.clone(),
      summary: "unbroken".repeat(2000),
      body: "Full content".to_owned(),
      copy: "Full content".to_owned(),
      raw: None,
      level: None,
    };
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| super::SystemActivity::new_activity("long".into(), data, cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let group = window.find("long-group");
      let summary = window.find("long-summary");
      assert_eq!(group.label(), Some(title.as_str()));
      for id in ["long-details", "long-copy"] {
        let bounds = window.find(id).bounds();
        assert!(bounds.left() >= group.bounds().left());
        assert!(bounds.right() <= group.bounds().right());
      }
      assert!(summary.bounds().size.width <= group.bounds().size.width);
      assert!(summary.bounds().size.height < gpui_kit::rems(3.).to_pixels(window.rem_size()));
      window.click("long-details", cx);
      assert!(window.try_find("long-summary").is_none());
      assert!(window.find("long-body-container").visible());
    })
    .expect("inspect activity");
  }

  #[gpui_kit::test]
  fn recorded_async_delivery_shares_compact_disclosure(cx: &mut TestAppContext) {
    let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/task_turn.messages.json")).unwrap();
    let alvin_protocol::AgentMessage::Custom(message) = serde_json::from_value(recording["messages"][7].clone()).unwrap() else {
      panic!("custom message expected")
    };
    let expected = message.content.as_ref().map(super::content_text).unwrap_or_default();
    cx.update(gpui_kit::init);
    let (window, _) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| super::SystemActivity::new(SharedString::from("delivery"), &message, cx))
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("delivery-body-container").is_none());
      window.click("delivery-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(expected));
      window.click("delivery-details", cx);
      assert!(window.find("delivery-body-container").visible());
      window.click("delivery-copy-raw", cx);
      let raw = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap()["customType"],
        "async-result"
      );
    })
    .expect("interact with delivery");
  }

  #[test]
  fn unknown_custom_preserves_full_details_and_prose() {
    let message: CustomMessage = serde_json::from_value(json!({
      "role": "custom", "customType": "future-type", "content": "**prose**", "details": {"unknown": [1, 2]}
    }))
    .unwrap();
    let view = Presentation::custom(&message);
    assert_eq!(view.title.as_ref(), "Custom · future-type");
    assert_eq!(view.body.as_ref(), "**prose**");
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(&view.raw).unwrap()["details"]["unknown"],
      json!([1, 2])
    );
  }
  #[gpui_kit::test]
  fn disclosure_and_copy_preserve_original_content(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let message: CustomMessage = serde_json::from_value(json!({
      "role": "custom", "customType": "irc:incoming", "content": "original envelope",
      "details": {"id": "one", "from": "peer", "message": "**hello**", "extra": "preserved"}
    }))
    .unwrap();
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| KeyboardFixture {
          before: cx.focus_handle().tab_stop(true),
          activity: cx.new(|cx| super::SystemActivity::new(SharedString::from("event"), &message, cx)),
        })
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("event-body-container").is_none());
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      window.press("tab", cx);
      assert_eq!(window.find("event-details").focused(), Some(true));
      window.press("enter", cx);
      assert!(window.find("event-body-container").visible());
      assert!(window.find("event-copy-raw").visible());
      window.press("tab", cx);
      assert_eq!(window.find("event-copy").focused(), Some(true));
      window.press("enter", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("original envelope".to_owned())
      );
      window.press("tab", cx);
      assert_eq!(window.find("event-copy-raw").focused(), Some(true));
      window.press("enter", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap()["details"]["extra"],
        "preserved"
      );
      window.click("event-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("original envelope".to_owned())
      );
      window.click("event-copy-raw", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap()["details"]["extra"],
        "preserved"
      );
      window.click("event-details", cx);
      assert!(window.try_find("event-copy-raw").is_none());
    })
    .expect("interact with event");
  }
}
