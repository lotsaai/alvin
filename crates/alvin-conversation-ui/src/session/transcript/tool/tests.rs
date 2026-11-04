use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, TestAppContext, WindowOptions};
use serde_json::json;

use super::{ToolBlock, ToolInput};

fn bash(output: &str) -> ToolInput {
  ToolInput {
    name: "bash".into(),
    task_progress: None,
    intent: None,
    args: Some(json!({"command": "printf output"})),
    result: Some(json!({"content": [{"type": "text", "text": output}], "details": {"exitCode": 7, "wallTimeMs": 18}})),
    delivery: None,
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
  let output = (0..40).map(|line| format!("界 {line}")).collect::<Vec<_>>().join("\n");
  let mut input = bash(&output);
  input.args = Some(json!({"command": "one\ntwo\nthree\nfour\nfive\nsix"}));
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("bash-test".into(), input.clone(), cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert_eq!(window.find("bash-test-status").label(), Some("Failed"));
    assert_eq!(window.find("bash-test-exit").label(), Some("Exit code 7"));
    assert!(window.try_find("bash-test-extent").is_none());
    assert_eq!(window.find("bash-test-source-extent").checked(), Some(false));
    assert_eq!(window.find("bash-test-output-extent").checked(), Some(false));
    let source = window.find("bash-test-source").bounds();
    let output_section = window.find("bash-test-output").bounds();
    assert!(window.find("bash-test-source-text").bounds().bottom() <= window.find("bash-test-source-extent").bounds().top());
    assert!(window.find("bash-test-output-extent").bounds().bottom() <= window.find("bash-test-output-text").bounds().top());
    window.click("bash-test-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(output.clone()));
    window.click("bash-test-source-extent", cx);
    assert_eq!(window.find("bash-test-source-extent").checked(), Some(true));
    assert_eq!(window.find("bash-test-output-extent").checked(), Some(false));
    window.render_frame(cx);
    assert!(window.find("bash-test-source").bounds().size.height > source.size.height);
    assert_eq!(window.find("bash-test-output").bounds().size.height, output_section.size.height);
    let expanded_source = window.find("bash-test-source").bounds();
    window.click("bash-test-output-extent", cx);
    window.render_frame(cx);
    assert!(window.find("bash-test-output").bounds().size.height > output_section.size.height);
    assert_eq!(window.find("bash-test-source").bounds().size.height, expanded_source.size.height);
    window.click("bash-test-toggle", cx);
    window.click("bash-test-toggle", cx);
    window.click("bash-test-raw-toggle", cx);
    window.click("bash-test-raw-toggle", cx);
    assert_eq!(window.find("bash-test-source-extent").checked(), Some(true));
    assert_eq!(window.find("bash-test-output-extent").checked(), Some(true));
  })
  .unwrap();
  input.result = Some(json!({"content": [{"type":"text","text": format!("{output}\nnew")}], "details":{"exitCode":7}}));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert_eq!(window.find("bash-test-source-extent").checked(), Some(true));
    assert_eq!(window.find("bash-test-output-extent").checked(), Some(true));
    window.click("bash-test-source-extent", cx);
    window.click("bash-test-output-extent", cx);
    assert_eq!(window.find("bash-test-source-extent").checked(), Some(false));
    assert_eq!(window.find("bash-test-output-extent").checked(), Some(false));
  })
  .unwrap();
}

