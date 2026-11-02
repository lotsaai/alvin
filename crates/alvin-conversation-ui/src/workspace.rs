//! The window's root view: the project tree, the open sessions as tabs, and the active session.

use std::collections::{BTreeMap, BTreeSet};

use alvin_conversation::{Connection, Level};
use alvin_protocol::{ErrorCode, LiveSessionId};
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
  AppContext as _, Context, Entity, EventEmitter, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _, Render, Role,
  SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, relative,
};

use crate::actions::{ToggleSidebar, WORKSPACE_CONTEXT};
use crate::model::{Model, ModelEvent};
use crate::session::SessionView;
use crate::sidebar::ProjectTree;

/// Local presentation preferences indexed by the gateway's stable project ID.
pub type ProjectPreferences = BTreeMap<String, ProjectPreference>;

/// Local presentation of a project; it never changes project files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPreference {
  /// Optional display name, independent of the project directory.
  pub alias: Option<String>,
  /// Whether this project appears before unpinned projects.
  pub pinned: bool,
  /// Whether this project is hidden from the project tree.
  pub hidden: bool,
  /// Session IDs pinned within this project.
  pub pinned_sessions: BTreeSet<String>,
  /// Session IDs hidden locally, without changing omp files.
  pub hidden_sessions: BTreeSet<String>,
}

/// Changes to locally persisted workspace presentation.
#[derive(Clone, Debug)]
pub enum WorkspaceEvent {
  /// The user changed project presentation preferences.
  ProjectPreferencesChanged(ProjectPreferences),
  /// The user requested application settings.
  SettingsRequested,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

#[derive(Default)]
struct NavigationHistory {
  entries: Vec<LiveSessionId>,
  index: Option<usize>,
}

impl NavigationHistory {
  fn visit(&mut self, live: LiveSessionId) {
    if self.index.and_then(|index| self.entries.get(index)) == Some(&live) {
      return;
    }
    self.entries.truncate(self.index.map_or(0, |index| index.saturating_add(1)));
    self.entries.push(live);
    self.index = self.entries.len().checked_sub(1);
  }

  fn target(&self, forward: bool) -> Option<(usize, LiveSessionId)> {
    let index = self
      .index
      .and_then(|index| if forward { index.checked_add(1) } else { index.checked_sub(1) })?;
    self.entries.get(index).copied().map(|live| (index, live))
  }

  fn traverse(&mut self, forward: bool) -> Option<LiveSessionId> {
    let (index, live) = self.target(forward)?;
    self.index = Some(index);
    Some(live)
  }

