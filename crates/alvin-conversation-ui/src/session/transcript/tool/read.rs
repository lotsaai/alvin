use std::collections::HashMap;

use gpui_kit::SharedString;

use serde_json::Value;

use super::{Body, Presentation, Summary, result_text, text};

#[derive(Clone)]
pub(super) struct DisplayContent {
  pub(super) text: String,
  pub(super) start_line: Option<u64>,
  pub(super) line_numbers: Vec<u64>,
}
#[derive(Clone)]
pub(super) struct Target {
  pub(super) path: String,
  pub(super) link: Option<SharedString>,
  pub(super) key: SharedString,
  pub(super) selection_key: SharedString,
  pub(super) label: SharedString,
}

impl DisplayContent {
  fn parse(value: &Value) -> Option<Self> {
    Some(Self {
      text: value.get("text")?.as_str()?.to_owned(),
      start_line: value.get("startLine").and_then(Value::as_u64),
      line_numbers: value
        .get("lineNumbers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .collect(),
    })
  }
}

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let path = text(args, "path")?;
  let result = result?;
  let mut occurrences = HashMap::<&str, usize>::new();
  let mut identities = HashMap::<(&str, Option<&str>), usize>::new();
  let targets: Vec<Target> = result
    .pointer("/details/displayReadTargets")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .enumerate()
    .filter_map(|(index, value)| {
      let path = value.as_str()?;
      let occurrence = occurrences.entry(path).or_default();
      let key = SharedString::from(format!("{occurrence}:{path}"));
      *occurrence = occurrence.saturating_add(1);
      let link = result
        .pointer("/details/displayReadTargetLinks")
        .and_then(Value::as_array)
        .and_then(|links| links.get(index))
        .and_then(Value::as_str);
      let identity = identities.entry((path, link)).or_default();
      let selection_key = SharedString::from(format!("{}:{path}:{link:?}:{identity}", path.len()));
      *identity = identity.saturating_add(1);
      let link = link.map(|link| SharedString::from(link.to_owned()));
      Some(Target {
        path: path.to_owned(),
        label: SharedString::from(format!("Details for {path}")),
        link,
        key,
        selection_key,
      })
    })
    .collect();
  if result.pointer("/details/displayContent/text").and_then(Value::as_str).is_none()
    && result.get("content").and_then(Value::as_array).is_none()
  {
    return None;
  }
  Some(Presentation {
    summary: if targets.len() > 1 {
      Summary::Label(format!("{} targets", targets.len()))
    } else {
      Summary::Resource(path.to_owned())
    },
    body: Body::Read {
      targets,
      display: result.pointer("/details/displayContent").and_then(DisplayContent::parse),
      output: result_text(result),
    },
  })
}

