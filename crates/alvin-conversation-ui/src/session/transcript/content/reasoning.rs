use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::motion::{MotionReveal, spring};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, v_flex};
use gpui_kit::{
  AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
  prelude::FluentBuilder as _,
};

pub(super) struct Reasoning {
  id: SharedString,
  text: SharedString,
  open: bool,
}

impl Reasoning {
  pub(super) fn new(id: SharedString, text: &str) -> Self {
    Self {
      id,
      text: SharedString::from(text.to_owned()),
      open: false,
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
    let progress = spring(
      (ElementId::View(cx.entity_id()), "disclosure"),
      if self.open { 1.0 } else { 0.0 },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    let toggle = Button::new(SharedString::from(format!("{}-toggle", self.id)))
      .ghost()
      .xsmall()
      .w_full()
      .justify_start()
      .text_color(cx.theme().foreground)
      .icon(if self.open { IconName::ChevronDown } else { IconName::ChevronRight })
      .label(if self.open { "Hide reasoning" } else { "Show reasoning" })
      .toggled(self.open)
      .on_click(cx.listener(|this, _, _, cx| {
        this.open = !this.open;
        cx.notify();
      }));
    let details = div()
      .id(SharedString::from(format!("{}-details", self.id)))
      .test_support()
      .w_full()
      .min_w_0()
      .p_2()
      .rounded_md()
      .border_1()
      .border_color(cx.theme().info.opacity(0.25))
      .bg(cx.theme().info.opacity(0.06))
      .text_color(cx.theme().foreground)
      .text_sm()
      .child(Icon::new(IconName::Info).text_color(cx.theme().info))
      .child(self.text.clone())
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
      .when(self.open || progress > 0.0, |body| body.child(details))
  }
}

#[cfg(test)]
mod tests {
  use gpui_kit::component::v_flex;
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TestAppContext, Window,
    WindowOptions,
  };

  use super::Reasoning;

  struct Fixture {
    first: Entity<Reasoning>,
    second: Entity<Reasoning>,
  }

  impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
      v_flex().w_full().child(self.first.clone()).child(self.second.clone())
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
          first: cx.new(|_| Reasoning::new(SharedString::from("first"), "First")),
          second: cx.new(|_| Reasoning::new(SharedString::from("second"), "Second")),
        })
      })
      .expect("open test window")
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("first-details").is_none());
      assert!(window.try_find("second-details").is_none());
      window.click("first-toggle", cx);
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
      window.click("first-toggle", cx);
      assert!(window.find("first-details").visible());
    })
    .expect("update test window");
  }
}
