use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::motion::{MotionReveal, spring};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, text::TextView, v_flex};
use gpui_kit::{
  AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
  prelude::FluentBuilder as _,
};

pub(super) struct Reasoning {
  id: SharedString,
  text: SharedString,
  open: Option<bool>,
}

impl Reasoning {
  pub(super) fn new(id: SharedString, text: &str, cx: &mut Context<Self>) -> Self {
    cx.observe_global::<crate::ConversationPreferences>(|_, cx| cx.notify()).detach();
    Self {
      id,
      text: SharedString::from(text.to_owned()),
      open: None,
    }
  }

  pub(super) fn sync(&mut self, text: &str, cx: &mut Context<Self>) {
    if self.text.as_ref() != text {
      self.text = SharedString::from(text.to_owned());
      cx.notify();
    }
  }

  pub(super) fn text(&self) -> &str {
    self.text.as_ref()
  }
}

impl Render for Reasoning {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let open = self.open.unwrap_or(crate::ConversationPreferences::current(cx).expand_thinking);
    let progress = spring(
      (ElementId::View(cx.entity_id()), "disclosure"),
      if open { 1.0 } else { 0.0 },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    let focus = window
      .use_keyed_state((ElementId::View(cx.entity_id()), "toggle-focus"), cx, |_, cx| {
        cx.focus_handle().tab_stop(true)
      })
      .read(cx)
      .clone();
    let label = if open { "Hide reasoning" } else { "Show reasoning" };
    let toggle = Button::new(SharedString::from(format!("{}-toggle", self.id)))
      .ghost()
      .xsmall()
      .w_full()
      .text_color(cx.theme().foreground)
      .track_focus(&focus)
      .accessibility_label(label)
      .child(
        h_flex()
          .id(SharedString::from(format!("{}-label", self.id)))
          .test_support()
          .w_full()
          .gap_1()
          .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall())
          .child(
            div()
              .id(SharedString::from(format!("{}-label-text", self.id)))
              .test_support()
              .child(label),
          ),
      )
      .toggled(open)
      .on_click(cx.listener(move |this, _, _, cx| {
        this.open = Some(!open);
        cx.notify();
      }));
    let details = div()
      .id(SharedString::from(format!("{}-details", self.id)))
      .test_support()
      .w_full()
      .min_w_0()
      .pl_5()
      .child(
        TextView::markdown(SharedString::from(format!("{}-markdown", self.id)), self.text.clone())
          .text_sm()
          .text_color(cx.theme().muted_foreground),
      )
      .into_any_element();
    let details: AnyElement = if cx.reduce_motion() {
      details
    } else {
      MotionReveal::new(
        ElementId::NamedChild(ElementId::View(cx.entity_id()).into(), "reveal".into()),
        progress,
        details,
      )
      .into_any_element()
    };
    v_flex()
      .w_full()
      .min_w_0()
      .child(toggle)
      .when(open || progress > 0.0, |body| body.child(details))
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

  use super::Reasoning;

  struct Fixture {
    before: FocusHandle,
    first: Entity<Reasoning>,
    second: Entity<Reasoning>,
  }

  impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex()
        .w_full()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .child(self.first.clone())
        .child(self.second.clone())
    }
  }

  #[gpui_kit::test]
  fn disclosures_remain_independent_when_reasoning_updates(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let (window, fixture) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| Fixture {
          before: cx.focus_handle().tab_stop(true),
          first: cx.new(|cx| {
            Reasoning::new(
              SharedString::from("first"),
              "A paragraph with **emphasis**.\n\n- one\n- two\n\n```rust\nlet value = 1;\n```",
              cx,
            )
          }),
          second: cx.new(|cx| Reasoning::new(SharedString::from("second"), "Second", cx)),
        })
      })
      .expect("open test window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let before = fixture.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      assert!(window.try_find("first-details").is_none());
      assert!(window.try_find("second-details").is_none());
      window.press("tab", cx);
      assert_eq!(window.find("first-toggle").focused(), Some(true));
      window.press("enter", cx);
      let details = window.find("first-details").bounds();
      let label = window.find("first-label-text").bounds();
      let toggle = window.find("first-toggle").bounds();
      assert_eq!(details.left() + gpui_kit::rems(1.25).to_pixels(window.rem_size()), label.left());
      assert!(label.left() < toggle.left() + toggle.size.width / 2.);
      assert!(details.size.height > toggle.size.height * 3.);
      assert!(window.find("first-details").visible());
      assert!(window.try_find("second-details").is_none());
      let first = fixture.read(cx).first.clone();
      first.update(cx, |reasoning, cx| reasoning.sync("First grows while streaming", cx));
      window.render_frame(cx);
      assert!(window.find("first-details").visible());
      window.click("second-toggle", cx);
      assert!(window.find("first-details").visible());
      assert!(window.find("second-details").visible());
      window.click("first-toggle", cx);
      assert!(window.try_find("first-details").is_none());
      assert!(window.find("second-details").visible());
      first.update(cx, |reasoning, cx| reasoning.sync("First grows while streaming again", cx));
      window.render_frame(cx);
      assert!(window.try_find("first-details").is_none());
      assert!(window.find("second-details").visible());
      window.click("first-toggle", cx);
      assert!(window.find("first-details").visible());
    })
    .expect("update test window");
  }
  #[gpui_kit::test]
  fn defaults_change_until_reasoning_is_manually_closed(cx: &mut TestAppContext) {
    cx.update(|cx| {
      gpui_kit::init(cx);
      cx.set_reduce_motion(true);
    });
    let (window, reasoning) = cx
      .update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
          cx.new(|cx| Reasoning::new("thinking".into(), "streaming", cx))
        })
      })
      .unwrap();
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("thinking-details").is_none());
    })
    .unwrap();
    cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(true, false, true)));
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.find("thinking-details").visible());
      window.click("thinking-toggle", cx);
      assert!(window.try_find("thinking-details").is_none());
      reasoning.update(cx, |reasoning, cx| reasoning.sync("streaming again", cx));
    })
    .unwrap();
    cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(false, true, false)));
    cx.update(|cx| cx.set_global(crate::ConversationPreferences::new(true, false, true)));
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("thinking-details").is_none());
    })
    .unwrap();
  }
}