#[cfg(test)]
mod tests {
  use gpui_kit::component::{button::Button, v_flex};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render, TestAppContext, Window, WindowOptions, px, size,
  };
  use serde_json::json;

  use super::super::{ToolBlock, ToolInput};

  struct KeyboardFixture {
    before: FocusHandle,
    block: Entity<ToolBlock>,
  }

  impl Render for KeyboardFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .child(self.block.clone())
    }
  }

  #[gpui_kit::test]
  fn target_details_are_independent_and_survive_appended_and_reordered_targets(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
      cx.set_global(crate::ConversationPreferences::new(true, false, false));
    });
    let recording: serde_json::Value =
      serde_json::from_str(include_str!("../../../../fixtures/real_mixed_read_stream.messages.json")).unwrap();
    let result = recording["messages"]
      .as_array()
      .unwrap()
      .iter()
      .find(|message| message.pointer("/details/displayReadTargets").is_some())
      .unwrap()
      .clone();
    let output = result["content"][0]["text"].as_str().unwrap().to_owned();
    let mut input = ToolInput {
      name: "read".into(),
      task_progress: None,
      intent: None,
      args: Some(json!({"path":"alpha.txt;beta.txt;gamma.txt"})),
      delivery: None,
      result: Some(result),
      stream: Vec::new(),
      running: false,
      failed: false,
      exit_code: None,
      timestamp: None,
    };
    let (window, fixture) = cx
      .update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
          let block = cx.new(|cx| ToolBlock::new("targets".into(), input.clone(), cx));
          cx.new(|cx| KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            block,
          })
        })
      })
      .unwrap();
    let block = cx.update(|cx| fixture.read(cx).block.clone());
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("targets-target-0").is_none());
      window.click("targets-toggle", cx);
      for index in 0..3 {
        assert_eq!(
          window.within(format!("targets-target-{index}")).find("target-toggle").checked(),
          Some(false)
        );
      }
      window.within("targets-target-0").click("target-toggle", cx);
      window.within("targets-target-2").click("target-toggle", cx);
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.press("tab", cx);
      while window.within("targets-target-2").find("target-toggle").focused() != Some(true) {
        assert!(!before.is_focused(window), "Target details are unreachable by keyboard");
        window.press("tab", cx);
      }
      window.press("enter", cx);
      assert!(window.within("targets-target-2").try_find("target-details").is_none());
      window.press("enter", cx);
      assert_eq!(window.within("targets-target-0").find("target-path").label(), Some("alpha.txt"));
      assert!(window.within("targets-target-1").try_find("target-details").is_none());
      assert_eq!(window.within("targets-target-2").find("target-path").label(), Some("gamma.txt"));
      window.click("targets-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(output.clone()));
    })
    .unwrap();
    let details = &mut input.result.as_mut().unwrap()["details"];
    let paths = details["displayReadTargets"].as_array_mut().unwrap();
    paths.swap(0, 1);
    paths.push(json!("agent://worker/界"));
    let links = details["displayReadTargetLinks"].as_array_mut().unwrap();
    links.swap(0, 1);
    links.push(json!("agent://worker/界?full=1"));
    cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.within("targets-target-1").find("target-toggle").checked(), Some(true));
      assert_eq!(window.within("targets-target-0").find("target-toggle").checked(), Some(false));
      assert_eq!(window.within("targets-target-2").find("target-toggle").checked(), Some(true));
      window.within("targets-target-3").click("target-toggle", cx);
      assert_eq!(
        window.within("targets-target-3").find("target-link").label(),
        Some("agent://worker/界?full=1")
      );
      window.within("targets-target-1").click("target-toggle", cx);
      assert!(window.within("targets-target-1").try_find("target-details").is_none());
      assert!(window.within("targets-target-2").find("target-details").visible());
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn duplicate_long_unicode_targets_keep_separate_details_and_exact_copy(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
      cx.set_global(crate::ConversationPreferences::new(true, false, false));
    });
    let path = format!("file:///{}/界.txt:2-4", "long".repeat(40));
    let link = format!("https://example.test/{}", "資".repeat(80));
    let output = "full combined output\n界".repeat(80);
    let input = ToolInput {
      name: "read".into(),
      task_progress: None,
      intent: None,
      args: Some(json!({"path":"opaque"})),
      delivery: None,
      result: Some(json!({"content":[{"type":"text","text":output}], "details":{
        "displayReadTargets":[path, path], "displayReadTargetLinks":[link, null]
      }})),
      stream: Vec::new(),
      running: false,
      failed: false,
      exit_code: None,
      timestamp: None,
    };
    let (window, _) = cx
      .update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
          cx.new(|cx| ToolBlock::new("duplicate".into(), input, cx))
        })
      })
      .unwrap();
    cx.simulate_window_resize(window, size(px(320.), px(720.)));
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("duplicate-target-0").is_none());
      window.click("duplicate-toggle", cx);
      window.within("duplicate-target-0").click("target-toggle", cx);
      assert_eq!(window.within("duplicate-target-0").find("target-path").label(), Some(path.as_str()));
      assert_eq!(window.within("duplicate-target-0").find("target-link").label(), Some(link.as_str()));
      assert_eq!(window.within("duplicate-target-1").find("target-toggle").checked(), Some(false));
      window.within("duplicate-target-0").click("target-path-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(path.clone()));
      window.within("duplicate-target-0").click("target-link-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(link.clone()));
      let preview = window.find("duplicate-output-preview").bounds().size.height;
      assert!(preview <= window.rem_size() * 20.);
      window.click("duplicate-extent", cx);
      assert!(window.find("duplicate-output-preview").bounds().size.height > preview);
      assert!(window.within("duplicate-target-0").find("target-details").visible());
      window.click("duplicate-extent", cx);
      assert!(window.find("duplicate-output-preview").bounds().size.height <= window.rem_size() * 20.);
      window.click("duplicate-copy", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(output));
      window.within("duplicate-target-0").click("target-toggle", cx);
      window.within("duplicate-target-1").click("target-toggle", cx);
      assert!(window.within("duplicate-target-1").try_find("target-link").is_none());
      assert_eq!(window.within("duplicate-target-1").find("target-path").label(), Some(path.as_str()));
    })
    .unwrap();
  }
}
