use alvin_conversation::{Level, Notice, Origin};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
  StatefulInteractiveElement as _, Styled as _, Window, div, px, rems,
};

use super::transcript::{SystemActivity, SystemActivityData};

/// Keeps routine activity quiet and urgent notifications brief; full content remains in the activity list.
pub(crate) fn system_activity_notification(notice: &Notice) -> Option<Notification> {
  match notice.level {
    Some(Level::Warning) => Some(Notification::warning(compact_text(&notice.text)).title(SharedString::from("System activity"))),
    Some(Level::Error) => Some(Notification::error(compact_text(&notice.text)).title(SharedString::from("System activity"))),
    Some(Level::Info) | None => None,
  }
}

fn compact_text(text: &str) -> String {
  let mut chars = text
    .split_whitespace()
    .enumerate()
    .flat_map(|(index, word)| (index > 0).then_some(' ').into_iter().chain(word.chars()));
  let mut summary: String = chars.by_ref().take(160).collect();
  if chars.next().is_some() {
    summary.push('…');
  }
  summary
}

struct Entry {
  source: Notice,
  title: String,
  summary: String,
  details: String,
  raw: Option<String>,
}

impl Entry {
  fn new(source: Notice) -> Self {
    let origin = match &source.origin {
      Origin::Notice(Some(source)) => format!("Notice · {source}"),
      Origin::Notice(None) => "Notice".to_owned(),
      Origin::CommandOutput => "Command output".to_owned(),
      Origin::Extension => "Extension".to_owned(),
      Origin::ExtensionError { path, event } => format!("Extension · {path} · {event}"),
      Origin::Run => "Run".to_owned(),
      Origin::FrameDropped(Some(kind)) => format!("Frame dropped · {kind}"),
      Origin::FrameDropped(None) => "Frame dropped".to_owned(),
      Origin::Undecodable => "Undecodable frame".to_owned(),
      Origin::TtsrTriggered(_) => "TTSR · Rules triggered".to_owned(),
      Origin::TodoReminder(_) => "Todo reminder".to_owned(),
    };
    let title = match source.level {
      Some(Level::Info) => format!("Info · {origin}"),
      Some(Level::Warning) => format!("Warning · {origin}"),
      Some(Level::Error) => format!("Error · {origin}"),
      None => origin,
    };
    let mut raw = source
      .raw
      .as_ref()
      .map(|value| serde_json::to_string_pretty(value).unwrap_or_else(|error| format!("Could not serialize frame: {error}")));
    let summary = match &source.origin {
      Origin::TtsrTriggered(rules) => compact_text(
        &rules
          .iter()
          .map(|rule| {
            let name = rule.get("name").and_then(|value| value.as_str()).unwrap_or("Unnamed rule");
            match rule.get("path").and_then(|value| value.as_str()) {
              Some(path) => format!("{name} ({path})"),
              None => name.to_owned(),
            }
          })
          .collect::<Vec<_>>()
          .join(", "),
      ),
      _ => compact_text(&source.text),
    };
    let details = match &source.origin {
      Origin::TtsrTriggered(rules) => serde_json::to_string_pretty(&serde_json::json!({"type": "ttsr_triggered", "rules": rules}))
        .unwrap_or_else(|error| format!("Could not serialize rules: {error}")),
      Origin::TodoReminder(reminder) => {
        let details = serde_json::to_string_pretty(&serde_json::json!({
          "type": "todo_reminder",
          "todos": reminder.todos,
          "attempt": reminder.attempt,
          "maxAttempts": reminder.max_attempts,
        }))
        .unwrap_or_else(|error| format!("Could not serialize reminder: {error}"));
        raw = Some(details.clone());
        details
      }
      _ => source.text.clone(),
    };
    Self {
      title,
      summary,
      details,
      source,
      raw,
    }
  }
}

impl Entry {
  fn activity(&self) -> SystemActivityData {
    let body = match &self.source.origin {
      Origin::TtsrTriggered(_) | Origin::TodoReminder(_) => super::transcript::fenced(&self.details),
      _ => self.details.clone(),
    };
    SystemActivityData {
      title: self.title.clone(),
      summary: self.summary.clone(),
      body,
      copy: self.details.clone(),
      raw: self.raw.clone(),
      level: self.source.level,
    }
  }
}