#[gpui_kit::test]
fn eval_renders_script_and_copies_full_result(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let output = (0..20).map(|line| format!("result {line}\n")).collect::<String>();
  let mut input = bash(&output);
  input.name = "eval".into();
  input.args = Some(json!({"code":"print(1)\nprint(2)\nprint(3)\nprint(4)\nprint(5)", "language":"python"}));
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("eval-test".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.find("eval-test-source").visible());
    assert!(window.find("eval-test-output").visible());
    assert!(window.try_find("eval-test-extent").is_none());
    window.click("eval-test-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(output));
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
    task_progress: None,
    intent: None,
    args: Some(json!({"path": "src/main.rs"})),
    result: Some(json!({"details": {"diff": diff}})),
    delivery: None,
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
fn known_readonly_tools_and_unknown_capabilities_are_classified_conservatively() {
  use super::{ToolCapability, capability};
  for name in ["read", "grep", "glob", "find", "ast_grep", "web_search"] {
    assert_eq!(capability(name), ToolCapability::Readonly);
  }
  for name in ["bash", "eval", "write", "edit", "task", "unknown"] {
    assert_eq!(capability(name), ToolCapability::PotentiallyMutating);
  }
}

#[test]
fn recorded_tool_results_preserve_resource_and_search_details() {
  let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/tools_turn.messages.json")).unwrap();
  let messages = recording["messages"].as_array().unwrap();
  let read = messages
    .iter()
    .find(|message| message["role"] == "toolResult" && message["toolName"] == "read")
    .unwrap();
  let args = messages
    .iter()
    .filter_map(|message| message["content"].as_array())
    .flatten()
    .find(|block| block["type"] == "toolCall" && block["name"] == "read")
    .unwrap();
  let presentation = super::read::parse(Some(&args["arguments"]), Some(read)).unwrap();
  assert_eq!(presentation.body.metadata(), "Starting line 1 · 2 reported lines");
  let super::Body::Read {
    display: Some(display), ..
  } = presentation.body
  else {
    panic!("expected read content")
  };
  assert_eq!(display.text, "alpha\nbeta");
  assert_eq!(display.start_line, Some(1));
  assert_eq!(display.line_numbers, [1, 2]);

  let result = messages
    .iter()
    .find(|message| message["role"] == "toolResult" && message["toolName"] == "grep")
    .unwrap();
  let presentation = super::search::parse(None, Some(result)).unwrap();
  assert_eq!(presentation.body.metadata(), "1 match · 1 file · notes.txt (1 match)");
  let super::Body::Search(search) = presentation.body else {
    panic!("expected search results")
  };
  assert_eq!(search.files, ["notes.txt"]);
  assert_eq!(search.matches[0].path, "notes.txt");
  assert_eq!(search.matches[0].count, Some(1));
  assert_eq!(search.match_count, Some(1));
  assert_eq!(search.file_count, Some(1));
  assert_eq!(search.truncated, Some(false));
}
#[test]
fn recorded_multiread_preserves_targets_links_and_combined_output() {
  let recording: serde_json::Value =
    serde_json::from_str(include_str!("../../../../fixtures/real_mixed_read_stream.messages.json")).unwrap();
  let messages = recording["messages"].as_array().unwrap();
  let result = messages
    .iter()
    .find(|message| message.pointer("/details/displayReadTargets").is_some())
    .unwrap();
  let presentation = super::read::parse(Some(&json!({"path":"alpha.txt;beta.txt;gamma.txt"})), Some(result)).unwrap();
  let super::Body::Read { targets, display, output } = presentation.body else {
    panic!("expected read")
  };
  assert_eq!(
    targets.iter().map(|target| target.path.as_str()).collect::<Vec<_>>(),
    ["alpha.txt", "beta.txt", "gamma.txt"]
  );
  assert!(
    targets
      .iter()
      .zip(["alpha.txt", "beta.txt", "gamma.txt"])
      .all(|(target, name)| target.link.as_deref().unwrap().ends_with(name))
  );
  assert!(display.is_none());
  assert_eq!(output, result["content"][0]["text"].as_str().unwrap());
}

#[test]
fn read_targets_preserve_duplicates_and_unicode_link_pairing() {
  let result = json!({"content":[{"type":"text","text":"combined"}], "details":{
    "displayReadTargets":["長い/界.txt:2-4","長い/界.txt:2-4","agent://worker/result"],
    "displayReadTargetLinks":["first","second","third"]
  }});
  let presentation = super::read::parse(Some(&json!({"path":"opaque;argument"})), Some(&result)).unwrap();
  let super::Body::Read { targets, output, .. } = presentation.body else {
    panic!("expected read")
  };
  assert_eq!(
    targets
      .iter()
      .map(|target| (target.path.as_str(), target.link.as_deref()))
      .collect::<Vec<_>>(),
    [
      ("長い/界.txt:2-4", Some("first")),
      ("長い/界.txt:2-4", Some("second")),
      ("agent://worker/result", Some("third"))
    ]
  );
  assert_eq!(output, "combined");
}

#[gpui_kit::test]
fn multiread_rows_keep_combined_result_copyable(cx: &mut TestAppContext) {
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
  let expected = result["content"][0]["text"].as_str().unwrap().to_owned();
  let mut input = bash("");
  input.name = "read".into();
  input.args = Some(json!({"path":"alpha.txt;beta.txt;gamma.txt"}));
  input.result = Some(result);
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("multiread".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("multiread-toggle", cx);
    assert!(window.find("multiread-target-0").visible());
    assert!(window.find("multiread-target-1").visible());
    assert!(window.find("multiread-target-2").visible());
    window.click("multiread-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(expected));
  })
  .unwrap();
}

#[test]
fn raw_output_preserves_image_data_and_metadata() {
  let mut input = bash("");
  input.name = "unknown".into();
  input.result = Some(json!({"content":[{"type":"image","mimeType":"image/png","data":"private-image-data"}]}));
  let raw = super::raw_text(&input);
  assert!(raw.contains("private-image-data"));
  assert!(raw.contains("image/png"));
}

#[test]
fn recorded_image_result_keeps_original_payload_in_raw_output() {
  let events: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_image_url_read.events.json")).unwrap();
  let result = events
    .as_array()
    .unwrap()
    .iter()
    .filter_map(|event| event.pointer("/body/frame"))
    .find(|frame| frame["type"] == "tool_execution_end" && frame["toolName"] == "read")
    .unwrap()["result"]
    .clone();
  let data = result["content"]
    .as_array()
    .unwrap()
    .iter()
    .find(|block| block["type"] == "image")
    .unwrap()["data"]
    .as_str()
    .unwrap()
    .to_owned();
  let mut input = bash("");
  input.result = Some(result);
  assert!(super::raw_text(&input).contains(&data));
}

#[gpui_kit::test]
fn recorded_read_image_renders_and_disclosure_survives_updates(cx: &mut TestAppContext) {
  use gpui_kit::{Bounds, WindowBounds, px, size};
  let events: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_image_url_read.events.json")).unwrap();
  let result = events
    .as_array()
    .unwrap()
    .iter()
    .filter_map(|event| event.pointer("/body/frame"))
    .find(|frame| frame["type"] == "tool_execution_end" && frame["toolName"] == "read")
    .unwrap()["result"]
    .clone();
  let mut input = bash("");
  input.name = "read".into();
  input.args = Some(json!({"path":"http://127.0.0.1:64722/image.webp"}));
  input.result = Some(result);
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(640.), px(700.))))),
          ..Default::default()
        },
        cx,
        |_, cx| cx.new(|cx| ToolBlock::new("image-read".into(), input.clone(), cx)),
      )
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("image-read-toggle", cx);
    let image = window.find("image-read-image-0");
    assert!(image.visible());
    assert_eq!(image.label(), Some("Image (image/webp)"));
    assert!(image.bounds().size.width <= px(640.));
    window.click("image-read-raw-toggle", cx);
    assert!(window.try_find("image-read-image-0").is_none());
    window.click("image-read-raw-toggle", cx);
    assert!(window.find("image-read-image-0").visible());
    window.click("image-read-toggle", cx);
  })
  .unwrap();
  input.timestamp = Some(123.);
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.try_find("image-read-image-0").is_none());
    window.click("image-read-toggle", cx);
    assert!(window.find("image-read-image-0").visible());
  })
  .unwrap();
}

