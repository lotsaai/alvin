//! The window's root view: the project tree, the open sessions as tabs, and the active session.

use alvin_conversation::{Connection, Level};
use alvin_protocol::{ErrorCode, LiveSessionId};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::{
  AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
  StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _,
};

use crate::actions::{ToggleSidebar, WORKSPACE_CONTEXT};
use crate::model::{Model, ModelEvent};
use crate::session::SessionView;
use crate::sidebar::ProjectTree;

/// The root view of a window: one per window, placed directly under gpui-kit's `Root`.
pub struct Workspace {
  model: Entity<Model>,
  projects: Entity<ProjectTree>,
  tabs: Vec<(LiveSessionId, Entity<SessionView>)>,
  active: Option<LiveSessionId>,
  sidebar_visible: bool,
  /// Keeps the window's key bindings in reach when nothing inside it has focus.
  focus: FocusHandle,
  _subscriptions: Vec<Subscription>,
}

impl Workspace {
  /// A workspace over the model it shows.
  pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let focus = cx.focus_handle();
    focus.focus(window, cx);
    let projects = cx.new(|cx| ProjectTree::new(model.clone(), cx));
    let subscriptions = vec![
      cx.observe(&model, |_, _, cx| cx.notify()),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        this.model_event(event, window, cx);
      }),
    ];
    Self {
      model,
      projects,
      tabs: Vec::new(),
      active: None,
      sidebar_visible: true,
      focus,
      _subscriptions: subscriptions,
    }
  }

  /// The model the views render.
  pub fn model(&self) -> &Entity<Model> {
    &self.model
  }

  /// The live session whose tab is showing.
  pub fn active(&self) -> Option<LiveSessionId> {
    self.active
  }

  fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.sidebar_visible = !self.sidebar_visible;
    // The tree leaves the window with the sidebar, and the key that brings it back has to stay in reach.
    if !self.sidebar_visible {
      self.focus.focus(window, cx);
    }
    cx.notify();
  }

  fn model_event(&mut self, event: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event {
      ModelEvent::Opened(live) => {
        let live = *live;
        if !self.tabs.iter().any(|(tab, _)| *tab == live) {
          let model = self.model.clone();
          let view = cx.new(|cx| SessionView::new(model, live, window, cx));
          self.tabs.push((live, view));
        }
        self.active = Some(live);
      }
      ModelEvent::Closed(live) => {
        self.tabs.retain(|(tab, _)| tab != live);
        if self.active == Some(*live) {
          self.active = self.tabs.last().map(|(tab, _)| *tab);
        }
      }
      // The banner already says it: the stream has to be read again, and a notification would say it twice.
      ModelEvent::Failed {
        code: Some(ErrorCode::OmpProcessExited | ErrorCode::StaleCursor),
        ..
      } => {}
      ModelEvent::Failed { code, message } => {
        let title = code.map_or_else(|| "Error".to_owned(), |code| format!("{code:?}"));
        window.push_notification(Notification::error(message.clone()).title(SharedString::from(title)), cx);
      }
      // The session's own view puts the text in its composer.
      ModelEvent::ComposerText { .. } => {}
      ModelEvent::Exported { path, .. } => {
        window.push_notification(Notification::success(SharedString::from(format!("Exported to {path}"))), cx);
      }
      ModelEvent::Notice { notice, .. } => {
        let message = SharedString::from(notice.text.clone());
        let notification = match notice.level {
          Level::Info => Notification::info(message),
          Level::Warning => Notification::warning(message),
          Level::Error => Notification::error(message),
        };
        window.push_notification(notification, cx);
      }
    }
    cx.notify();
  }

  fn tab_title(&self, live: LiveSessionId, cx: &gpui_kit::App) -> SharedString {
    let title = self.model.read(cx).conversation().live(live).map(|view| {
      view.title.clone().unwrap_or_else(|| {
        view
          .cwd
          .file_name()
          .map_or_else(|| "session".to_owned(), |name| name.to_string_lossy().into_owned())
      })
    });
    SharedString::from(title.unwrap_or_else(|| "closed".to_owned()))
  }

  fn connection_banner(&self, cx: &gpui_kit::App) -> Option<impl IntoElement> {
    let (alert, text) = match self.model.read(cx).conversation().connection() {
      Connection::Connected => return None,
      Connection::Reconnecting { attempt } => {
        let text = SharedString::from(format!("Connection to the gateway dropped; reconnecting (attempt {attempt})"));
        (Alert::warning("connection-alert", text.clone()), text)
      }
      Connection::Lost => {
        let text = SharedString::from("Connection to the gateway lost");
        (Alert::error("connection-alert", text.clone()), text)
      }
    };
    Some(
      div()
        .id("connection-banner")
        .test_support()
        .role(Role::Status)
        .aria_label(text)
        .child(alert.banner()),
    )
  }

  fn render_tabs(&self, cx: &mut Context<Self>) -> TabBar {
    let selected = self
      .active
      .and_then(|active| self.tabs.iter().position(|(live, _)| *live == active));
    TabBar::new("tabs")
      .selected_index(selected.unwrap_or_default())
      .on_click(cx.listener(|this, index: &usize, _, cx| {
        this.active = this.tabs.get(*index).map(|(live, _)| *live);
        cx.notify();
      }))
      .children(self.tabs.iter().map(|(live, _)| {
        let live = *live;
        let model = self.model.clone();
        Tab::new().label(self.tab_title(live, cx)).suffix(
          Button::new(SharedString::from(format!("close-{}", live.as_uuid())))
            .ghost()
            .xsmall()
            .icon(IconName::Close)
            .accessibility_label("Close session")
            .on_click(move |_, _, cx| model.update(cx, |model, cx| model.close(live, cx)).detach()),
        )
      }))
  }
}