  fn remove(&mut self, live: LiveSessionId) {
    let old_index = self.index;
    let before = old_index.map_or(0, |index| self.entries.iter().take(index).filter(|entry| **entry == live).count());
    let current_removed = old_index.and_then(|index| self.entries.get(index)) == Some(&live);
    self.entries.retain(|entry| *entry != live);
    self.index = old_index
      .and_then(|index| {
        let index = index.saturating_sub(before);
        if current_removed {
          index.checked_sub(1).or(Some(0))
        } else {
          Some(index)
        }
      })
      .filter(|index| *index < self.entries.len());
    let mut previous = None;
    let mut position = 0;
    let mut index = self.index;
    self.entries.retain(|entry| {
      let duplicate = previous == Some(*entry);
      previous = Some(*entry);
      if duplicate && index.is_some_and(|current| position <= current) {
        index = index.and_then(|current| current.checked_sub(1));
      }
      if !duplicate {
        position += 1;
      }
      !duplicate
    });
    self.index = index;
  }
}

/// The root view of a window: one per window, placed directly under gpui-kit's `Root`.
pub struct Workspace {
  model: Entity<Model>,
  projects: Entity<ProjectTree>,
  tabs: Vec<(LiveSessionId, Entity<SessionView>)>,
  active: Option<LiveSessionId>,
  history: NavigationHistory,
  user_name: String,
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
    let projects = cx.new(|cx| ProjectTree::new(model.clone(), window, cx));
    let subscriptions = vec![
      cx.observe(&model, |this, _, cx| {
        this.sync_active_sidebar(cx);
        cx.notify();
      }),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        this.model_event(event, window, cx);
      }),
      cx.subscribe(&projects, |_, _, preferences: &ProjectPreferences, cx| {
        cx.emit(WorkspaceEvent::ProjectPreferencesChanged(preferences.clone()));
      }),
    ];
    Self {
      model,
      projects,
      tabs: Vec::new(),
      active: None,
      history: NavigationHistory::default(),
      user_name: "Local user".to_owned(),
      sidebar_visible: true,
      focus,
      _subscriptions: subscriptions,
    }
  }

  /// The model the views render.
  pub fn model(&self) -> &Entity<Model> {
    &self.model
  }

  /// Sets the local display name shown in the sidebar.
  pub fn set_user_name(&mut self, name: String, cx: &mut Context<Self>) {
    self.user_name = if name.trim().is_empty() { "Local user".to_owned() } else { name };
    cx.notify();
  }

  /// Returns the local display name shown in the sidebar.
  pub fn user_name(&self) -> &str {
    &self.user_name
  }

  fn traverse(&mut self, forward: bool, cx: &mut Context<Self>) {
    if let Some(live) = self.history.traverse(forward) {
      self.active = Some(live);
      self.model.update(cx, |model, _| model.select_tab());
      self.sync_active_sidebar(cx);
      cx.notify();
    }
  }

  /// Applies saved presentation preferences without emitting a change event.
  pub fn set_project_preferences(&mut self, preferences: ProjectPreferences, cx: &mut Context<Self>) {
    self.projects.update(cx, |projects, cx| projects.set_preferences(preferences, cx));
  }

  /// The live session whose tab is showing.
  pub fn active(&self) -> Option<LiveSessionId> {
    self.active
  }

  /// Whether the project sidebar is visible.
  pub fn sidebar_visible(&self) -> bool {
    self.sidebar_visible
  }

  /// Returns keyboard focus to the workspace.
  pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
    self.focus.focus(window, cx);
  }

  /// Shows or hides the project sidebar.
  pub fn toggle_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.sidebar_visible = !self.sidebar_visible;
    // The tree leaves the window with the sidebar, and the key that brings it back has to stay in reach.
    if !self.sidebar_visible {
      self.focus.focus(window, cx);
    }
    cx.notify();
  }

  fn model_event(&mut self, event: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
    match event {
      ModelEvent::Opened { live, intent } => {
        let live = *live;
        if !self.tabs.iter().any(|(tab, _)| *tab == live) {
          let model = self.model.clone();
          let view = cx.new(|cx| SessionView::new(model, live, window, cx));
          self.tabs.push((live, view));
        }
        if self.model.read(cx).navigation_is_current(*intent) {
          self.active = Some(live);
          self.history.visit(live);
          self.sync_active_sidebar(cx);
        }
      }
      ModelEvent::Closed(live) => {
        self.tabs.retain(|(tab, _)| tab != live);
        self.history.remove(*live);
        if self.active == Some(*live) {
          self.active = self
            .history
            .index
            .and_then(|index| self.history.entries.get(index).copied())
            .or_else(|| self.tabs.last().map(|(tab, _)| *tab));
          if self.history.index.is_none()
            && let Some(live) = self.active
          {
            self.history.visit(live);
          }
          self.sync_active_sidebar(cx);
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

  fn sync_active_sidebar(&mut self, cx: &mut Context<Self>) {
    let active = self.active.and_then(|live| {
      let model = self.model.read(cx);
      model
        .conversation()
        .live(live)
        .map(|view| (model.session_id(live).cloned(), view.file.clone()))
    });
    self.projects.update(cx, |projects, cx| projects.set_active(active, cx));
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
        if let Some(live) = this.tabs.get(*index).map(|(live, _)| *live) {
          this.active = Some(live);
          this.history.visit(live);
          this.model.update(cx, |model, _| model.select_tab());
          this.sync_active_sidebar(cx);
        }
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

  fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let label = if self.sidebar_visible { "Hide sidebar" } else { "Show sidebar" };
    let back = self.history.target(false).is_some();
    let forward = self.history.target(true).is_some();
    let controls = h_flex()
      .id("titlebar-sidebar-controls")
      .flex_1()
      .w_full()
      .justify_start()
      .min_w_0()
      .child(
        h_flex()
          .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
          .child(
            Button::new("titlebar-toggle-sidebar")
              .ghost()
              .xsmall()
              .icon(if self.sidebar_visible {
                IconName::PanelLeftClose
              } else {
                IconName::PanelLeftOpen
              })
              .accessibility_label(label)
              .tooltip(label)
              .on_click(cx.listener(|this, _, window, cx| this.toggle_sidebar(window, cx))),
          )
          .child(
            Button::new("navigation-back")
              .ghost()
              .xsmall()
              .icon(IconName::ChevronLeft)
              .accessibility_label("Back")
              .tooltip("Back")
              .disabled(!back)
              .on_click(cx.listener(|this, _, _, cx| this.traverse(false, cx))),
          )
          .child(
            Button::new("navigation-forward")
              .ghost()
              .xsmall()
              .icon(IconName::ChevronRight)
              .accessibility_label("Forward")
              .tooltip("Forward")
              .disabled(!forward)
              .on_click(cx.listener(|this, _, _, cx| this.traverse(true, cx))),
          ),
      )
      .child(div().flex_1().min_w_0().child(self.render_tabs(cx)));
    if cfg!(target_os = "macos") {
      gpui_kit::component::TitleBar::new()
        .bg(cx.theme().sidebar)
        .child(controls)
        .into_any_element()
    } else {
      controls.into_any_element()
    }
  }

  fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
    let scopes: Vec<_> = self
      .model
      .read(cx)
      .conversation()
      .workspace()
      .projects()
      .iter()
      .flat_map(|project| {
        project
          .checkouts
          .iter()
          .map(move |checkout| (project.id.clone(), checkout.id.clone(), checkout.root.display().to_string()))
      })
      .collect();
    let model = self.model.clone();
    let owner = cx.entity();
    let name = SharedString::from(self.user_name.clone());
    let menu_owner = owner.clone();
    v_flex()
      .size_full()
      .min_h_0()
      .bg(cx.theme().sidebar)
      .child(
        v_flex()
          .p_2()
          .items_start()
          .gap_1()
          .child(
            Button::new("new-session")
              .ghost()
              .small()
              .max_w(relative(1.))
              .icon(IconName::Plus)
              .label("New session…")
              .disabled(scopes.is_empty())
              .tooltip(if scopes.is_empty() {
                "No project checkouts available"
              } else {
                "Choose a project checkout"
              })
              .dropdown_menu(move |mut menu, _, _| {
                for (project, checkout, path) in &scopes {
                  let model = model.clone();
                  let project = project.clone();
                  let checkout = checkout.clone();
                  menu = menu.item(PopupMenuItem::new(SharedString::from(path.clone())).on_click(move |_, _, cx| {
                    model
                      .update(cx, |model, cx| model.new_session(project.clone(), checkout.clone(), cx))
                      .detach();
                  }));
                }
                menu
              }),
          )
          .child(
            Button::new("automations")
              .ghost()
              .small()
              .max_w(relative(1.))
              .label("Automations · Coming soon")
              .disabled(true),
          )
          .child(
            Button::new("plugins")
              .ghost()
              .small()
              .max_w(relative(1.))
              .label("Plugins · Coming soon")
              .disabled(true),
          ),
      )
      .child(div().flex_1().min_h_0().child(self.projects.clone()))
      .child(
        h_flex()
          .id("sidebar-footer")
          .test_support()
          .p_2()
          .gap_1()
          .border_t_1()
          .border_color(cx.theme().border)
          .child(
            div().flex_1().min_w_0().child(
              Button::new("user-menu")
                .ghost()
                .small()
                .w_full()
                .min_w_0()
                .accessibility_label(name.clone())
                .child(div().flex_shrink_0().child(Avatar::new().name(name.clone()).small()))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .dropdown_menu_with_anchor(gpui_kit::Anchor::BottomLeft, move |menu, _, _| {
                  let owner = menu_owner.clone();
                  menu.item(PopupMenuItem::new("Settings…").on_click(move |_, _, cx| {
                    owner.update(cx, |_, cx| cx.emit(WorkspaceEvent::SettingsRequested));
                  }))
                }),
            ),
          )
          .child(
            Button::new("mobile-coming-soon")
              .ghost()
              .xsmall()
              .flex_shrink_0()
              .icon(gpui_kit::assets::IconName::Smartphone)
              .accessibility_label("Mobile · Coming soon")
              .tooltip("Mobile · Coming soon")
              .disabled(true),
          )
          .child(
            Button::new("sidebar-settings")
              .flex_shrink_0()
              .ghost()
              .xsmall()
              .icon(IconName::Settings)
              .accessibility_label("Settings")
              .tooltip("Settings")
              .on_click(cx.listener(|_, _, _, cx| cx.emit(WorkspaceEvent::SettingsRequested))),
          ),
      )
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
    v_flex()
      .id("workspace")
      .key_context(WORKSPACE_CONTEXT)
      .track_focus(&self.focus)
      .on_action(cx.listener(|this, _: &ToggleSidebar, window, cx| this.toggle_sidebar(window, cx)))
      .size_full()
      .bg(cx.theme().sidebar)
      .text_color(cx.theme().foreground)
      .child(self.render_header(cx))
      .child(
        div().flex_1().min_h_0().child(
          h_resizable("workspace-panes")
            .when(self.sidebar_visible, |this| {
              this.child(
                resizable_panel()
                  .size(rem * 15.)
                  .size_range(rem * 12. ..rem * 24.)
                  .child(self.render_sidebar(cx)),
              )
            })
            .child(
              resizable_panel().size_range(rem * 30. ..rem * 400.).child(
                v_flex().size_full().min_w_0().child(
                  v_flex()
                    .flex_1()
                    .min_h_0()
                    .m_1()
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .bg(cx.theme().background)
                    .children(self.connection_banner(cx))
                    .child(match active {
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

  use crate::test_support::{live_session, open_workspace};
  use crate::workspace::{NavigationHistory, ProjectPreference, ProjectPreferences};

  const BUDGET: Duration = Duration::from_secs(1);

  #[test]
  fn navigation_history_traverses_branches_and_prunes_closed_sessions() {
    let (a, b, c) = (
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
    );
    let mut history = NavigationHistory::default();
    assert_eq!(history.traverse(false), None);
    history.visit(a);
    history.visit(b);
    history.visit(c);
    assert_eq!(history.traverse(false), Some(b));
    assert_eq!(history.traverse(false), Some(a));
    assert_eq!(history.traverse(false), None);
    assert_eq!(history.traverse(true), Some(b));
    history.visit(a);
    assert_eq!(history.traverse(true), None);
    history.remove(b);
    assert_eq!(history.traverse(false), None);
    history.remove(a);
    assert_eq!(history.traverse(false), None);
    assert_eq!(history.traverse(true), None);
  }

  #[test]
  fn closing_inactive_tab_coalesces_history_without_losing_current_position() {
    let (a, b, c) = (
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
    );
    let mut history = NavigationHistory::default();
    for live in [a, b, a, c] {
      history.visit(live);
    }
    history.remove(b);
    assert_eq!(history.traverse(false), Some(a));
    assert_eq!(history.traverse(false), None);
    assert_eq!(history.traverse(true), Some(c));
  }

  #[test]
  fn closing_current_history_entry_preserves_forward_navigation() {
    let (a, b, c, d) = (
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
      alvin_protocol::LiveSessionId::new(),
    );
    let mut history = NavigationHistory::default();
    for live in [a, b, c, d] {
      history.visit(live);
    }
    assert_eq!(history.traverse(false), Some(c));
    assert_eq!(history.traverse(false), Some(b));
    history.remove(b);
    assert_eq!(history.entries.get(history.index.unwrap()), Some(&a));
    assert_eq!(history.traverse(true), Some(c));
    assert_eq!(history.traverse(true), Some(d));
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn sidebar_controls_navigate_real_sessions_and_request_settings(cx: &mut TestAppContext) {
    use crate::model::{Model, ModelEvent};
    use crate::workspace::{Workspace, WorkspaceEvent};
    use alvin_client::{ChannelPair, Client, Endpoint};
    use alvin_gateway::{Gateway, GatewayConfig, HostConfig};
    use alvin_protocol::{ListSessions, ProjectsApiClient, SessionsApiClient};
    use gpui_kit::{Bounds, Context, Entity, Subscription, WindowBounds, WindowOptions, px, size};
    use std::path::PathBuf;

    struct Events {
      _subscriptions: Vec<Subscription>,
    }
    impl Events {
      fn new(
        model: &Entity<Model>,
        workspace: &Entity<Workspace>,
        tx: tokio::sync::mpsc::UnboundedSender<bool>,
        cx: &mut Context<Self>,
      ) -> Self {
        let opened = tx.clone();
        Self {
          _subscriptions: vec![
            cx.subscribe(model, move |_, _, event: &ModelEvent, _| {
              if matches!(event, ModelEvent::Opened { .. }) {
                opened.send(false).unwrap();
              }
            }),
            cx.subscribe(workspace, move |_, _, event: &WorkspaceEvent, _| {
              if matches!(event, WorkspaceEvent::SettingsRequested) {
                tx.send(true).unwrap();
              }
            }),
          ],
        }
      }
    }

    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let base = std::env::temp_dir().join(format!("alvin-sidebar-{}", alvin_protocol::EventId::new().as_uuid()));
      let repo = base.join("repo");
      let sessions = base.join("sessions").join("-repo");
      std::fs::create_dir_all(&repo).unwrap();
      std::fs::create_dir_all(&sessions).unwrap();
      assert!(
        std::process::Command::new("git")
          .args(["init", "--quiet"])
          .current_dir(&repo)
          .status()
          .unwrap()
          .success()
      );
      let repo = repo.canonicalize().unwrap();
      let seed = include_str!("../../alvin-app/fixtures/seed-session.jsonl").replace("/tmp/alvin-probe.hnBA/proj", repo.to_str().unwrap());
      std::fs::write(
        sessions.join("2026-01-01T00-00-00-000Z_01a18ff1-ab74-73b8-a3a2-79905565c49b.jsonl"),
        seed,
      )
      .unwrap();
      let gateway = Gateway::new(
        GatewayConfig::builder()
          .sessions_dir(base.join("sessions"))
          .omp(HostConfig::builder().omp(PathBuf::from("omp")).build())
          .build(),
      );
      let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64).unwrap())))
        .await
        .unwrap();
      let projects = ProjectsApiClient::list(&client).await.unwrap();
      let project = projects.first().unwrap();
      let key = alvin_conversation::SessionsKey {
        project: project.id.clone(),
        checkout: Some(project.checkouts.first().unwrap().id.clone()),
      };
      let page = SessionsApiClient::list(
        &client,
        ListSessions {
          project: key.project.clone(),
          checkout: key.checkout.clone(),
          after: None,
          limit: 50,
        },
      )
      .await
      .unwrap();
      let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
      let (handle, workspace, _events) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
        let model = cx.new(|cx| {
          let mut model = Model::new(client, cx);
          model.seed_workspace(projects, vec![(key, page)], cx);
          model
        });
        let (handle, workspace) = gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1100.), px(700.))))),
            ..Default::default()
          },
          cx,
          |window, cx| cx.new(|cx| Workspace::new(model.clone(), window, cx)),
        )
        .unwrap();
        let events = cx.new(|cx| Events::new(&model, &workspace, tx, cx));
        (handle, workspace, events)
      });
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("automations").disabled(), Some(true));
        assert_eq!(window.find("plugins").disabled(), Some(true));
        assert_eq!(window.find("mobile-coming-soon").disabled(), Some(true));
        assert_eq!(window.find("navigation-back").disabled(), Some(true));
        window.click("titlebar-toggle-sidebar", cx);
        assert!(!workspace.read(cx).sidebar_visible());
        window.click("titlebar-toggle-sidebar", cx);
        assert!(workspace.read(cx).sidebar_visible());
        window.click("user-menu", cx);
        window.render_frame(cx);
        let settings = window.find_by_label("Settings…").bounds().center();
        window.pointer_down(settings, MouseButton::Left, cx);
        window.pointer_up(settings, MouseButton::Left, cx);
      })
      .unwrap();
      assert_eq!(rx.recv().await, Some(true));
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sidebar-settings", cx);
      })
      .unwrap();
      assert_eq!(rx.recv().await, Some(true));
      for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          window.click("new-session", cx);
          window.render_frame(cx);
          let scope = window.find_by_label(repo.to_str().unwrap()).bounds().center();
          window.pointer_down(scope, MouseButton::Left, cx);
          window.pointer_up(scope, MouseButton::Left, cx);
        })
        .unwrap();
        assert_eq!(rx.recv().await, Some(false));
      }
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let second = workspace.read(cx).active().unwrap();
        window.click("navigation-back", cx);
        let first = workspace.read(cx).active().unwrap();
        assert_ne!(first, second);
        window.click("navigation-forward", cx);
        assert_eq!(workspace.read(cx).active(), Some(second));
      })
      .unwrap();
      gateway.shutdown().await.unwrap();
      std::fs::remove_dir_all(base).unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn saved_alias_and_hidden_project_can_be_restored(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let mut preferences = ProjectPreferences::new();
      preferences.insert(
        "project".to_owned(),
        ProjectPreference {
          alias: Some("Workbench".to_owned()),
          pinned: true,
          hidden: true,
          ..ProjectPreference::default()
        },
      );
      cx.update(|cx| {
        opened
          .workspace
          .update(cx, |workspace, cx| workspace.set_project_preferences(preferences, cx))
      });
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("Write the docs").is_none());
        let restore = window.find_by_label("Restore Workbench").bounds().center();
        window.pointer_down(restore, MouseButton::Left, cx);
        window.pointer_up(restore, MouseButton::Left, cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Workbench").visible());
        assert!(window.find_by_label("Write the docs").visible());
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn short_sidebar_keeps_footer_accessible_with_long_name_and_hidden_items(cx: &mut TestAppContext) {
    use gpui_kit::{ScrollDelta, point, px, size};

    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let name = "長いユーザー名🙂".repeat(40);
      let mut preferences = ProjectPreferences::new();
      preferences.insert(
        "project".to_owned(),
        ProjectPreference {
          hidden: true,
          hidden_sessions: (0..40).map(|index| format!("session-{index:02}")).collect(),
          ..ProjectPreference::default()
        },
      );
      cx.update(|cx| {
        opened.workspace.update(cx, |workspace, cx| {
          workspace.set_user_name(name, cx);
          workspace.set_project_preferences(preferences, cx);
        });
      });
      cx.simulate_window_resize(opened.handle, size(px(800.), px(400.)));
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        let footer = window.find("sidebar-footer").bounds();
        let user = window.find("user-menu").bounds();
        let mobile = window.find("mobile-coming-soon").bounds();
        let settings = window.find("sidebar-settings").bounds();
        let restore = window.find("hidden-restore-list").bounds();
        assert!(restore.bottom() <= footer.top());
        assert!(footer.bottom() <= px(400.));
        assert!(user.right() <= mobile.left());
        assert!(mobile.right() <= settings.left());
        assert!(settings.right() <= footer.right());
        assert!(window.find("sidebar-settings").visible());
        assert!(window.find("restore-project-project").visible());
        window.scroll("hidden-restore-list", ScrollDelta::Pixels(point(px(0.), px(-10000.))), cx);
        assert!(window.find("restore-session-project-session-39").visible());
        assert!(window.find("sidebar-settings").visible());
      })
      .unwrap();
    });
  }

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
        for label in ["alvin", "Fix the build", "Write the docs"] {
          assert!(window.find_by_label(label).visible(), "{label} is listed");
        }
        let project = window.find_by_label("alvin").bounds().center();
        window.pointer_move(project, None, cx);
        window.render_frame(cx);
        assert!(window.find_by_label("New session: alvin").visible());
        assert!(window.find_by_label("Project menu: alvin").visible());
        assert!(window.find_by_label("New session…").visible());
        let menu = window.find_by_label("Project menu: alvin").bounds().center();
        window.pointer_down(menu, MouseButton::Left, cx);
        window.pointer_up(menu, MouseButton::Left, cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Edit project alias…").visible());
        let session = window.find_by_label("Write the docs").bounds().center();
        window.pointer_move(session, None, cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Project menu: alvin").visible());
        assert!(window.find_by_label("Edit project alias…").visible());
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
      let (mut first, mut second) = (live_session("/work/api"), live_session("/work/web"));
      first.file = "/sessions/first.jsonl".into();
      second.file = "/sessions/second.jsonl".into();
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
        assert_eq!(
          opened.workspace.read(cx).projects.read(cx).active_file(),
          Some(second.file.as_path())
        );

        let tab = window.find_by_label("api").bounds().center();
        window.pointer_down(tab, MouseButton::Left, cx);
        window.pointer_up(tab, MouseButton::Left, cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(first.live));
        assert_eq!(
          opened.workspace.read(cx).projects.read(cx).active_file(),
          Some(first.file.as_path())
        );
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
  async fn closing_active_tab_after_back_keeps_forward_tabs_and_history(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let sessions = [
        live_session("/work/a"),
        live_session("/work/b"),
        live_session("/work/c"),
        live_session("/work/d"),
      ];
      for session in &sessions {
        cx.update(|cx| opened.model.update(cx, |model, cx| model.seed_open(session, cx)));
      }
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("navigation-back", cx);
        window.click("navigation-back", cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(sessions[1].live));
      })
      .unwrap();
      cx.update(|cx| opened.model.update(cx, |model, cx| model.seed_close(sessions[1].live, cx)));
      cx.update_window(opened.handle, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(sessions[0].live));
        assert!(window.find_by_label("c").visible());
        assert!(window.find_by_label("d").visible());
        window.click("navigation-forward", cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(sessions[2].live));
        window.click("navigation-forward", cx);
        assert_eq!(opened.workspace.read(cx).active(), Some(sessions[3].live));
      })
      .unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_sidebar_key_hides_and_shows_the_tree(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let opened = open_workspace(cx).await;
      let key = if cfg!(target_os = "macos") { "cmd-b" } else { "ctrl-b" };
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("alvin").is_some())
        .await;

      cx.update_window(opened.handle, |_, window, cx| {
        window.press(key, cx);
        assert!(!opened.workspace.read(cx).sidebar_visible);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("alvin").is_none())
        .await;

      cx.update_window(opened.handle, |_, window, cx| {
        window.press(key, cx);
        assert!(opened.workspace.read(cx).sidebar_visible);
      })
      .unwrap();
      cx.wait_for(opened.handle, BUDGET, |window, _| window.try_find_by_label("alvin").is_some())
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
