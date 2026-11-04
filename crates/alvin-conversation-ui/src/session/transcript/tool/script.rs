use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, v_flex};
use gpui_kit::{
  Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};

use super::code;

const SOURCE_LINES: usize = 4;
const OUTPUT_LINES: usize = 12;

pub(super) struct ScriptPanel {
  id: SharedString,
  source: String,
  language: String,
  output: String,
  source_expanded: bool,
  output_expanded: bool,
}

impl ScriptPanel {
  pub(super) fn new(id: SharedString, source: String, language: String, output: String, _: &mut Context<Self>) -> Self {
    Self {
      id,
      source,
      language,
      output,
      source_expanded: false,
      output_expanded: false,
    }
  }

  pub(super) fn sync(&mut self, source: &str, language: &str, output: &str, cx: &mut Context<Self>) {
    if self.source != source || self.language != language || self.output != output {
      self.source = source.to_owned();
      self.language = language.to_owned();
      self.output = output.to_owned();
      cx.notify();
    }
  }

  fn source_preview(&self) -> (&str, usize) {
    let count = self.source.split_terminator('\n').count();
    let hidden = count.saturating_sub(SOURCE_LINES);
    if self.source_expanded || hidden == 0 {
      return (&self.source, hidden);
    }
    let end = self
      .source
      .match_indices('\n')
      .nth(SOURCE_LINES - 1)
      .map_or(self.source.len(), |(ix, _)| ix + 1);
    (self.source.get(..end).unwrap_or_default(), hidden)
  }

  fn output_preview(&self) -> (&str, usize) {
    let count = self.output.split_terminator('\n').count();
    let hidden = count.saturating_sub(OUTPUT_LINES);
    if self.output_expanded || hidden == 0 {
      return (&self.output, hidden);
    }
    let without_final_newline = self.output.strip_suffix('\n').unwrap_or(&self.output);
    let start = without_final_newline
      .rmatch_indices('\n')
      .nth(OUTPUT_LINES - 1)
      .map_or(0, |(ix, _)| ix + 1);
    (self.output.get(start..).unwrap_or_default(), hidden)
  }
}

impl Render for ScriptPanel {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let (source, source_hidden) = self.source_preview();
    let (output, output_hidden) = self.output_preview();
    let source_view = code::view(SharedString::from(format!("{}-source-code", self.id)), source, &self.language, cx);
    let output_view = code::view(SharedString::from(format!("{}-output-code", self.id)), output, "text", cx);
    let source_focus = crate::session::focus::tab_stop((ElementId::View(cx.entity_id()), "source-focus"), window, cx);
    let output_focus = crate::session::focus::tab_stop((ElementId::View(cx.entity_id()), "output-focus"), window, cx);
    let source_control = (source_hidden > 0).then(|| {
      Button::new(SharedString::from(format!("{}-source-extent", self.id)))
        .ghost()
        .xsmall()
        .track_focus(&source_focus)
        .text_color(cx.theme().muted_foreground)
        .icon(if self.source_expanded {
          IconName::ChevronUp
        } else {
          IconName::ChevronDown
        })
        .toggled(self.source_expanded)
        .accessibility_label(if self.source_expanded {
          "Collapse script source"
        } else {
          "Expand script source"
        })
        .label(if self.source_expanded {
          "Show less".to_owned()
        } else {
          format!("{source_hidden} more lines")
        })
        .on_click(cx.listener(|this, _, _, cx| {
          this.source_expanded = !this.source_expanded;
          cx.notify();
        }))
    });
    let output_control = (output_hidden > 0).then(|| {
      Button::new(SharedString::from(format!("{}-output-extent", self.id)))
        .ghost()
        .xsmall()
        .track_focus(&output_focus)
        .text_color(cx.theme().muted_foreground)
        .icon(if self.output_expanded {
          IconName::ChevronDown
        } else {
          IconName::ChevronUp
        })
        .toggled(self.output_expanded)
        .accessibility_label(if self.output_expanded {
          "Collapse script output"
        } else {
          "Expand script output"
        })
        .label(if self.output_expanded {
          "Show less".to_owned()
        } else {
          format!("{output_hidden} earlier lines")
        })
        .on_click(cx.listener(|this, _, _, cx| {
          this.output_expanded = !this.output_expanded;
          cx.notify();
        }))
    });
    v_flex()
      .w_full()
      .min_w_0()
      .child(
        v_flex()
          .id(SharedString::from(format!("{}-source", self.id)))
          .test_support()
          .w_full()
          .min_w_0()
          .items_start()
          .p_3()
          .gap_2()
          .child(
            div()
              .id(SharedString::from(format!("{}-source-text", self.id)))
              .test_support()
              .w_full()
              .min_w_0()
              .child(source_view),
          )
          .children(source_control),
      )
      .child(
        v_flex()
          .id(SharedString::from(format!("{}-output", self.id)))
          .test_support()
          .w_full()
          .min_w_0()
          .items_start()
          .border_t_1()
          .border_color(cx.theme().border)
          .p_3()
          .gap_2()
          .children(output_control)
          .child(
            div()
              .id(SharedString::from(format!("{}-output-text", self.id)))
              .test_support()
              .w_full()
              .min_w_0()
              .child(output_view),
          ),
      )
  }
}