#[gpui_kit::test]
fn preview_measures_combined_content_at_narrow_width(cx: &mut TestAppContext) {
  use gpui_kit::{Bounds, WindowBounds, px, size};
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
    cx.set_global(crate::ConversationPreferences::new(true, true, false));
  });
  let mut input = bash("small result");
  input.name = "read".into();
  input.args = Some(json!({"path":"images"}));
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
    cx.set_global(crate::ConversationPreferences::new(true, true, false));
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

#[gpui_kit::test]
fn short_output_copy_and_raw_state_survive_collapse(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let mut input = bash("short output");
  input.failed = false;
  input.args = Some(json!({"command": "printf '%s\\n' 'short output'\nexit 7"}));
  assert_eq!(input.status(), super::ToolStatus::Failed);
  assert_eq!(input.exit_code(), Some(7));
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("short".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("short-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("short output".into()));
    window.click("short-raw-toggle", cx);
    assert_eq!(window.find("short-raw-toggle").checked(), Some(true));
    window.click("short-toggle", cx);
    window.click("short-toggle", cx);
    assert_eq!(window.find("short-raw-toggle").checked(), Some(true));
    window.click("short-copy", cx);
    assert!(
      cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap()
        .contains("short output")
    );
    window.click("short-raw-toggle", cx);
    assert_eq!(window.find("short-raw-toggle").checked(), Some(false));
  })
  .unwrap();
}