pub(super) struct NoticeList {
  entries: Vec<(Entry, Entity<SystemActivity>)>,
  open: bool,
  header: SharedString,
  highlight: Option<(Level, SharedString)>,
  scroll: ScrollHandle,
}

impl NoticeList {
  pub(super) fn new() -> Self {
    Self {
      entries: Vec::new(),
      open: false,
      header: "System activity".into(),
      highlight: None,
      scroll: ScrollHandle::new(),
    }
  }

  pub(super) fn sync(&mut self, notices: &[Notice], cx: &mut Context<Self>) {
    if self.entries.len() == notices.len() && self.entries.iter().zip(notices).all(|((entry, _), notice)| &entry.source == notice) {
      return;
    }
    let common = self
      .entries
      .iter()
      .zip(notices)
      .take_while(|((entry, _), notice)| &entry.source == *notice)
      .count();
    self.entries.truncate(common);
    for (index, notice) in notices.iter().enumerate().skip(common) {
      let entry = Entry::new(notice.clone());
      let data = entry.activity();
      let row = cx.new(|cx| SystemActivity::new_activity(format!("notice-{index}").into(), data, cx));
      self.entries.push((entry, row));
    }
    self.header = format!("System activity · {}", self.entries.len()).into();
    self.highlight = [Level::Error, Level::Warning].into_iter().find_map(|level| {
      self
        .entries
        .iter()
        .rev()
        .find(|(entry, _)| entry.source.level == Some(level))
        .map(|(entry, _)| (level, SharedString::from(format!("{}: {}", entry.title, entry.summary))))
    });
    cx.notify();
  }
}