impl Render for Workspace {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let rem = window.rem_size();
    let active = self
      .tabs
      .iter()
      .find(|(live, _)| Some(*live) == self.active)
      .map(|(_, view)| view.clone());
    let toggle_label = if self.sidebar_visible { "Hide sidebar" } else { "Show sidebar" };
    v_flex()
      .id("workspace")
      .key_context(WORKSPACE_CONTEXT)
      .track_focus(&self.focus)
      .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)))
      .size_full()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .children(self.connection_banner(cx))
      .child(
        div().flex_1().min_h_0().child(
          h_resizable("workspace-panes")
            .when(self.sidebar_visible, |this| {
              this.child(
                resizable_panel()
                  .size(rem * 18.)
                  .size_range(rem * 12. ..rem * 32.)
                  .child(v_flex().size_full().min_h_0().bg(cx.theme().sidebar).child(self.projects.clone())),
              )
            })
            .child(
              resizable_panel().size_range(rem * 30. ..rem * 400.).child(
                v_flex().size_full().min_w_0().child(self.render_tabs(cx)).child(match active {
                  Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
                  None => div()
                    .flex_1()
                    .p_4()
                    .text_color(cx.theme().muted_foreground)
                    .child("Open a session from the sidebar")
                    .into_any_element(),
                }),
              ),
            ),
        ),
      )
      .child(
        StatusBar::new().left(
          Button::new("toggle-sidebar")
            .toggled(self.sidebar_visible)
            .ghost()
            .small()
            .icon(if self.sidebar_visible {
              IconName::PanelLeftClose
            } else {
              IconName::PanelLeftOpen
            })
            .accessibility_label(toggle_label)
            .tooltip_with_action(toggle_label, &ToggleSidebar, Some(WORKSPACE_CONTEXT))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx))),
        ),
      )
  }
}

/// Test-only access, so tests reach a session's own state without widening visibility.
#[cfg(test)]
impl Workspace {
  /// The view of an open session.
  pub(crate) fn session(&self, live: LiveSessionId) -> Option<&Entity<SessionView>> {
    self.tabs.iter().find(|(id, _)| *id == live).map(|(_, view)| view)
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use gpui_kit::test::{TestAppContextExt as _, TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
  use gpui_kit::{AppContext as _, MouseButton, TestAppContext};

  use crate::test_support::{ROOT, live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_tree_lists_the_sessions_of_every_checkout(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      cx.wait_for(opened.handle, BUDGET, |window, _| {
        window.try_find_by_label("Write the docs").is_some()
      })
      .await;
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        for label in ["alvin", ROOT, "New session", "Fix the build", "Write the docs"] {
          assert!(window.find_by_label(label).visible(), "{label} is listed");
        }
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn an_opened_session_gets_a_tab_and_clicking_a_tab_activates_it(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let (first, second) = (live_session("/work/api"), live_session("/work/web"));
      cx.update(|cx| {
        opened.model.update(cx, |model, cx| {
          model.seed_open(&first, cx);
          model.seed_open(&second, cx);
        });
      });
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("web").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        assert!(window.find_by_label("api").visible());
        assert_eq!(opened.workspace.read(cx).active(), Some(second.live));

        let tab = window.find_by_label("api").bounds().center();
        window.pointer_down(tab, MouseButton::Left, cx);
        window.pointer_up(tab, MouseButton::Left, cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(first.live));
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn closing_the_active_session_activates_the_one_before_it(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let (first, second) = (live_session("/work/api"), live_session("/work/web"));
      cx.update(|cx| {
        opened.model.update(cx, |model, cx| {
          model.seed_open(&first, cx);
          model.seed_open(&second, cx);
          model.seed_close(second.live, cx);
        });
      });
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("web").is_none())
        .await;
      cx.update_window(opened.handle, |_, window, cx| {
        assert!(window.find_by_label("api").visible());
        assert_eq!(opened.workspace.read(cx).active(), Some(first.live));
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_sidebar_key_and_the_status_bar_button_both_hide_and_show_the_tree(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let key = if cfg!(target_os = "macos") { "cmd-b" } else { "ctrl-b" };
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("New session").is_some())
        .await;

      cx.update_window(opened.handle, |_, window, cx| {
        window.press(key, cx);
        assert!(!opened.workspace.read(cx).sidebar_visible);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("New session").is_none())
        .await;

      cx.update_window(opened.handle, |_, window, cx| {
        assert_eq!(window.find("toggle-sidebar").label(), Some("Show sidebar"));
        window.click("toggle-sidebar", cx);
        assert!(opened.workspace.read(cx).sidebar_visible);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("New session").is_some())
        .await;
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn losing_the_gateway_shows_the_connection_banner(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("connection-banner").is_none());
      })
      .unwrap();
      cx.update(|cx| {
        opened
          .model
          .update(cx, |model, cx| model.seed_connection(alvin_conversation::Connection::Lost, cx))
      });
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find("connection-banner").is_some())
        .await;
      cx.update_window(opened.handle, |_, window, _| {
        assert_eq!(window.find("connection-banner").label(), Some("Connection to the gateway lost"));
      })
      .unwrap();
    });
  }
}