#[cfg(test)]
mod tests {
  use gpui_kit::component::{button::Button, v_flex};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{
    AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TestAppContext,
    Window, WindowOptions,
  };

  use super::ScriptPanel;

  struct KeyboardFixture {
    before: FocusHandle,
    panel: Entity<ScriptPanel>,
  }

  impl Render for KeyboardFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex()
        .w_full()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .child(self.panel.clone())
    }
  }

  #[gpui_kit::test]
  fn previews_preserve_unicode_blank_lines_and_final_newlines(cx: &mut TestAppContext) {
    cx.update(|cx| {
      let panel = cx.new(|cx| {
        ScriptPanel::new(
          "script".into(),
          "α\n\n三\n四\n五\n".into(),
          "text".into(),
          "零\n一\n二\n三\n四\n五\n六\n七\n八\n九\n十\n十一\n\n".into(),
          cx,
        )
      });
      let panel = panel.read(cx);
      assert_eq!(panel.source_preview(), ("α\n\n三\n四\n", 1));
      assert_eq!(panel.output_preview(), ("一\n二\n三\n四\n五\n六\n七\n八\n九\n十\n十一\n\n", 1));
    });
  }

  #[gpui_kit::test]
  fn disclosures_are_independent_and_survive_sync(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| {
          let panel = cx.new(|cx| {
            ScriptPanel::new(
              SharedString::from("script"),
              "a\nb\nc\nd\ne".into(),
              "text".into(),
              "1\n".repeat(13),
              cx,
            )
          });
          KeyboardFixture {
            before: cx.focus_handle().tab_stop(true),
            panel,
          }
        })
      })
      .expect("open test window")
    });
    let panel = cx.update(|cx| fixture.read(cx).panel.clone());
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      window.press("tab", cx);
      assert_eq!(window.find("script-source-extent").focused(), Some(true));
      window.press("enter", cx);
      assert_eq!(panel.read(cx).source_preview().0, "a\nb\nc\nd\ne");
      assert_eq!(panel.read(cx).output_preview().1, 1);
      assert_eq!(panel.read(cx).output_preview().0, "1\n".repeat(12));
      panel.update(cx, |panel, cx| panel.sync("α\nβ\nγ\nδ\nε", "text", &"2\n".repeat(14), cx));
      window.render_frame(cx);
      assert_eq!(panel.read(cx).source_preview().0, "α\nβ\nγ\nδ\nε");
      assert_eq!(panel.read(cx).output_preview().0, "2\n".repeat(12));
      window.press("tab", cx);
      assert_eq!(window.find("script-output-extent").focused(), Some(true));
      window.press("enter", cx);
      assert_eq!(panel.read(cx).output_preview().0, "2\n".repeat(14));
      window.click("script-source-extent", cx);
      assert_eq!(panel.read(cx).source_preview().0, "α\nβ\nγ\nδ\n");
      assert_eq!(panel.read(cx).output_preview().0, "2\n".repeat(14));
      window.click("script-output-extent", cx);
      assert_eq!(panel.read(cx).output_preview().0, "2\n".repeat(12));
    })
    .expect("update test window");
  }
}
