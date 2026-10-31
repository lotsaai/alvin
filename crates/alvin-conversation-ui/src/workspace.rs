//! The window's root view: the sidebar, the open sessions as tabs, and the active session.

use alvin_client::Client;
use alvin_protocol::LiveSessionId;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
  AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
  Subscription, Window, div, prelude::FluentBuilder as _,
};

use crate::actions::{ToggleSidebar, WORKSPACE_CONTEXT};
use crate::model::{Model, ModelEvent};
use crate::session::SessionView;
use crate::sidebar::sidebar;

/// The root view of a window: one per window, placed directly under gpui-kit's `Root`.
pub struct Workspace {
  model: Entity<Model>,
  tabs: Vec<(LiveSessionId, Entity<SessionView>)>,
  active: Option<LiveSessionId>,
  sidebar_visible: bool,
  _subscriptions: Vec<Subscription>,
}

impl Workspace {
  /// A workspace over a connected client. Loads the projects.
  pub fn new(client: Client, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let model = cx.new(|cx| Model::new(client, cx));
    let subscriptions = vec![
      cx.observe(&model, |_, _, cx| cx.notify()),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        this.model_event(event, window, cx)
      }),
    ];
    model.update(cx, |model, cx| model.refresh_projects(cx));
    Self {
      model,
      tabs: Vec::new(),
      active: None,
      sidebar_visible: true,
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

  fn model_event(&mut self, event: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event {
      ModelEvent::Opened(live) => {
        if !self.tabs.iter().any(|(tab, _)| tab == live) {
          let model = self.model.clone();
          let live = *live;
          let view = cx.new(|cx| SessionView::new(model, live, window, cx));
          self.tabs.push((live, view));
        }
        self.active = Some(*live);
      }
      ModelEvent::Closed(live) => {
        self.tabs.retain(|(tab, _)| tab != live);
        if self.active == Some(*live) {
          self.active = self.tabs.last().map(|(tab, _)| *tab);
        }
      }
      ModelEvent::Failed { code, message } => {
        let title = code.map_or_else(|| "Error".to_owned(), |code| format!("{code:?}"));
        window.push_notification(Notification::error(message.clone()).title(SharedString::from(title)), cx);
      }
    }
    cx.notify();
  }
}

impl Render for Workspace {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let tabs = self.tabs.iter().map(|(live, _)| {
      let live = *live;
      let model = self.model.clone();
      h_flex()
        .gap_1()
        .child(
          Button::new(ElementId::from(SharedString::from(format!("tab-{}", live.as_uuid()))))
            .small()
            .selected(self.active == Some(live))
            .label(SharedString::from(self.tab_title(live, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
              this.active = Some(live);
              cx.notify();
            })),
        )
        .child(
          Button::new(ElementId::from(SharedString::from(format!("close-{}", live.as_uuid()))))
            .small()
            .ghost()
            .label("Close")
            .on_click(move |_, _, cx| model.update(cx, |model, cx| model.close(live, cx))),
        )
    });
    let active = self
      .active
      .and_then(|live| self.tabs.iter().find(|(tab, _)| *tab == live))
      .map(|(_, view)| view.clone());
    h_flex()
      .key_context(WORKSPACE_CONTEXT)
      .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
        this.sidebar_visible = !this.sidebar_visible;
        cx.notify();
      }))
      .size_full()
      .bg(cx.theme().background)
      .items_stretch()
      .when(self.sidebar_visible, |this| this.child(sidebar(&self.model, cx)))
      .child(
        v_flex()
          .flex_1()
          .min_w_0()
          .child(h_flex().gap_2().p_2().children(tabs))
          .child(match active {
            Some(view) => div().flex_1().min_h_0().child(view).into_any_element(),
            None => div().flex_1().child("Open a session from the sidebar").into_any_element(),
          }),
      )
  }
}

impl Workspace {
  fn tab_title(&self, live: LiveSessionId, cx: &Context<Self>) -> String {
    self
      .model
      .read(cx)
      .conversation()
      .live(live)
      .map(|view| {
        view.title.clone().unwrap_or_else(|| {
          view
            .cwd
            .file_name()
            .map_or_else(|| "session".to_owned(), |name| name.to_string_lossy().into_owned())
        })
      })
      .unwrap_or_else(|| "closed".to_owned())
  }
}

