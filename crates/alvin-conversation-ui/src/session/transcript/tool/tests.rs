use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, TestAppContext, WindowOptions};
use serde_json::json;

use super::{ToolBlock, ToolInput};

fn bash(output: &str) -> ToolInput {
  ToolInput {
    name: "bash".into(),
    intent: None,
    args: Some(json!({"command": "printf output"})),
    result: Some(json!({"content": [{"type": "text", "text": output}], "details": {"exitCode": 7, "wallTimeMs": 18}})),
    stream: Vec::new(),
    running: false,
    failed: true,
    exit_code: None,
    timestamp: None,
  }
}

fn render_layout_frames(cx: &mut TestAppContext, handle: AnyWindowHandle) {
  while cx
    .update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      window.simulate_next_frame(cx)
    })
    .unwrap()
    > 0
  {}
}

#[gpui_kit::test]
fn long_unicode_output_survives_expand_collapse_and_update(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let output = (0..40)
    .map(|line| format!("界 `{line}` {}", "界".repeat(64)))
    .collect::<Vec<_>>()
    .join("\n");
  let input = bash(&output);
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("bash-test".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert_eq!(window.find("bash-test-toggle").checked(), Some(true));
    assert_eq!(window.find("bash-test-extent").checked(), Some(false));
    assert!(window.find("bash-test-body").bounds().size.height <= window.rem_size() * 20.);
    window.click("bash-test-copy", cx);
    assert_eq!(cx.read_from_clipboard().unwrap().text(), Some(format!("$ printf output\n{output}")));
    window.click("bash-test-extent", cx);
    assert_eq!(window.find("bash-test-extent").checked(), Some(true));
  })
  .unwrap();
  cx.run_until_parked();
  render_layout_frames(cx, window);
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.find("bash-test-body").bounds().size.height > window.rem_size() * 20.);
    window.click("bash-test-toggle", cx);
    assert!(window.try_find("bash-test-details").is_none());
    window.click("bash-test-toggle", cx);
    assert_eq!(window.find("bash-test-extent").checked(), Some(true));
  })
  .unwrap();
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&bash("finished"), cx)));
  cx.run_until_parked();
  render_layout_frames(cx, window);
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.try_find("bash-test-extent").is_none());
    assert_eq!(window.find("bash-test-toggle").checked(), Some(true));
  })
  .unwrap();
}

#[gpui_kit::test]
fn diff_keeps_expansion_when_card_reopens(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let diff = (1..=30).map(|line| format!("+{line}|line {line}\n")).collect::<String>();
  let input = ToolInput {
    name: "edit".into(),
    intent: None,
    args: Some(json!({"path": "src/main.rs"})),
    result: Some(json!({"details": {"diff": diff}})),
    stream: Vec::new(),
    running: false,
    failed: false,
    exit_code: None,
    timestamp: None,
  };
  let mut updated = input.clone();
  updated.timestamp = Some(1234.);
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("diff-test".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("diff-test-extent", cx);
    window.click("collapse-file", cx);
    assert_eq!(window.find("diff-test-extent").checked(), Some(true));
    window.click("diff-test-toggle", cx);
    window.click("diff-test-toggle", cx);
    assert_eq!(window.find("diff-test-extent").checked(), Some(true));
    window.click("diff-test-extent", cx);
    assert_eq!(window.find("diff-test-extent").checked(), Some(false));
  })
  .unwrap();
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&updated, cx)));
  cx.update(|cx| {
    let card = block.read(cx).card.read(cx);
    assert!(card.diff.as_ref().unwrap().read(cx).is_file_collapsed("src/main.rs"));
  });
  cx.update(|cx| gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx));
  cx.update(|cx| {
    let card = block.read(cx).card.read(cx);
    assert!(card.diff.as_ref().unwrap().read(cx).is_file_collapsed("src/main.rs"));
  });
}

