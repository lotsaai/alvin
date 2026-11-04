//! Current plan, independent of historical tool results.

use alvin_protocol::{TodoPhase, TodoStatus};
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  App, ClickEvent, FocusHandle, HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
  StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, Window, div, px, rems,
};

fn status_name(status: TodoStatus) -> &'static str {
  match status {
    TodoStatus::Pending => "Pending",
    TodoStatus::InProgress => "In progress",
    TodoStatus::Completed => "Completed",
    TodoStatus::Abandoned => "Abandoned",
    TodoStatus::Blocked => "Blocked",
  }
}

fn details_label(phases: &[TodoPhase]) -> String {
  let mut label = String::new();
  for phase in phases {
    let done = phase.tasks.iter().filter(|task| task.status == TodoStatus::Completed).count();
    label.push_str(&format!("{} · {done}/{}\n", phase.name, phase.tasks.len()));
    for task in &phase.tasks {
      label.push_str(&format!("{} · {}", status_name(task.status), task.content));
      if task.status == TodoStatus::Blocked
        && let Some(blocker) = &task.blocker
      {
        label.push_str(&format!(" · Blocked: {blocker}"));
      }
      label.push('\n');
    }
  }
  label
}

pub(super) fn view(
  phases: &[TodoPhase],
  open: bool,
  on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
  focus: &FocusHandle,
  cx: &App,
) -> Option<impl IntoElement> {
  if phases.is_empty() {
    return None;
  }
  let total: usize = phases.iter().map(|phase| phase.tasks.len()).sum();
  let completed = phases
    .iter()
    .flat_map(|phase| &phase.tasks)
    .filter(|task| task.status == TodoStatus::Completed)
    .count();
  let activity = phases
    .iter()
    .flat_map(|phase| &phase.tasks)
    .find(|task| task.status == TodoStatus::InProgress);
  let summary: SharedString = match activity {
    Some(task) => format!("Plan · {completed}/{total} completed · {}", task.content).into(),
    None => format!("Plan · {completed}/{total} completed").into(),
  };
  let details = v_flex()
    .id("current-plan-details")
    .test_support()
    .role(gpui_kit::Role::Group)
    .aria_label(details_label(phases))
    .max_h(rems(12.))
    .overflow_y_scroll()
    .gap_2()
    .children(phases.iter().map(|phase| {
      let done = phase.tasks.iter().filter(|task| task.status == TodoStatus::Completed).count();
      v_flex()
        .gap_1()
        .child(
          div()
            .font_semibold()
            .child(format!("{} · {done}/{}", phase.name, phase.tasks.len())),
        )
        .children(phase.tasks.iter().map(|task| {
          let status = status_name(task.status);
          let text: SharedString = task.content.clone().into();
          let styled = StyledText::new(text.clone()).with_highlights(if task.status == TodoStatus::Completed {
            vec![(
              0..text.len(),
              HighlightStyle {
                strikethrough: Some(StrikethroughStyle {
                  thickness: px(1.),
                  ..Default::default()
                }),
                ..Default::default()
              },
            )]
          } else {
            Vec::new()
          });
          v_flex()
            .child(
              h_flex()
                .gap_2()
                .child(div().text_color(cx.theme().muted_foreground).child(status))
                .child(div().min_w_0().child(styled)),
            )
            .when(task.status == TodoStatus::Blocked, |row| {
              row.children(task.blocker.as_ref().map(|blocker| {
                div()
                  .pl_4()
                  .text_color(cx.theme().muted_foreground)
                  .child(format!("Blocked: {blocker}"))
              }))
            })
        }))
    }));
  Some(
    Collapsible::new()
      .open(open)
      .child(
        Button::new("current-plan-toggle")
          .xsmall()
          .ghost()
          .track_focus(focus)
          .toggled(open)
          .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
          .label(summary)
          .on_click(on_toggle),
      )
      .content(details),
  )
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{SessionState, TodoPhase};
  use gpui_kit::component::{button::Button, v_flex};
  use gpui_kit::test::TestWindowExt as _;
  use gpui_kit::{AppContext as _, Context, FocusHandle, IntoElement, ParentElement as _, Render, TestAppContext, Window, WindowOptions};

  struct Plan {
    phases: Vec<TodoPhase>,
    open: bool,
    before: FocusHandle,
  }

  impl Render for Plan {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
      let focus = super::super::focus::tab_stop((gpui_kit::ElementId::View(cx.entity_id()), "plan-focus"), window, cx);
      v_flex()
        .child(Button::new("before").label("Before").track_focus(&self.before))
        .children(super::view(
          &self.phases,
          self.open,
          cx.listener(|this, _, _, cx| {
            this.open = !this.open;
            cx.notify();
          }),
          &focus,
          cx,
        ))
    }
  }

  #[gpui_kit::test]
  fn recorded_snapshots_replace_current_plan(cx: &mut TestAppContext) {
    let before: SessionState = serde_json::from_str(include_str!(
      "../../../alvin-conversation/fixtures/real_todo_transitions.state_before.json"
    ))
    .unwrap();
    let blocked: SessionState = serde_json::from_str(include_str!(
      "../../../alvin-conversation/fixtures/real_todo_transitions.state_blocked.json"
    ))
    .unwrap();
    let completed: SessionState = serde_json::from_str(include_str!(
      "../../../alvin-conversation/fixtures/real_todo_transitions.state_completed.json"
    ))
    .unwrap();
    cx.update(gpui_kit::init);
    let (handle, plan) = cx.update(|cx| {
      gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
        cx.new(|cx| Plan {
          phases: before.todo_phases,
          open: false,
          before: cx.focus_handle().tab_stop(true),
        })
      })
      .unwrap()
    });
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("current-plan-toggle").is_none());
    })
    .unwrap();
    cx.update(|cx| {
      plan.update(cx, |plan, cx| {
        plan.phases = blocked.todo_phases;
        cx.notify();
      })
    });
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find_all("current-plan-toggle").len(), 1);
      assert_eq!(window.find("current-plan-toggle").label(), Some("Plan · 2/3 completed"));
      let before = plan.read(cx).before.clone();
      before.focus(window, cx);
      window.render_frame(cx);
      assert_eq!(window.find("before").focused(), Some(true));
      window.press("tab", cx);
      assert_eq!(window.find("current-plan-toggle").focused(), Some(true));
      window.press("enter", cx);
      let label = window.find("current-plan-details").label().unwrap().to_owned();
      assert!(label.contains("Capture · 2/3"));
      assert!(label.contains("Blocked · Task 3: third item · Blocked: Waiting for an example"));
    })
    .unwrap();
    cx.update(|cx| {
      plan.update(cx, |plan, cx| {
        plan.phases = completed.todo_phases;
        cx.notify();
      })
    });
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert_eq!(window.find_all("current-plan-toggle").len(), 1);
      assert_eq!(window.find("current-plan-toggle").label(), Some("Plan · 3/3 completed"));
      let label = window.find("current-plan-details").label().unwrap().to_owned();
      assert!(label.contains("Capture · 3/3"));
      assert!(label.contains("Completed · Task 3: third item"));
      assert!(!label.contains("Waiting for an example"));
    })
    .unwrap();
    cx.update(|cx| {
      plan.update(cx, |plan, cx| {
        plan.phases.clear();
        cx.notify();
      })
    });
    cx.update_window(handle, |_, window, cx| {
      window.render_frame(cx);
      assert!(window.try_find("current-plan-toggle").is_none());
      assert!(window.try_find("current-plan-details").is_none());
    })
    .unwrap();
  }
}