#[cfg(test)]
mod tests {
  use alvin_client::Client;
  use alvin_conversation::Run;
  use alvin_protocol::{AgentMessage, LiveSessionId, PromptStatus};
  use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
  use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};

  use super::Workspace;
  use crate::test_support::{Fixture, until};

  fn open_window(cx: &mut TestAppContext, client: Client) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(gpui_kit::init);
    cx.update(crate::init);
    let (handle, workspace) = cx.update(|cx| {
      gpui_kit::open_window(
        WindowOptions {
          window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1100.), px(700.))))),
          ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| Workspace::new(client, window, cx)),
      )
      .expect("open the test window")
    });
    (handle, workspace)
  }

  fn active(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> Option<LiveSessionId> {
    cx.read(|cx| workspace.read(cx).active())
  }

  /// Reads the active session's state out of the model.
  fn with_active<T>(
    cx: &mut TestAppContext,
    workspace: &Entity<Workspace>,
    read: impl FnOnce(&alvin_conversation::LiveView) -> T,
  ) -> Option<T> {
    cx.read(|cx| {
      let workspace = workspace.read(cx);
      let live = workspace.active()?;
      workspace.model().read(cx).conversation().live(live).map(read)
    })
  }

  /// Clicks the composer, found by its accessible label, types `text` and presses Enter.
  fn type_and_send(cx: &mut TestAppContext, window: AnyWindowHandle, text: &str) {
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let composer = window
        .find_by_label("Message")
        .path()
        .last()
        .cloned()
        .expect("the composer has an element id");
      window.click(composer, cx);
      window.input(text, cx);
      window.press("enter", cx);
    })
    .unwrap();
  }

  fn open_new_session(cx: &mut TestAppContext, window: AnyWindowHandle, workspace: &Entity<Workspace>) {
    until(cx, "the projects to load", |cx| {
      cx.read(|cx| !workspace.read(cx).model().read(cx).conversation().workspace().projects().is_empty())
    });
    let checkout = cx.read(|cx| {
      let projects = workspace.read(cx).model().read(cx).conversation().workspace().projects().to_vec();
      projects[0].checkouts[0].id.clone()
    });
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      let id = format!("new-session-{}", checkout.as_str());
      let button = window.find(id.clone());
      assert!(
        button.visible(),
        "button {:?} sidebar {:?} viewport {:?}",
        button.bounds(),
        window.find("sidebar").bounds(),
        window.viewport_size()
      );
      window.click(id, cx);
    })
    .unwrap();
    until(cx, "the session to open", |cx| active(cx, workspace).is_some());
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  #[gpui_kit::test]
  async fn a_new_session_streams_a_reply_and_settles(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    let (window, workspace) = open_window(cx, fixture.client().await);
    open_new_session(cx, window, &workspace);

    type_and_send(cx, window, "Reply with the single word ok.");
    until(cx, "the turn to settle", |cx| {
      with_active(cx, &workspace, |view| view.run == Run::Settled).unwrap_or(false)
    });

    let (roles, last_prompt, streaming) = with_active(cx, &workspace, |view| {
      (
        view
          .transcript
          .iter()
          .map(|message| matches!(message, AgentMessage::User(_)))
          .collect::<Vec<_>>(),
        view.last_prompt,
        view.streaming.is_some(),
      )
    })
    .unwrap();
    assert_eq!(roles, [true, false]);
    assert_eq!(last_prompt, Some(PromptStatus::Completed));
    assert!(!streaming);
    fixture.finish().await;
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  #[gpui_kit::test]
  async fn a_listed_session_opens_with_its_transcript(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    fixture.write_session();
    let (window, workspace) = open_window(cx, fixture.client().await);

    // The sidebar lists the session real omp wrote; clicking it opens it.
    let session = loop_until_listed(cx, &workspace);
    cx.update_window(window, |_, window, cx| {
      window.render_frame(cx);
      window.click(format!("session-{}", session.as_str()), cx);
    })
    .unwrap();
    until(cx, "the session's transcript", |cx| {
      with_active(cx, &workspace, |view| view.transcript.len() == 2).unwrap_or(false)
    });

    let (user, assistant) = with_active(cx, &workspace, |view| {
      (
        matches!(view.transcript[0], AgentMessage::User(_)),
        matches!(view.transcript[1], AgentMessage::Assistant(_)),
      )
    })
    .unwrap();
    assert!(user && assistant);
    fixture.finish().await;
  }

  /// Waits for the sidebar to list a session that real omp wrote, and returns its id.
  fn loop_until_listed(cx: &mut TestAppContext, workspace: &Entity<Workspace>) -> alvin_protocol::SessionId {
    let mut found = None;
    until(cx, "a listed session", |cx| {
      found = cx.read(|cx| {
        let state = workspace.read(cx).model().read(cx);
        let workspace = state.conversation().workspace();
        workspace.projects().iter().find_map(|project| {
          project.checkouts.iter().find_map(|checkout| {
            let key = alvin_conversation::SessionsKey {
              project: project.id.clone(),
              checkout: Some(checkout.id.clone()),
            };
            // The seed session is listed too; the one omp wrote has a real title.
            workspace
              .sessions(&key)?
              .sessions
              .iter()
              .find(|session| session.title.as_deref() == Some("Reply with the single word ok."))
              .map(|session| session.id.clone())
          })
        })
      });
      found.is_some()
    });
    found.unwrap()
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  #[gpui_kit::test]
  async fn a_bang_command_runs_in_the_session_and_lands_in_the_transcript(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    let (window, workspace) = open_window(cx, fixture.client().await);
    open_new_session(cx, window, &workspace);

    type_and_send(cx, window, "!echo alvin-bang-ok");
    until(cx, "the command's output", |cx| {
      with_active(cx, &workspace, |view| {
        view.transcript.iter().any(|message| matches!(message, AgentMessage::BashExecution(bash) if bash.output.as_deref().is_some_and(|output| output.contains("alvin-bang-ok"))))
      })
      .unwrap_or(false)
    });
    fixture.finish().await;
  }

  #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
  #[gpui_kit::test]
  async fn escape_aborts_a_running_turn(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let fixture = Fixture::new();
    let (window, workspace) = open_window(cx, fixture.client().await);
    open_new_session(cx, window, &workspace);

    type_and_send(cx, window, "Write the numbers from 1 to 400, one per line, nothing else.");
    until(cx, "the reply to start streaming", |cx| {
      with_active(cx, &workspace, |view| view.streaming.is_some() && view.run == Run::Running).unwrap_or(false)
    });
    cx.update_window(window, |_, window, cx| window.press("escape", cx)).unwrap();
    until(cx, "the turn to be aborted", |cx| {
      with_active(cx, &workspace, |view| view.last_prompt == Some(PromptStatus::Aborted)).unwrap_or(false)
    });
    fixture.finish().await;
  }
}