#[gpui_kit::test]
fn disclosure_defaults_follow_preferences_until_manually_changed(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let mut input = bash("result");
  input.name = "read".into();
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("preference".into(), input.clone(), cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.try_find("preference-body").is_none());
  })
  .unwrap();
  cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(true, true, false)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.find("preference-body").visible());
    window.click("preference-toggle", cx);
    assert!(window.try_find("preference-body").is_none());
    input.result = Some(json!({"content": [{"type": "text", "text": "updated"}]}));
    block.update(cx, |block, cx| block.sync(&input, cx));
  })
  .unwrap();
  cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(false, false, true)));
  cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(true, true, false)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.try_find("preference-body").is_none());
    window.click("preference-toggle", cx);
    assert!(window.find("preference-body").visible());
  })
  .unwrap();
}

#[gpui_kit::test]
fn tool_families_render_their_initial_disclosure(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  for (name, expected) in [
    ("read", false),
    ("grep", false),
    ("glob", false),
    ("find", false),
    ("ast_grep", false),
    ("web_search", false),
    ("bash", true),
    ("eval", true),
    ("edit", true),
    ("write", true),
    ("task", true),
    ("unknown", true),
  ] {
    let mut input = bash("result");
    input.name = name.into();
    let (window, _) = cx
      .update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
          cx.new(|cx| ToolBlock::new("family".into(), input, cx))
        })
      })
      .unwrap();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.try_find("family-body").is_some(), expected, "{name}");
    })
    .unwrap();
  }
}

#[gpui_kit::test]
fn streamed_snapshots_replace_output_and_keep_disclosure(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_mixed_read_stream.events.json")).unwrap();
  let snapshots: Vec<_> = recording
    .as_array()
    .unwrap()
    .iter()
    .filter_map(|event| {
      let frame = event.pointer("/body/frame")?;
      (frame["type"] == "tool_execution_update" && frame["toolName"] == "bash").then(|| frame["partialResult"].clone())
    })
    .collect();
  assert!(snapshots.len() >= 2);
  let mut input = bash("");
  input.running = true;
  input.failed = false;
  input.result = Some(snapshots[0].clone());
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("stream-test".into(), input.clone(), cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("stream-test-toggle", cx);
  })
  .unwrap();
  input.result = Some(snapshots[1].clone());
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert!(window.try_find("stream-test-body").is_none());
    window.click("stream-test-copy", cx);
    assert_eq!(
      cx.read_from_clipboard().and_then(|item| item.text()),
      Some(super::result_text(&snapshots[1]))
    );
  })
  .unwrap();
  input.running = false;
  input.result = Some(json!({"content":[{"type":"text","text":"final output"}]}));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("stream-test-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("final output".into()));
  })
  .unwrap();
}

#[gpui_kit::test]
fn edit_preview_updates_and_final_result_wins(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let mut input = bash("");
  input.name = "edit".into();
  input.args = Some(json!({"path":"src/demo.ts"}));
  input.result = None;
  input.running = true;
  input.stream = vec![json!({"generation":1,"streaming":true,"files":[
    {"path":"src/demo.ts","diff":"@@ -1 +1 @@\n-old\n+first","firstChangedLine":1}
  ]})];
  let (window, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("edit-stream".into(), input.clone(), cx))
      })
    })
    .unwrap();
  input.stream.push(json!({"generation":2,"streaming":true,"files":[
    {"path":"src/demo.ts","diff":"@@ -1 +1 @@\n-old\n+second","firstChangedLine":1}
  ]}));
  input.stream.push(json!({"unknown":"retained"}));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("edit-stream-copy", cx);
    let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    assert!(text.contains("+second"));
    assert!(!text.contains("+first"));
    window.click("edit-stream-raw-toggle", cx);
    window.click("edit-stream-copy", cx);
    assert!(cx.read_from_clipboard().and_then(|item| item.text()).unwrap().contains("retained"));
    window.click("edit-stream-raw-toggle", cx);
  })
  .unwrap();
  input.running = false;
  input.result = Some(json!({"content":[{"type":"text","text":"final result"}]}));
  input.stream.push(json!({"generation":3,"streaming":false,"files":[
    {"path":"src/demo.ts","diff":"@@ -1 +1 @@\n-old\n+stale"}
  ]}));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("edit-stream-copy", cx);
    let text = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    assert!(!text.contains("+stale"));
    assert!(text.contains("final result"));
  })
  .unwrap();
  input.stream.clear();
  input.result = Some(json!({"content":[{"type":"text","text":"committed result"}]}));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    window.click("edit-stream-raw-toggle", cx);
    window.click("edit-stream-copy", cx);
    let raw = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    assert!(raw.contains("retained"));
    assert!(raw.contains("+stale"));
    assert!(raw.contains("committed result"));
  })
  .unwrap();
}