impl Render for NoticeList {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if self.entries.is_empty() {
      return div().into_any_element();
    }
    let focus = super::focus::tab_stop((gpui_kit::ElementId::View(cx.entity_id()), "activity-focus"), window, cx);
    let scroll = self.scroll.clone();
    let open = self.open;
    v_flex()
      .id("session-notices")
      .test_support()
      .role(gpui_kit::Role::Group)
      .aria_label("System activity")
      .w_full()
      .min_w_0()
      .on_key_down(move |event, window, cx| {
        if !open {
          return;
        }
        let mut offset = scroll.offset();
        let step = window.rem_size() * 3.;
        match event.keystroke.key.as_str() {
          "down" => offset.y = (offset.y - step).max(-scroll.max_offset().y),
          "up" => offset.y = (offset.y + step).min(px(0.)),
          "home" => offset.y = px(0.),
          "end" => offset.y = -scroll.max_offset().y,
          _ => return,
        }
        scroll.set_offset(offset);
        window.refresh();
        cx.stop_propagation();
      })
      .child(
        Collapsible::new()
          .open(self.open)
          .child(
            h_flex()
              .w_full()
              .min_w_0()
              .gap_2()
              .child(
                Button::new("system-activity-toggle")
                  .xsmall()
                  .ghost()
                  .track_focus(&focus)
                  .toggled(self.open)
                  .icon(if self.open { IconName::ChevronDown } else { IconName::ChevronRight })
                  .label(self.header.clone())
                  .on_click(cx.listener(|this, _, _, cx| {
                    this.open = !this.open;
                    cx.notify();
                  })),
              )
              .when_some(self.highlight.as_ref(), |header, (level, summary)| {
                header.child(
                  div()
                    .id("system-activity-highlight")
                    .test_support()
                    .role(gpui_kit::Role::Status)
                    .aria_label(summary.clone())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(match level {
                      Level::Error => cx.theme().danger,
                      Level::Warning => cx.theme().warning,
                      Level::Info => cx.theme().muted_foreground,
                    })
                    .child(summary.clone()),
                )
              }),
          )
          .content(
            v_flex()
              .id("system-activity-details")
              .test_support()
              .w_full()
              .min_w_0()
              .max_h(rems(12.))
              .track_scroll(&self.scroll)
              .overflow_y_scroll()
              .gap_2()
              .children(self.entries.iter().map(|(_, row)| row.clone())),
          ),
      )
      .into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use super::Entry;
  use alvin_conversation::{Level, Notice, Origin};
  use gpui_kit::component::{button::Button, v_flex};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render, ScrollDelta, Styled as _, TestAppContext,
    Window, WindowOptions, point, px,
  };
  use serde_json::json;

  #[test]
  fn routine_activity_is_retained_without_interrupting_with_a_popup() {
    let text = format!("Mounted extensions:\n{}", "large extension payload\n".repeat(100));
    for level in [None, Some(Level::Info)] {
      let notice = Notice {
        level,
        origin: Origin::Extension,
        text: text.clone(),
        raw: None,
      };
      assert!(super::system_activity_notification(&notice).is_none());
      assert_eq!(Entry::new(notice).details, text);
    }
  }

  struct KeyboardFixture {
    before: FocusHandle,
    list: Entity<super::NoticeList>,
  }

  impl Render for KeyboardFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex()
        .w_full()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .child(self.list.clone())
    }
  }

  #[test]
  fn warning_keeps_source_and_original_frame() {
    let entry = Entry::new(Notice {
      level: Some(Level::Warning),
      origin: Origin::Undecodable,
      text: "bad frame".into(),
      raw: Some(json!({"type":"future", "payload": [1, 2]})),
    });
    assert_eq!(entry.title, "Warning · Undecodable frame");
    assert_eq!(entry.source.text, "bad frame");
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(entry.raw.as_ref().unwrap()).unwrap(),
      json!({"type":"future", "payload": [1, 2]})
    );
  }

  #[test]
  fn information_keeps_notice_provenance() {
    let entry = Entry::new(Notice {
      level: Some(Level::Info),
      origin: Origin::Notice(Some("extension".into())),
      text: "ready".into(),
      raw: None,
    });
    assert_eq!(entry.title, "Info · Notice · extension");
    assert_eq!(entry.source.text, "ready");
    assert!(entry.raw.is_none());
  }

  #[test]
  fn status_without_severity_stays_neutral() {
    let entry = Entry::new(Notice {
      level: None,
      origin: Origin::Run,
      text: "Fallback succeeded on backup".into(),
      raw: None,
    });
    assert_eq!(entry.title, "Run");
    assert_eq!(entry.activity().level, None);
    assert_eq!(entry.summary, "Fallback succeeded on backup");
  }

  #[test]
  fn rules_show_citations_without_exposing_content_in_summary() {
    let rules = json!([{"name":"safety","path":"/rules/safety.md","content":"private rule body","extra":{"future":true}}]);
    let entry = Entry::new(Notice {
      level: Some(Level::Warning),
      origin: Origin::TtsrTriggered(serde_json::from_value(rules.clone()).unwrap()),
      text: "Rules triggered".into(),
      raw: None,
    });
    assert_eq!(entry.title, "Warning · TTSR · Rules triggered");
    assert_eq!(entry.summary, "safety (/rules/safety.md)");
    assert!(!entry.summary.contains("private rule body"));
    assert_eq!(
      serde_json::from_str::<serde_json::Value>(&entry.details).unwrap(),
      json!({"type":"ttsr_triggered","rules":rules})
    );
  }

  #[test]
  fn ordinary_notice_summary_is_bounded_but_details_are_complete() {
    let text = format!("start\n{}", "é".repeat(200));
    let entry = Entry::new(Notice {
      level: Some(Level::Info),
      origin: Origin::Notice(None),
      text: text.clone(),
      raw: None,
    });
    assert_eq!(entry.summary.chars().count(), 161);
    assert!(entry.summary.ends_with('…'));
    assert_eq!(entry.details, text);
  }
  #[test]
  fn recorded_info_notice_keeps_source_and_message() {
    let events: Vec<alvin_protocol::Event> =
      serde_json::from_str(include_str!("../../fixtures/real_mixed_read_stream.events.json")).unwrap();
    let notice = events
      .iter()
      .find_map(|event| match &event.body {
        alvin_protocol::EventBody::Agent(alvin_protocol::RpcAgentEvent::Notice(notice)) if notice.source.as_deref() == Some("xdev") => {
          Some(notice)
        }
        _ => None,
      })
      .unwrap();
    let entry = Entry::new(Notice {
      level: Some(notice.level),
      origin: Origin::Notice(notice.source.clone()),
      text: notice.message.clone(),
      raw: None,
    });
    assert_eq!(entry.title, "Info · Notice · xdev");
    assert!(entry.source.text.starts_with("xd://: mounted "));
  }
  #[gpui_kit::test]
  fn recorded_reminder_is_quiet_but_fully_inspectable(cx: &mut TestAppContext) {
    let events: Vec<alvin_protocol::Event> = serde_json::from_str(include_str!(
      "../../../alvin-conversation/fixtures/real_todo_reminder_repeated.events.json"
    ))
    .unwrap();
    let event = events
      .iter()
      .find(|event| {
        matches!(
          &event.body,
          alvin_protocol::EventBody::Agent(alvin_protocol::RpcAgentEvent::TodoReminder(_))
        )
      })
      .unwrap();
    let expected = serde_json::to_value(event).unwrap()["body"]["frame"].clone();
    let mut conversation = alvin_conversation::Conversation::default();
    conversation.open(&alvin_protocol::LiveSessionSummary {
      live: event.live,
      cwd: "/repo".into(),
      file: "/session".into(),
      exited: false,
    });
    conversation.apply(event);
    let view = conversation.live(event.live).unwrap();
    assert!(view.todo_phases.is_empty());
    for notice in &view.notices {
      assert!(super::system_activity_notification(notice).is_none());
    }
    cx.update(gpui_kit::init);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          let list = cx.new(|cx| {
            let mut list = super::NoticeList::new();
            list.sync(&view.notices, cx);
            list
          });
          KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            list,
          }
        })
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("system-activity-toggle").checked(), Some(false));
      assert!(window.try_find("notice-0-details").is_none());
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.press("tab", cx);
      window.press("enter", cx);
      window.press("tab", cx);
      window.press("enter", cx);
      assert!(window.find("notice-0-body-container").visible());
      window.press("tab", cx);
      window.press("enter", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(serde_json::from_str::<serde_json::Value>(&copied).unwrap(), expected);
      let viewport = window.find("system-activity-details").bounds();
      let target = window.find("notice-0-copy-raw").bounds();
      window.scroll(
        "system-activity-details",
        ScrollDelta::Pixels(point(px(0.), viewport.center().y - target.center().y)),
        cx,
      );
      window.click("notice-0-copy-raw", cx);
      let raw = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(serde_json::from_str::<serde_json::Value>(&raw).unwrap(), expected);
    })
    .expect("inspect reminder");
  }

  #[gpui_kit::test]
  fn raw_frame_disclosure_and_copy_work(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          let list = cx.new(|cx| {
            let mut list = super::NoticeList::new();
            list.sync(
              &[Notice {
                level: Some(Level::Warning),
                origin: Origin::Undecodable,
                text: "bad frame".into(),
                raw: Some(json!({"type":"future"})),
              }],
              cx,
            );
            list
          });
          KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            list,
          }
        })
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      window.press("tab", cx);
      assert_eq!(window.find("system-activity-toggle").focused(), Some(true));
      assert!(window.try_find("notice-0-details").is_none());
      window.press("enter", cx);
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-details").focused(), Some(true));
      window.press("enter", cx);
      assert!(window.find("notice-0-body-container").visible());
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-copy").focused(), Some(true));
      window.press("enter", cx);
      assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("bad frame"));
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-copy-raw").focused(), Some(true));
      window.press("enter", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap(),
        json!({"type":"future"})
      );
      window.click("notice-0-copy-raw", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap(),
        json!({"type":"future"})
      );
      window.click("notice-0-details", cx);
      assert!(window.try_find("notice-0-body-container").is_none());
    })
    .expect("interact with notice");
  }

  #[gpui_kit::test]
  fn rule_details_remain_open_after_append_and_copy_complete_rules(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let rules = json!([{"name":"safety","path":"/rules/safety.md","content":"full body","future":7}]);
    let notice = Notice {
      level: Some(Level::Warning),
      origin: Origin::TtsrTriggered(serde_json::from_value(rules.clone()).unwrap()),
      text: "Rules triggered".into(),
      raw: None,
    };
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          let list = cx.new(|cx| {
            let mut list = super::NoticeList::new();
            list.sync(std::slice::from_ref(&notice), cx);
            list
          });
          KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            list,
          }
        })
      })
      .expect("open window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      let list = fixture.read(cx).list.clone();
      window.press("tab", cx);
      assert_eq!(window.find("system-activity-toggle").focused(), Some(true));
      window.press("enter", cx);
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-details").focused(), Some(true));
      window.press("enter", cx);
      assert!(window.find("notice-0-body-container").visible());
      list.update(cx, |list, cx| list.sync(&[notice.clone(), notice.clone()], cx));
      window.render_frame(cx);
      assert!(window.find("notice-0-body-container").visible());
      window.press("shift-tab", cx);
      assert_eq!(window.find("system-activity-toggle").focused(), Some(true));
      window.press("enter", cx);
      assert!(window.try_find("notice-0-body-container").is_none());
      window.press("enter", cx);
      assert!(window.find("notice-0-body-container").visible());
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-details").focused(), Some(true));
      window.press("tab", cx);
      assert_eq!(window.find("notice-0-copy").focused(), Some(true));
      window.press("enter", cx);
      let copied = cx.read_from_clipboard().and_then(|item| item.text()).unwrap();
      assert_eq!(
        serde_json::from_str::<serde_json::Value>(&copied).unwrap(),
        json!({"type":"ttsr_triggered","rules":rules})
      );
    })
    .expect("interact with rules");
  }

  #[gpui_kit::test]
  fn activity_volume_does_not_take_transcript_space_until_opened(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let mut notices = vec![Notice {
      level: Some(Level::Info),
      origin: Origin::Extension,
      text: "Extension 0 loaded".into(),
      raw: None,
    }];
    let (handle, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          let list = cx.new(|cx| {
            let mut list = super::NoticeList::new();
            list.sync(&notices, cx);
            list
          });
          KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            list,
          }
        })
      })
      .unwrap()
    });
    let list = cx.update(|cx| fixture.read(cx).list.clone());
    let collapsed_height = cx
      .update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("system-activity-details").is_none());
        assert!(window.try_find("system-activity-highlight").is_none());
        window.find("session-notices").bounds().size.height
      })
      .unwrap();
    notices.extend((1..40).map(|index| Notice {
      level: Some(Level::Info),
      origin: Origin::Extension,
      text: format!("Extension {index} loaded"),
      raw: None,
    }));
    let warning = "Action needs attention. ".repeat(20);
    notices.push(Notice {
      level: Some(Level::Warning),
      origin: Origin::Notice(Some("upstream".into())),
      text: warning.clone(),
      raw: None,
    });
    cx.update(|cx| list.update(cx, |list, cx| list.sync(&notices, cx)));
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find("session-notices").bounds().size.height, collapsed_height);
      assert_eq!(window.find("system-activity-toggle").label(), Some("System activity · 41"));
      let region = window.find("session-notices").bounds();
      let highlight = window.find("system-activity-highlight").bounds();
      assert!(highlight.left() >= region.left());
      assert!(highlight.right() <= region.right());
      assert!(window.try_find("notice-0-details").is_none());
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      window.press("tab", cx);
      assert_eq!(window.find("system-activity-toggle").focused(), Some(true));
      window.press("enter", cx);
      let details = window.find("system-activity-details").bounds();
      assert!(details.size.height <= window.rem_size() * 12.);
      assert!(window.find("notice-0-details").visible());
      window.press("end", cx);
      window.render_frame(cx);
      let last = window.find("notice-40-details").bounds();
      assert!(last.top() >= details.top());
      assert!(last.bottom() <= details.bottom());
      window.click("notice-40-copy", cx);
      assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
        Some(warning.as_str())
      );
    })
    .unwrap();
  }
}