#[test]
fn specialized_adapters_preserve_content_and_reject_malformed_results() {
  let result = json!({"content": [{"type":"text","text":"first"}, {"type":"text","text":"second"}]});
  let command = json!({"command":"echo test"});
  assert_eq!(
    super::bash::parse(Some(&command), Some(&result)).unwrap().content,
    "$ echo test\nfirst\nsecond"
  );
  let args = json!({"path":"notes.txt","content":"written"});
  assert_eq!(super::read::parse(Some(&args), Some(&result)).unwrap().content, "first\nsecond");
  assert_eq!(super::write::parse(Some(&args)).unwrap().content, "written");
  let patch = json!({"details":{"diff":"-1|before\n+1|after\n"}});
  let patch = super::edit::parse(Some(&args), Some(&patch)).unwrap();
  let parsed = gpui_kit::component::diff::DiffFile::parse(&patch.content).unwrap();
  assert_eq!(parsed[0].additions(), 1);
  assert_eq!(parsed[0].deletions(), 1);
  let files = json!({"details":{"files":["one","two"]}});
  assert_eq!(super::search::parse(None, Some(&files)).unwrap().content, "one\ntwo");
  let progress = json!({"details":{"progress":[{"id":"a","agent":"worker","status":"running","toolCount":3,"assignment":"review"}]}});
  assert!(super::task::parse(None, Some(&progress)).unwrap().content.contains("3 tool calls"));
  let malformed = json!({"error":"permission denied"});
  assert!(super::read::parse(Some(&args), Some(&malformed)).is_none());
  assert!(super::search::parse(None, Some(&malformed)).is_none());
  assert!(super::bash::parse(Some(&command), Some(&malformed)).is_none());
}

#[test]
fn raw_output_redacts_image_data_without_losing_metadata() {
  let mut input = bash("");
  input.name = "unknown".into();
  input.result = Some(json!({"content":[{"type":"image","mimeType":"image/png","data":"private-image-data"}]}));
  let raw = super::raw_text(&input);
  assert!(!raw.contains("private-image-data"));
  assert!(raw.contains("image/png"));
  assert!(raw.contains("[image data omitted]"));
}

#[gpui_kit::test]
fn preview_measures_combined_content_at_narrow_width(cx: &mut TestAppContext) {
  use gpui_kit::{Bounds, WindowBounds, px, size};
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let mut input = bash("small result");
  input.result = Some(json!({"content": (0..12).map(|_| json!({"type":"image","mimeType":"image/png"})).collect::<Vec<_>>()}));
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(240.), px(900.))))),
          ..Default::default()
        },
        cx,
        |_, cx| cx.new(|cx| ToolBlock::new("narrow".into(), input, cx)),
      )
    })
    .unwrap();
  render_layout_frames(cx, window);
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.find("narrow-body").bounds().size.height <= window.rem_size() * 20.);
    assert_eq!(window.find("narrow-extent").checked(), Some(false));
    window.click("narrow-extent", cx);
    assert!(window.find("narrow-body").bounds().size.height > window.rem_size() * 20.);
  })
  .unwrap();
}

#[gpui_kit::test]
fn narrow_wrapped_text_offers_expansion_without_logical_truncation(cx: &mut TestAppContext) {
  use gpui_kit::{Bounds, WindowBounds, px, size};
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let text = "word ".repeat(300);
  let mut input = bash(&text);
  input.name = "read".into();
  input.args = Some(json!({"path":"notes.txt"}));
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(240.), px(900.))))),
          ..Default::default()
        },
        cx,
        |_, cx| cx.new(|cx| ToolBlock::new("wrapped".into(), input, cx)),
      )
    })
    .unwrap();
  render_layout_frames(cx, window);
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.find("wrapped-body").bounds().size.height <= window.rem_size() * 20.);
    assert_eq!(window.find("wrapped-extent").checked(), Some(false));
    window.click("wrapped-extent", cx);
    assert!(window.find("wrapped-body").bounds().size.height > window.rem_size() * 20.);
    window.click("wrapped-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text.clone()));
    window.click("wrapped-extent", cx);
    assert!(window.find("wrapped-body").bounds().size.height <= window.rem_size() * 20.);
  })
  .unwrap();
}