#[gpui_kit::test]
fn reported_zero_execution_metadata_is_visible(cx: &mut TestAppContext) {
  cx.update(gpui_kit::init);
  let mut input = bash("done");
  input.failed = false;
  input.result = Some(json!({"content":[{"type":"text","text":"done"}],"details":{"exitCode":0,"wallTimeMs":0}}));
  let (window, _) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("zero-metadata".into(), input, cx))
      })
    })
    .unwrap();
  cx.update_window(window, |_, window, cx| {
    window.render_frame(cx);
    assert_eq!(window.find("zero-metadata-exit").label(), Some("Exit code 0"));
    assert_eq!(window.find("zero-metadata-duration").label(), Some("0 ms"));
    assert_eq!(window.find("zero-metadata-status").label(), Some("Completed"));
  })
  .unwrap();
}

#[test]
fn argument_stream_deltas_remain_raw_not_tool_output() {
  let mut input = bash("");
  input.name = "edit".into();
  input.args = Some(json!({"path":"src/demo.ts"}));
  input.result = None;
  input.stream = vec![json!({"delta":"{\"value\":"}), json!({"delta":"\"hello\""}), json!({"delta":"}"})];
  assert!(super::present(&input).is_none());
  let raw = super::raw_text(&input);
  let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
  assert_eq!(
    value["stream"],
    json!([{"delta":"{\"value\":"},{"delta":"\"hello\""},{"delta":"}"}])
  );
}

#[gpui_kit::test]
fn task_delivery_and_latest_progress_remain_inspectable_after_refresh(cx: &mut TestAppContext) {
  cx.update(|cx| {
    gpui_kit::init(cx);
    cx.set_reduce_motion(true);
  });
  let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_task_progress.messages.json")).unwrap();
  let messages = recording["messages"].as_array().unwrap();
  let result = messages.iter().find(|message| message["toolName"] == "task").unwrap().clone();
  let delivery = messages.iter().find(|message| message["toolName"] == "wait").unwrap().clone();
  let expected = delivery["details"]["jobs"][0]["resultText"].as_str().unwrap().to_owned();
  let events: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../../../fixtures/real_task_progress.events.json")).unwrap();
  let progress = events
    .iter()
    .rev()
    .filter_map(|event| event.pointer("/body/frame"))
    .find(|frame| frame["type"] == "tool_execution_update" && frame["toolName"] == "task")
    .unwrap()["partialResult"]
    .clone();
  let start = events
    .iter()
    .filter_map(|event| event.pointer("/body/frame"))
    .find(|frame| frame["type"] == "tool_execution_start" && frame["toolName"] == "task")
    .unwrap();
  let mut input = ToolInput {
    name: "task".into(),
    intent: None,
    args: Some(start["args"].clone()),
    result: Some(result.clone()),
    delivery: None,
    task_progress: Some(progress.clone()),
    stream: Vec::new(),
    running: false,
    failed: false,
    exit_code: None,
    timestamp: None,
  };
  let (handle, block) = cx
    .update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| ToolBlock::new("task-progress-retention".into(), input.clone(), cx))
      })
    })
    .unwrap();
  input.task_progress = None;
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  input.delivery = Some(delivery.clone());
  cx.update(|cx| block.update(cx, |block, cx| block.sync(&input, cx)));
  cx.update_window(handle, |_, window, cx| {
    window.render_frame(cx);
    window.click("task-progress-retention-copy", cx);
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(expected));
    window.click("task-progress-retention-raw-toggle", cx);
    window.click("task-progress-retention-copy", cx);
    let raw = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
    let raw: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw["result"], result);
    assert_eq!(raw["taskProgress"], progress);
    assert_eq!(raw["delivery"], delivery);
  })
  .unwrap();
}
