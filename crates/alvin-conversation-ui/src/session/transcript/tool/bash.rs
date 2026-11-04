use serde_json::Value;

use super::{Body, Presentation, Summary, result_text, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let source = text(args, "command")?;
  if result.is_some_and(|value| value.get("content").and_then(Value::as_array).is_none()) {
    return None;
  }
  Some(Presentation {
    summary: Summary::Label("Command".into()),
    body: Body::Script {
      source: source.to_owned(),
      language: "bash".into(),
      output: result.map(result_text).unwrap_or_default(),
    },
  })
}

#[cfg(test)]
mod tests {
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{AppContext as _, TestAppContext, WindowOptions};
  use serde_json::Value;

  use super::super::{ToolBlock, ToolInput};

  #[gpui_kit::test]
  fn recorded_single_call_replaces_partial_and_keeps_disclosure(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let events: Vec<Value> = serde_json::from_str(include_str!("../../../../fixtures/real_single_call_stream.events.json")).unwrap();
    let frames: Vec<_> = events.iter().filter_map(|event| event.pointer("/body/frame")).collect();
    let start = frames.iter().find(|frame| frame["type"] == "tool_execution_start").unwrap();
    let partial = frames.iter().find(|frame| frame["type"] == "tool_execution_update").unwrap();
    let end = frames.iter().find(|frame| frame["type"] == "tool_execution_end").unwrap();
    assert_eq!(start["toolCallId"], partial["toolCallId"]);
    assert_eq!(start["toolCallId"], end["toolCallId"]);
    assert_eq!(start["toolName"], "bash");
    assert_eq!(partial["partialResult"]["content"][0]["text"], "STREAM_BEFORE\n");
    assert_eq!(end["result"]["content"][0]["text"], "STREAM_BEFORE\nSTREAM_AFTER\n");
    let mut input = ToolInput {
      name: "bash".into(),
      task_progress: None,
      intent: start["intent"].as_str().map(str::to_owned),
      args: Some(start["args"].clone()),
      delivery: None,
      result: Some(partial["partialResult"].clone()),
      stream: Vec::new(),
      running: true,
      failed: false,
      exit_code: None,
      timestamp: None,
    };
    let (window, block) = cx
      .update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
          cx.new(|cx| ToolBlock::new("single-call".into(), input.clone(), cx))
        })
      })
      .unwrap();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click("single-call-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
        Some("STREAM_BEFORE\n")
      );
      window.click("single-call-toggle", cx);
    })
    .unwrap();
    input.result = Some(end["result"].clone());
    input.running = false;
    cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
    cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("single-call-body").is_none());
      window.click("single-call-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
        Some("STREAM_BEFORE\nSTREAM_AFTER\n")
      );
      window.click("single-call-toggle", cx);
      window.click("single-call-raw-toggle", cx);
      window.click("single-call-copy", cx);
      let raw: Value = serde_json::from_str(&cx.read_from_clipboard().and_then(|item| item.text()).unwrap()).unwrap();
      assert_eq!(raw["result"], end["result"]);
    })
    .unwrap();
  }
}
