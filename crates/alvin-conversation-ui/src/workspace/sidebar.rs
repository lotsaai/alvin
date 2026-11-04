//! The project tree: projects, their checkouts and the sessions of each, with the commands to open them.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use alvin_conversation::{Run, SessionsKey, Workspace};
use alvin_protocol::{CheckoutId, LiveSessionId, ProjectId, SessionCursor, SessionId};
use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogFooter, DialogHeader, DialogTitle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
  App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _, InteractiveElement as _, IntoElement,
  ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
  prelude::FluentBuilder as _, rems,
};

use crate::model::{Model, ModelChange};
use crate::workspace::ProjectPreferences;

/// What a row of the tree stands for. Rows are found by their item id, which is derived from the domain id behind them.
#[derive(Clone)]
enum Node {
  /// A project: opens and closes.
  Project {
    id: String,
    path: SharedString,
    pinned: bool,
    checkout: Option<CheckoutId>,
  },
  /// A checkout and its new-session scope.
  Checkout {
    path: SharedString,
    project: ProjectId,
    checkout: CheckoutId,
  },
  /// A listed session.
  Session {
    id: SessionId,
    project: String,
    checkout: CheckoutId,
    path: std::path::PathBuf,
    modified: i64,
    pinned: bool,
  },
  Live {
    live: LiveSessionId,
    path: std::path::PathBuf,
    session: Option<SessionId>,
    project: String,
    pinned: bool,
  },
  /// Loads the next page of a checkout's sessions.
  More { key: SessionsKey, after: SessionCursor },
}

impl Node {
  fn is_active(&self, active: Option<&(Option<SessionId>, std::path::PathBuf)>) -> bool {
    match self {
      Self::Session { id, path, .. } => {
        active.is_some_and(|(session, file)| session.as_ref().map_or(path == file, |session| session == id))
      }
      Self::Live { path, .. } => active.is_some_and(|(_, file)| path == file),
      _ => false,
    }
  }
}

/// The tree view over the projects and session lists the model holds.
pub(crate) struct ProjectTree {
  model: Entity<Model>,
  tree: Entity<TreeState>,
  nodes: Rc<BTreeMap<SharedString, Node>>,
  active: Option<(Option<SessionId>, std::path::PathBuf)>,
  active_live: Option<LiveSessionId>,
  preferences: ProjectPreferences,
  search: Entity<InputState>,
  search_trigger: FocusHandle,
  grouping: Grouping,
  loading_all: bool,
  load_failed: bool,
  open_menu: Option<String>,
  /// What the tree was last built from, so a model change that leaves it alone keeps the rows and their expansion.
  built_from: Option<Workspace>,
  _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Grouping {
  Project,
  Recent,
}

impl EventEmitter<ProjectPreferences> for ProjectTree {}
impl EventEmitter<LiveSessionId> for ProjectTree {}

impl ProjectTree {
  pub(crate) fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let tree = cx.new(|cx| TreeState::new(cx));
    let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search sessions"));
    let subscriptions = vec![
      cx.subscribe(&model, |this, _, change: &ModelChange, cx| match change {
        ModelChange::Workspace => this.sync(cx),
        ModelChange::SessionMetadata(_) => {
          this.built_from = None;
          this.sync(cx);
        }
        ModelChange::Connection | ModelChange::Session(_) => {}
      }),
      cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
          this.built_from = None;
          this.sync(cx);
        }
      }),
    ];
    let mut this = Self {
      model,
      tree,
      nodes: Rc::default(),
      active: None,
      active_live: None,
      preferences: ProjectPreferences::new(),
      search,
      search_trigger: cx.focus_handle().tab_stop(true),
      grouping: Grouping::Project,
      loading_all: false,
      load_failed: false,
      open_menu: None,
      built_from: None,
      _subscriptions: subscriptions,
    };
    this.sync(cx);
    this
  }

  fn sync(&mut self, cx: &mut Context<Self>) {
    let query = self.search.read(cx).value().to_lowercase();
    let workspace = self.model.read(cx).conversation().workspace();
    let needs_all = (self.grouping == Grouping::Recent || !query.is_empty())
      && workspace.projects().iter().any(|project| {
        project.checkouts.iter().any(|checkout| {
          let key = SessionsKey {
            project: project.id.clone(),
            checkout: Some(checkout.id.clone()),
          };
          workspace.sessions(&key).is_none_or(|list| list.next.is_some())
        })
      });
    let changed = self.built_from.as_ref() != Some(workspace);
    if changed {
      let (mut items, mut nodes) = items(workspace, &self.preferences, &query, self.grouping);
      merge_live_sessions(
        self.model.read(cx),
        &self.preferences,
        &query,
        self.grouping,
        &mut items,
        &mut nodes,
      );
      self.built_from = Some(workspace.clone());
      self.nodes = Rc::new(nodes);
      self.tree.update(cx, |tree, cx| {
        fn remember(item: &TreeItem, expanded: &mut BTreeMap<SharedString, bool>) {
          expanded.insert(item.id.clone(), item.is_expanded());
          for child in &item.children {
            remember(child, expanded);
          }
        }
        fn restore(items: &mut [TreeItem], expanded: &BTreeMap<SharedString, bool>) {
          for item in items {
            if let Some(value) = expanded.get(&item.id) {
              *item = item.clone().expanded(*value);
            }
            restore(&mut item.children, expanded);
          }
        }
        let mut expanded = BTreeMap::new();
        for entry in (0..).map_while(|ix| tree.entry(ix)) {
          if entry.is_root() {
            remember(entry.item(), &mut expanded);
          }
        }
        restore(&mut items, &expanded);
        let selected = tree.selected_item().cloned();
        tree.set_items(items, cx);
        tree.set_selected_item(selected.as_ref(), cx);
      });
    }
    if needs_all && !self.loading_all && !self.load_failed {
      self.loading_all = true;
      let load = self.model.update(cx, |model, cx| model.load_all_sessions(cx));
      cx.spawn(async move |this, cx| {
        let success = load.await;
        drop(this.update(cx, |view, cx| {
          view.loading_all = false;
          view.load_failed = !success;
          view.sync(cx);
        }));
      })
      .detach();
    }
    cx.notify();
  }

  pub(crate) fn set_active(
    &mut self,
    live: Option<LiveSessionId>,
    active: Option<(Option<SessionId>, std::path::PathBuf)>,
    cx: &mut Context<Self>,
  ) {
    if self.active != active || self.active_live != live {
      self.active_live = live;
      self.active = active;
      cx.notify();
    }
  }

  #[cfg(test)]
  pub(crate) fn active_file(&self) -> Option<&std::path::Path> {
    self.active.as_ref().map(|(_, path)| path.as_path())
  }

  pub(crate) fn set_preferences(&mut self, preferences: ProjectPreferences, cx: &mut Context<Self>) {
    self.preferences = preferences;
    self.built_from = None;
    self.sync(cx);
  }

  fn change_preference(&mut self, id: &str, change: impl FnOnce(&mut crate::workspace::ProjectPreference), cx: &mut Context<Self>) {
    change(self.preferences.entry(id.to_owned()).or_default());
    self.built_from = None;
    self.sync(cx);
    cx.emit(self.preferences.clone());
  }
}

fn item_id(kind: &str, id: &str) -> SharedString {
  SharedString::from(format!("{kind}:{id}"))
}

/// The tree items for the workspace, and what each stands for.
fn items(
  workspace: &Workspace,
  preferences: &ProjectPreferences,
  query: &str,
  grouping: Grouping,
) -> (Vec<TreeItem>, BTreeMap<SharedString, Node>) {
  let mut nodes = BTreeMap::new();
  let mut projects = Vec::new();
  let mut ordered: Vec<_> = workspace.projects().iter().collect();
  let mut recent = Vec::new();
  let mut more = Vec::new();
  ordered.sort_by_key(|project| !preferences.get(project.id.as_str()).is_some_and(|pref| pref.pinned));
  for project in ordered {
    let preference = preferences.get(project.id.as_str());
    if preference.is_some_and(|pref| pref.hidden) {
      continue;
    }
    let mut checkouts = Vec::new();
    for checkout in &project.checkouts {
      let key = SessionsKey {
        project: project.id.clone(),
        checkout: Some(checkout.id.clone()),
      };
      let list = workspace.sessions(&key);
      let mut children = Vec::new();

      let mut sessions: Vec<_> = list.iter().flat_map(|list| &list.sessions).collect();
      sessions.sort_by_key(|session| !preference.is_some_and(|pref| pref.pinned_sessions.contains(session.id.as_str())));
      for session in sessions {
        if preference.is_some_and(|pref| pref.hidden_sessions.contains(session.id.as_str())) {
          continue;
        }
        let title = session.title.clone().unwrap_or_else(|| session.id.as_str().to_owned());
        if !query.is_empty() && !title.to_lowercase().contains(query) {
          continue;
        }
        let id = item_id("session", session.id.as_str());
        let pinned = preference.is_some_and(|pref| pref.pinned_sessions.contains(session.id.as_str()));
        nodes.insert(
          id.clone(),
          Node::Session {
            id: session.id.clone(),
            project: project.id.as_str().to_owned(),
            checkout: checkout.id.clone(),
            path: session.path.clone(),
            modified: session.modified.as_second(),
            pinned,
          },
        );
        let item = TreeItem::new(id, title);
        if grouping == Grouping::Recent {
          recent.push((pinned, session.modified, item));
        } else {
          children.push(item);
        }
      }

      if let Some(after) = list.and_then(|list| list.next.clone()) {
        let id = item_id("more", checkout.id.as_str());
        nodes.insert(id.clone(), Node::More { key, after });
        let item = TreeItem::new(id, "More sessions");
        if grouping == Grouping::Recent {
          more.push(item);
        } else {
          children.push(item);
        }
      }
      if grouping == Grouping::Project && !query.is_empty() && children.is_empty() {
        continue;
      }

      if project.checkouts.len() == 1 && checkout.root == project.root {
        checkouts.extend(children);
      } else {
        let id = item_id("checkout", checkout.id.as_str());
        let path = checkout.root.display().to_string();
        let name = checkout
          .root
          .file_name()
          .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
        nodes.insert(
          id.clone(),
          Node::Checkout {
            path: path.into(),
            project: project.id.clone(),
            checkout: checkout.id.clone(),
          },
        );
        checkouts.push(TreeItem::new(id, name).expanded(true).children(children));
      }
    }
    if grouping == Grouping::Project && !query.is_empty() && checkouts.is_empty() {
      continue;
    }
    let id = item_id("project", project.id.as_str());
    nodes.insert(
      id.clone(),
      Node::Project {
        id: project.id.as_str().to_owned(),
        path: project.root.display().to_string().into(),
        pinned: preference.is_some_and(|pref| pref.pinned),
        checkout: project
          .checkouts
          .first()
          .filter(|checkout| project.checkouts.len() == 1 && checkout.root == project.root)
          .map(|checkout| checkout.id.clone()),
      },
    );
    let name = preference.and_then(|pref| pref.alias.clone()).unwrap_or_else(|| {
      project
        .root
        .file_name()
        .map_or_else(|| project.root.display().to_string(), |name| name.to_string_lossy().into_owned())
    });
    projects.push(TreeItem::new(id, name).expanded(true).children(checkouts));
  }
  if grouping == Grouping::Recent {
    recent.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    projects = recent.into_iter().map(|(_, _, item)| item).collect();
    projects.extend(more);
  }
  (projects, nodes)
}

fn merge_live_sessions(
  model: &Model,
  preferences: &ProjectPreferences,
  query: &str,
  grouping: Grouping,
  items: &mut Vec<TreeItem>,
  nodes: &mut BTreeMap<SharedString, Node>,
) {
  for live in model.conversation().live_sessions() {
    let Some((project, checkout)) = model.session_checkout(live.live) else {
      continue;
    };
    let Some(summary) = model.conversation().workspace().projects().iter().find(|item| item.id == *project) else {
      continue;
    };
    let Some(scope) = summary.checkouts.iter().find(|item| item.id == *checkout) else {
      continue;
    };
    let preference = preferences.get(project.as_str());
    if preference.is_some_and(|pref| pref.hidden) {
      continue;
    }
    let session = model.session_id(live.live);
    if session.is_some_and(|id| preference.is_some_and(|pref| pref.hidden_sessions.contains(id.as_str()))) {
      continue;
    }
    let title = live.title.clone().unwrap_or_else(|| {
      live
        .cwd
        .file_name()
        .map_or_else(|| "session".to_owned(), |name| name.to_string_lossy().into_owned())
    });
    let indexed = nodes.iter().find_map(|(item_id, node)| match node {
      Node::Session { id, path, .. } if session == Some(id) || (!live.file.as_os_str().is_empty() && *path == live.file) => {
        Some(item_id.clone())
      }
      _ => None,
    });
    if let Some(id) = indexed {
      fn update(items: &mut [TreeItem], id: &SharedString, title: &str) {
        for item in items {
          if item.id == *id {
            item.label = SharedString::from(title.to_owned());
          } else {
            update(&mut item.children, id, title);
          }
        }
      }
      if let Some(title) = &live.title {
        if !query.is_empty() && !title.to_lowercase().contains(query) {
          fn remove(items: &mut Vec<TreeItem>, id: &SharedString) {
            items.retain(|item| item.id != *id);
            for item in items {
              remove(&mut item.children, id);
            }
          }
          remove(items, &id);
          nodes.remove(&id);
        } else {
          update(items, &id, title);
        }
      }
      continue;
    }
    if !query.is_empty() && !title.to_lowercase().contains(query) {
      continue;
    }
    let id = item_id("live", &live.live.as_uuid().to_string());
    let pinned = session.is_some_and(|id| preference.is_some_and(|pref| pref.pinned_sessions.contains(id.as_str())));
    nodes.insert(
      id.clone(),
      Node::Live {
        live: live.live,
        path: live.file.clone(),
        session: session.cloned(),
        project: project.as_str().to_owned(),
        pinned,
      },
    );
    let item = TreeItem::new(id, title);
    if grouping == Grouping::Recent {
      items.insert(0, item);
    } else {
      let project_id = item_id("project", project.as_str());
      if !items.iter().any(|item| item.id == project_id) {
        let name = preference.and_then(|pref| pref.alias.clone()).unwrap_or_else(|| {
          summary
            .root
            .file_name()
            .map_or_else(|| summary.root.display().to_string(), |name| name.to_string_lossy().into_owned())
        });
        nodes.insert(
          project_id.clone(),
          Node::Project {
            id: project.as_str().to_owned(),
            path: summary.root.display().to_string().into(),
            pinned: preference.is_some_and(|pref| pref.pinned),
            checkout: (summary.checkouts.len() == 1 && scope.root == summary.root).then(|| checkout.clone()),
          },
        );
        items.push(TreeItem::new(project_id.clone(), name).expanded(true));
      }
      if let Some(project_item) = items.iter_mut().find(|item| item.id == project_id) {
        if summary.checkouts.len() == 1 && scope.root == summary.root {
          project_item.children.insert(0, item);
        } else {
          let checkout_id = item_id("checkout", checkout.as_str());
          if !project_item.children.iter().any(|item| item.id == checkout_id) {
            let path = scope.root.display().to_string();
            let name = scope
              .root
              .file_name()
              .map_or_else(|| path.clone(), |name| name.to_string_lossy().into_owned());
            nodes.insert(
              checkout_id.clone(),
              Node::Checkout {
                path: path.into(),
                project: project.clone(),
                checkout: checkout.clone(),
              },
            );
            project_item.children.push(TreeItem::new(checkout_id.clone(), name).expanded(true));
          }
          if let Some(checkout_item) = project_item.children.iter_mut().find(|item| item.id == checkout_id) {
            checkout_item.children.insert(0, item);
          }
        }
      }
    }
  }
}

fn open_alias(view: &Entity<ProjectTree>, id: String, window: &mut Window, cx: &mut App) {
  let current = view
    .read(cx)
    .preferences
    .get(&id)
    .and_then(|pref| pref.alias.clone())
    .unwrap_or_default();
  let input = cx.new(|cx| {
    let mut input = InputState::new(window, cx).placeholder("Use folder name");
    input.set_value(current, window, cx);
    input
  });
  let view = view.clone();
  let initial_focus = Cell::new(true);
  window.open_dialog(cx, move |dialog, window, cx| {
    if initial_focus.replace(false) {
      input.focus_handle(cx).focus(window, cx);
    }
    let (view, input, id) = (view.clone(), input.clone(), id.clone());
    dialog.content(move |content, _, _| {
      let (view, input, id) = (view.clone(), input.clone(), id.clone());
      content
        .child(DialogHeader::new().child(DialogTitle::new().child("Edit project alias")))
        .child(Input::new(&input).aria_label("Project alias"))
        .child(
          DialogFooter::new()
            .child(
              Button::new("alias-cancel")
                .outline()
                .label("Cancel")
                .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(Button::new("alias-save").primary().label("Save").on_click(move |_, window, cx| {
              let text = input.read(cx).value().trim().to_owned();
              view.update(cx, |view, cx| {
                view.change_preference(&id, |pref| pref.alias = (!text.is_empty()).then_some(text), cx)
              });
              window.close_dialog(cx);
            })),
        )
    })
  });
}

fn project_menu(
  view: Entity<ProjectTree>,
  id: String,
  name: SharedString,
  pinned: bool,
  open: bool,
  window: &mut Window,
  cx: &mut App,
) -> impl IntoElement {
  let menu_view = view.clone();
  let menu_id = id.clone();
  let button_id = SharedString::from(format!("project-menu-{id}"));
  let focus = window
    .use_keyed_state(button_id.clone(), cx, |_, cx| cx.focus_handle().tab_stop(true))
    .read(cx)
    .clone();
  div()
    .id(SharedString::from(format!("project-menu-wrapper-{id}")))
    .on_click(|_, _, cx| cx.stop_propagation())
    .child(
      Button::new(button_id)
        .track_focus(&focus)
        .xsmall()
        .ghost()
        .icon(IconName::Ellipsis)
        .accessibility_label(SharedString::from(format!("Project menu: {name}")))
        .tooltip("Project actions")
        .when(!open, |button| {
          button
            .opacity(0.)
            .group_hover("scope-row", |style| style.opacity(1.))
            .focus(|style| style.opacity(1.))
        })
        .dropdown_menu(move |menu, _, _| {
          let (edit_view, edit_id) = (view.clone(), id.clone());
          let (pin_view, pin_id) = (view.clone(), id.clone());
          let (hide_view, hide_id) = (view.clone(), id.clone());
          menu
            .item(PopupMenuItem::new("Edit project alias…").on_click(move |_, window, cx| {
              open_alias(&edit_view, edit_id.clone(), window, cx);
            }))
            .item(PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" }).on_click(move |_, _, cx| {
              pin_view.update(cx, |view, cx| view.change_preference(&pin_id, |pref| pref.pinned = !pinned, cx));
            }))
            .separator()
            .item(PopupMenuItem::new("Hide project").on_click(move |_, _, cx| {
              hide_view.update(cx, |view, cx| view.change_preference(&hide_id, |pref| pref.hidden = true, cx));
            }))
        })
        .on_open_change(move |open, _, cx| {
          menu_view.update(cx, |view, cx| {
            view.open_menu = open.then(|| menu_id.clone());
            cx.notify();
          });
        }),
    )
}
fn relative_age(modified: i64, now: i64) -> String {
  let seconds = now.saturating_sub(modified).max(0);
  match seconds {
    0..=59 => "now".to_owned(),
    60..=3599 => format!("{}m", seconds / 60),
    3600..=86399 => format!("{}h", seconds / 3600),
    _ => format!("{}d", seconds / 86400),
  }
}

fn session_actions(view: Entity<ProjectTree>, project: String, session: String, name: SharedString, pinned: bool) -> impl IntoElement {
  let (pin_view, pin_project, pin_session) = (view.clone(), project.clone(), session.clone());
  let pin_label = if pinned { "Unpin session" } else { "Pin session" };
  h_flex()
    .id(SharedString::from(format!("session-actions-{session}")))
    .on_click(|_, _, cx| cx.stop_propagation())
    .child(
      Button::new(SharedString::from(format!("pin-session-{session}")))
        .xsmall()
        .ghost()
        .icon(IconName::Star)
        .accessibility_label(SharedString::from(format!("{pin_label}: {name}")))
        .tooltip(pin_label)
        .opacity(0.)
        .group_hover("scope-row", |style| style.opacity(1.))
        .focus(|style| style.opacity(1.))
        .on_click(move |_, _, cx| {
          cx.stop_propagation();
          pin_view.update(cx, |view, cx| {
            view.change_preference(
              &pin_project,
              |pref| {
                if pinned {
                  pref.pinned_sessions.remove(&pin_session);
                } else {
                  pref.pinned_sessions.insert(pin_session.clone());
                }
              },
              cx,
            )
          });
        }),
    )
    .child(
      Button::new(SharedString::from(format!("hide-session-{session}")))
        .xsmall()
        .ghost()
        .icon(IconName::EyeOff)
        .accessibility_label(SharedString::from(format!("Hide session: {name}")))
        .tooltip("Hide session")
        .opacity(0.)
        .group_hover("scope-row", |style| style.opacity(1.))
        .focus(|style| style.opacity(1.))
        .on_click(move |_, _, cx| {
          cx.stop_propagation();
          view.update(cx, |view, cx| {
            view.change_preference(
              &project,
              |pref| {
                pref.hidden_sessions.insert(session.clone());
              },
              cx,
            )
          });
        }),
    )
}

impl Render for ProjectTree {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let view = cx.entity();
    let hidden: Vec<_> = self
      .model
      .read(cx)
      .conversation()
      .workspace()
      .projects()
      .iter()
      .filter(|project| self.preferences.get(project.id.as_str()).is_some_and(|pref| pref.hidden))
      .map(|project| {
        let id = project.id.as_str().to_owned();
        let name = self.preferences.get(&id).and_then(|pref| pref.alias.clone()).unwrap_or_else(|| {
          project
            .root
            .file_name()
            .map_or_else(|| project.root.display().to_string(), |name| name.to_string_lossy().into_owned())
        });
        (id, name)
      })
      .collect();
    let workspace = self.model.read(cx).conversation().workspace();
    let hidden_sessions: Vec<_> = self
      .preferences
      .iter()
      .flat_map(|(project, pref)| {
        pref.hidden_sessions.iter().map(move |session| {
          let title = workspace
            .projects()
            .iter()
            .find(|summary| summary.id.as_str() == project.as_str())
            .and_then(|summary| {
              summary.checkouts.iter().find_map(|checkout| {
                let key = SessionsKey {
                  project: summary.id.clone(),
                  checkout: Some(checkout.id.clone()),
                };
                workspace.sessions(&key).and_then(|list| {
                  list
                    .sessions
                    .iter()
                    .find(|item| item.id.as_str() == session.as_str())
                    .and_then(|item| item.title.clone())
                })
              })
            })
            .unwrap_or_else(|| session.clone());
          (project.clone(), session.clone(), title)
        })
      })
      .collect();
    let grouping = self.grouping;
    let grouping_view = view.clone();
    let now = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map_or(0, |duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX));
    let model = self.model.clone();
    let nodes = self.nodes.clone();
    let active_session = self.active.clone();
    let active_live = self.active_live;
    let open_menu = self.open_menu.clone();
    let complete_results = grouping == Grouping::Recent || !self.search.read(cx).value().is_empty();
    let blocked = complete_results && (self.loading_all || self.load_failed);
    v_flex()
      .size_full()
      .child(
        h_flex()
          .px_3()
          .py_2()
          .text_sm()
          .text_color(cx.theme().muted_foreground)
          .child(
            Button::new("focus-sessions")
              .ghost()
              .small()
              .label("Sessions")
              .on_click(cx.listener(|this, _, window, cx| this.tree.update(cx, |tree, cx| tree.focus(window, cx)))),
          )
          .child(div().flex_1().min_w_0())
          .child(
            Popover::new("session-search-popover")
              .track_focus(&self.search.read(cx).focus_handle(cx))
              .anchor(gpui_kit::Anchor::TopRight)
              .on_open_change(cx.listener(|this, open: &bool, window, cx| {
                if !open {
                  this.search.update(cx, |search, cx| search.set_value("", window, cx));
                  this.built_from = None;
                  this.sync(cx);
                  this.search_trigger.focus(window, cx);
                }
              }))
              .trigger(
                Button::new("session-search")
                  .ghost()
                  .xsmall()
                  .icon(IconName::Search)
                  .accessibility_label("Search sessions…")
                  .tooltip("Search sessions")
                  .track_focus(&self.search_trigger),
              )
              .child(Input::new(&self.search).aria_label("Search sessions").w_48()),
          )
          .child(
            Button::new("session-grouping")
              .xsmall()
              .ghost()
              .icon(IconName::Ellipsis)
              .accessibility_label("Session grouping")
              .tooltip("Session grouping")
              .dropdown_menu(move |menu, _, _| {
                let project_view = grouping_view.clone();
                let recent_view = grouping_view.clone();
                menu
                  .item(
                    PopupMenuItem::new("Project and worktree")
                      .icon(IconName::Folder)
                      .checked(grouping == Grouping::Project)
                      .on_click(move |_, _, cx| {
                        project_view.update(cx, |view, cx| {
                          view.grouping = Grouping::Project;
                          view.built_from = None;
                          view.sync(cx);
                        });
                      }),
                  )
                  .item(
                    PopupMenuItem::new("Recently updated")
                      .icon(IconName::SortDescending)
                      .checked(grouping == Grouping::Recent)
                      .on_click(move |_, _, cx| {
                        recent_view.update(cx, |view, cx| {
                          view.grouping = Grouping::Recent;
                          view.built_from = None;
                          view.sync(cx);
                        });
                      }),
                  )
              }),
          ),
      )
      .when(blocked && self.loading_all, |sidebar| {
        sidebar.child(h_flex().px_3().gap_2().child(Spinner::new().small()).child("Loading sessions…"))
      })
      .when(blocked && self.load_failed, |sidebar| {
        sidebar.child(
          v_flex().px_3().gap_2().child("Couldn’t load all sessions").child(
            Button::new("retry-session-pages")
              .small()
              .label("Retry")
              .on_click(cx.listener(|this, _, _, cx| {
                this.load_failed = false;
                this.sync(cx);
              })),
          ),
        )
      })
      .when(
        !blocked
          && (self.nodes.is_empty()
            || (complete_results
              && !self
                .nodes
                .values()
                .any(|node| matches!(node, Node::Session { .. } | Node::Live { .. })))),
        |sidebar| {
          sidebar.child(div().px_3().pt_3().text_sm().text_color(cx.theme().muted_foreground).child(
            if !self.search.read(cx).value().is_empty() {
              "No matching sessions"
            } else if hidden.is_empty() {
              "No projects yet"
            } else {
              "All projects are hidden"
            },
          ))
        },
      )
      .when(!blocked, |sidebar| {
        sidebar.child(
          div()
            .flex_1()
            .min_h_0()
            .on_key_down(cx.listener(|this, event: &gpui_kit::KeyDownEvent, _, cx| {
              if event.keystroke.key != "enter" || event.keystroke.modifiers.modified() {
                return;
              }
              let node = this
                .tree
                .read(cx)
                .selected_item()
                .and_then(|item| this.nodes.get(&item.id))
                .cloned();
              match node {
                Some(Node::Session { id, project, checkout, .. }) => {
                  this
                    .model
                    .update(cx, |model, cx| model.open_session(id, ProjectId::from(project), checkout, cx))
                    .detach();
                  cx.stop_propagation();
                }
                Some(Node::Live { live, .. }) => {
                  cx.emit(live);
                  cx.stop_propagation();
                }
                Some(Node::More { key, after }) => {
                  this
                    .model
                    .update(cx, |model, cx| model.load_sessions(key, Some(after), cx))
                    .detach();
                  cx.stop_propagation();
                }
                _ => {}
              }
            }))
            .child(tree(&self.tree, move |_, entry, selected, window, cx| {
              let item = entry.item();
              let node = nodes.get(&item.id);
              let scope = matches!(node, Some(Node::Project { .. } | Node::Checkout { .. }));
              let icon = match node {
                Some(Node::Session { .. } | Node::Live { .. }) => IconName::Ellipsis,
                Some(Node::More { .. }) => IconName::Ellipsis,
                Some(Node::Checkout { .. }) => IconName::GitFork,
                Some(Node::Project { .. }) | None if entry.is_expanded() => IconName::FolderOpen,
                Some(Node::Project { .. }) | None => IconName::Folder,
              };
              let label = match node {
                Some(Node::Project { path, .. } | Node::Checkout { path, .. }) => path.clone(),
                _ => item.label.clone(),
              };
              let target = match node {
                Some(Node::Project {
                  id,
                  checkout: Some(checkout),
                  ..
                }) => Some((ProjectId::from(id.clone()), checkout.clone())),
                Some(Node::Checkout { project, checkout, .. }) => Some((project.clone(), checkout.clone())),
                _ => None,
              };
              let menu_open = matches!(node, Some(Node::Project { id, .. }) if open_menu.as_ref() == Some(id));
              let running = match node {
                Some(Node::Session { path, .. }) => model
                  .read(cx)
                  .conversation()
                  .live_sessions()
                  .any(|live| &live.file == path && !live.exited && live.run == Run::Running),
                Some(Node::Live { live, .. }) => model
                  .read(cx)
                  .conversation()
                  .live(*live)
                  .is_some_and(|view| !view.exited && view.run == Run::Running),
                _ => false,
              };
              let active = match node {
                Some(Node::Live { live, .. }) => active_live == Some(*live),
                _ => node.is_some_and(|node| node.is_active(active_session.as_ref())),
              };
              let row = ListItem::new(item.id.clone())
                .group("scope-row")
                .when(active, |row| row.bg(cx.theme().accent))
                .when(selected, |row| row.border_1().border_color(cx.theme().border))
                .text_sm()
                .px_2()
                .rounded(cx.theme().radius)
                .when(!scope && !active, |row| row.text_color(cx.theme().muted_foreground))
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
                .child(
                  h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .pl(rems(0.75) * entry.depth() as f32)
                    .when(!matches!(node, Some(Node::Session { .. } | Node::Live { .. })), |row| {
                      row.child(Icon::new(icon).small())
                    })
                    .child(
                      div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(scope, |this| this.font_semibold())
                        .child(item.label.clone()),
                    )
                    .when(matches!(node, Some(Node::Project { pinned: true, .. })), |row| row.child("Pinned"))
                    .when(
                      matches!(node, Some(Node::Session { pinned: true, .. } | Node::Live { pinned: true, .. })),
                      |row| row.child(Icon::new(IconName::Star).xsmall()),
                    )
                    .when_some(
                      match node {
                        Some(Node::Session { modified, .. }) => Some(relative_age(*modified, now)),
                        _ => None,
                      },
                      |row, age| row.child(div().text_xs().text_color(cx.theme().muted_foreground).child(age)),
                    )
                    .when(running, |row| row.child(Spinner::new().xsmall()))
                    .when_some(
                      match node {
                        Some(Node::Session { id, project, pinned, .. }) => Some((id.as_str().to_owned(), project.clone(), *pinned)),
                        Some(Node::Live {
                          session: Some(id),
                          project,
                          pinned,
                          ..
                        }) => Some((id.as_str().to_owned(), project.clone(), *pinned)),
                        _ => None,
                      },
                      |row, (id, project, pinned)| row.child(session_actions(view.clone(), project, id, item.label.clone(), pinned)),
                    )
                    .when_some(target, |row, (project, checkout)| {
                      let model = model.clone();
                      let button_id = item_id("new", checkout.as_str());
                      let focus = window
                        .use_keyed_state(button_id.clone(), cx, |_, cx| cx.focus_handle().tab_stop(true))
                        .read(cx)
                        .clone();
                      row.child(
                        Button::new(button_id)
                          .track_focus(&focus)
                          .xsmall()
                          .ghost()
                          .icon(IconName::Plus)
                          .accessibility_label(SharedString::from(format!("New session: {}", item.label)))
                          .tooltip("New session")
                          .when(!menu_open, |button| {
                            button
                              .opacity(0.)
                              .group_hover("scope-row", |style| style.opacity(1.))
                              .focus(|style| style.opacity(1.))
                          })
                          .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            model
                              .update(cx, |model, cx| model.new_session(project.clone(), checkout.clone(), cx))
                              .detach();
                          }),
                      )
                    })
                    .when_some(
                      match node {
                        Some(Node::Project { id, pinned, .. }) => Some((id.clone(), *pinned)),
                        _ => None,
                      },
                      |row, (id, pinned)| row.child(project_menu(view.clone(), id, item.label.clone(), pinned, menu_open, window, cx)),
                    ),
                );
              let model = model.clone();
              match node.cloned() {
                Some(Node::Session { id, project, checkout, .. }) => row.on_click(move |_, _, cx| {
                  model
                    .update(cx, |model, cx| {
                      model.open_session(id.clone(), ProjectId::from(project.clone()), checkout.clone(), cx)
                    })
                    .detach();
                }),
                Some(Node::Live { live, .. }) => {
                  let view = view.clone();
                  row.on_click(move |_, _, cx| view.update(cx, |_, cx| cx.emit(live)))
                }
                Some(Node::More { key, after }) => row.on_click(move |_, _, cx| {
                  model
                    .update(cx, |model, cx| model.load_sessions(key.clone(), Some(after.clone()), cx))
                    .detach();
                }),
                Some(Node::Project { .. } | Node::Checkout { .. }) | None => row,
              }
            })),
        )
      })
      .when(!hidden.is_empty() || !hidden_sessions.is_empty(), |sidebar| {
        sidebar.child(
          v_flex()
            .id("hidden-restore-list")
            .test_support()
            .flex_1()
            .min_h_0()
            .when(!hidden.is_empty(), |sidebar| {
              sidebar.child(
                v_flex()
                  .p_2()
                  .gap_1()
                  .border_t_1()
                  .border_color(cx.theme().border)
                  .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Hidden projects"))
                  .children(hidden.into_iter().map(|(id, name)| {
                    let view = cx.entity();
                    Button::new(SharedString::from(format!("restore-project-{id}")))
                      .small()
                      .ghost()
                      .label(SharedString::from(format!("Restore {name}")))
                      .on_click(move |_, _, cx| {
                        view.update(cx, |view, cx| {
                          view.change_preference(&id, |pref| pref.hidden = false, cx);
                        })
                      })
                  })),
              )
            })
            .when(!hidden_sessions.is_empty(), |sidebar| {
              sidebar.child(
                v_flex()
                  .p_2()
                  .gap_1()
                  .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Hidden sessions"))
                  .children(hidden_sessions.into_iter().map(|(project, session, title)| {
                    let view = cx.entity();
                    Button::new(SharedString::from(format!("restore-session-{project}-{session}")))
                      .small()
                      .ghost()
                      .label(SharedString::from(format!("Restore {title}")))
                      .on_click(move |_, _, cx| {
                        view.update(cx, |view, cx| {
                          view.change_preference(
                            &project,
                            |pref| {
                              pref.hidden_sessions.remove(&session);
                            },
                            cx,
                          )
                        });
                      })
                  })),
              )
            })
            .overflow_y_scroll(),
        )
      })
      .into_any_element()
  }
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use alvin_conversation::{SessionsKey, Workspace};
  use alvin_protocol::{CheckoutId, CheckoutSummary, ProjectId, ProjectSummary, SessionId, SessionPage};

  use super::{Grouping, Node, ProjectTree, item_id, items, relative_age};
  use crate::test_support::session_summary;
  use crate::workspace::{ProjectPreference, ProjectPreferences};
  use alvin_client::{ChannelPair, Client, Endpoint};
  use alvin_gateway::{Gateway, GatewayConfig, HostConfig};
  use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
  use gpui_kit::{AppContext as _, Bounds, TestAppContext, WindowBounds, WindowOptions, px, size};

  use crate::model::Model;

  #[test]
  fn active_session_uses_session_id_or_new_session_file() {
    let node = Node::Session {
      id: SessionId::from("one".to_owned()),
      project: "project".to_owned(),
      checkout: CheckoutId::from("checkout".to_owned()),
      path: PathBuf::from("/sessions/one.jsonl"),
      modified: 0,
      pinned: false,
    };
    let listed = Some((Some(SessionId::from("one".to_owned())), PathBuf::from("/canonical/one.jsonl")));
    assert!(node.is_active(listed.as_ref()));
    let different = Some((Some(SessionId::from("two".to_owned())), PathBuf::from("/sessions/one.jsonl")));
    assert!(!node.is_active(different.as_ref()));
    let new_session = Some((None, PathBuf::from("/sessions/one.jsonl")));
    assert!(node.is_active(new_session.as_ref()));
    assert!(!node.is_active(None));
  }

  #[test]
  fn new_session_scope_follows_the_checkout_hierarchy() {
    let mut workspace = Workspace::default();
    let project = ProjectId::from("project".to_owned());
    let checkout = |id: &str, root: &str| CheckoutSummary {
      id: CheckoutId::from(id.to_owned()),
      root: PathBuf::from(root),
      vcs: None,
    };
    let summary = |checkouts| ProjectSummary {
      id: project.clone(),
      root: PathBuf::from("/work/repo"),
      checkouts,
    };

    workspace.set_projects(vec![summary(vec![checkout("root", "/work/repo")])]);
    let (tree, nodes) = items(&workspace, &ProjectPreferences::new(), "", Grouping::Project);
    assert!(tree[0].children.is_empty());
    assert!(matches!(nodes.get(&item_id("project", "project")), Some(Node::Project { checkout: Some(id), .. }) if id.as_str() == "root"));

    workspace.set_projects(vec![summary(vec![checkout("branch", "/work/branch")])]);
    let (tree, nodes) = items(&workspace, &ProjectPreferences::new(), "", Grouping::Project);
    assert_eq!(tree[0].children.len(), 1);
    assert!(matches!(
      nodes.get(&item_id("project", "project")),
      Some(Node::Project { checkout: None, .. })
    ));
    assert!(matches!(nodes.get(&item_id("checkout", "branch")), Some(Node::Checkout { checkout, .. }) if checkout.as_str() == "branch"));

    workspace.set_projects(vec![summary(vec![
      checkout("root", "/work/repo"),
      checkout("branch", "/work/branch"),
    ])]);
    let (tree, nodes) = items(&workspace, &ProjectPreferences::new(), "", Grouping::Project);
    assert_eq!(tree[0].children.len(), 2);
    assert!(matches!(
      nodes.get(&item_id("project", "project")),
      Some(Node::Project { checkout: None, .. })
    ));
    for id in ["root", "branch"] {
      assert!(
        matches!(nodes.get(&item_id("checkout", id)), Some(Node::Checkout { project: p, checkout, .. }) if p == &project && checkout.as_str() == id)
      );
    }
  }

  #[test]
  fn preferences_sort_alias_and_hide_without_changing_projects() {
    let mut workspace = Workspace::default();
    workspace.set_projects(
      ["first", "second", "third"]
        .into_iter()
        .map(|id| ProjectSummary {
          id: ProjectId::from(id.to_owned()),
          root: PathBuf::from(format!("/work/{id}")),
          checkouts: Vec::new(),
        })
        .collect(),
    );
    let mut preferences = ProjectPreferences::new();
    preferences.insert(
      "second".to_owned(),
      ProjectPreference {
        alias: Some("Renamed".to_owned()),
        pinned: true,
        hidden: false,
        ..ProjectPreference::default()
      },
    );
    preferences.insert(
      "third".to_owned(),
      ProjectPreference {
        hidden: true,
        ..ProjectPreference::default()
      },
    );
    let (projects, _) = items(&workspace, &preferences, "", Grouping::Project);
    assert_eq!(
      projects.iter().map(|item| item.label.as_ref()).collect::<Vec<_>>(),
      ["Renamed", "first"]
    );
    assert_eq!(workspace.projects().len(), 3);
    preferences.get_mut("third").unwrap().hidden = false;
    assert_eq!(items(&workspace, &preferences, "", Grouping::Project).0.len(), 3);
  }

  #[test]
  fn session_preferences_search_and_grouping_preserve_history() {
    let project = ProjectId::from("project".to_owned());
    let checkout = CheckoutId::from("checkout".to_owned());
    let mut workspace = Workspace::default();
    workspace.set_projects(vec![ProjectSummary {
      id: project.clone(),
      root: PathBuf::from("/work/repo"),
      checkouts: vec![CheckoutSummary {
        id: checkout.clone(),
        root: PathBuf::from("/work/repo"),
        vcs: None,
      }],
    }]);
    workspace.load_sessions(
      SessionsKey {
        project,
        checkout: Some(checkout),
      },
      None,
      SessionPage {
        sessions: vec![session_summary("one", "Fix build"), session_summary("two", "Write docs")],
        next: None,
      },
    );
    let mut preferences = ProjectPreferences::new();
    let pref = preferences.entry("project".to_owned()).or_default();
    pref.pinned_sessions.insert("two".to_owned());
    let (tree, _) = items(&workspace, &preferences, "", Grouping::Project);
    assert_eq!(tree[0].children[0].label.as_ref(), "Write docs");
    preferences.get_mut("project").unwrap().hidden_sessions.insert("two".to_owned());
    let (tree, _) = items(&workspace, &preferences, "", Grouping::Recent);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].label.as_ref(), "Fix build");
    preferences.get_mut("project").unwrap().hidden_sessions.remove("two");
    let (tree, _) = items(&workspace, &preferences, "docs", Grouping::Recent);
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].label.as_ref(), "Write docs");
    assert_eq!(
      workspace
        .sessions(&SessionsKey {
          project: ProjectId::from("project".to_owned()),
          checkout: Some(CheckoutId::from("checkout".to_owned())),
        })
        .unwrap()
        .sessions
        .len(),
      2
    );
  }

  #[test]
  fn age_uses_elapsed_time_and_clamps_future_timestamps() {
    assert_eq!(relative_age(100, 99), "now");
    assert_eq!(relative_age(0, 60), "1m");
    assert_eq!(relative_age(0, 3600), "1h");
    assert_eq!(relative_age(0, 86400), "1d");
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn session_actions_and_search_use_real_controls(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let dir = std::env::temp_dir().join(format!("alvin-sidebar-{}", alvin_protocol::EventId::new().as_uuid()));
      std::fs::create_dir_all(&dir).unwrap();
      let gateway = Gateway::new(
        GatewayConfig::builder()
          .sessions_dir(dir.clone())
          .omp(HostConfig::builder().omp(PathBuf::from("omp")).build())
          .build(),
      );
      let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64).unwrap())))
        .await
        .unwrap();
      let project = ProjectId::from("project".to_owned());
      let checkout = CheckoutId::from("checkout".to_owned());
      let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
        let model = cx.new(|cx| {
          let mut model = Model::new(client, cx);
          model.seed_workspace(
            vec![ProjectSummary {
              id: project.clone(),
              root: PathBuf::from("/work/repo"),
              checkouts: vec![CheckoutSummary {
                id: checkout.clone(),
                root: PathBuf::from("/work/repo"),
                vcs: None,
              }],
            }],
            vec![(
              SessionsKey {
                project: project.clone(),
                checkout: Some(checkout),
              },
              SessionPage {
                sessions: vec![session_summary("one", "Fix build"), session_summary("two", "Write docs")],
                next: None,
              },
            )],
            cx,
          );
          model
        });
        gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1000.), px(700.))))),
            ..Default::default()
          },
          cx,
          |window, cx| cx.new(|cx| ProjectTree::new(model, window, cx)),
        )
        .unwrap()
      });
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("focus-sessions", cx);
        window.press("down", cx);
        let selected = view.read(cx).tree.read(cx).selected_item().unwrap().id.clone();
        let other = window.find_by_label("Write docs").bounds().center();
        window.pointer_move(other, None, cx);
        assert_eq!(view.read(cx).tree.read(cx).selected_item().unwrap().id, selected);
        let row = window.find_by_label("Fix build").bounds().center();
        window.pointer_move(row, None, cx);
        window.click("pin-session-one", cx);
        window.render_frame(cx);
        assert!(view.read(cx).preferences["project"].pinned_sessions.contains("one"));
        let row = window.find_by_label("Fix build").bounds().center();
        window.pointer_move(row, None, cx);
        window.click("hide-session-one", cx);
        window.render_frame(cx);
        assert!(view.read(cx).preferences["project"].hidden_sessions.contains("one"));
        assert!(window.try_find_by_label("Fix build").is_none());
        window.click("restore-session-project-one", cx);
        window.render_frame(cx);
        assert!(!view.read(cx).preferences["project"].hidden_sessions.contains("one"));
        assert!(view.read(cx).preferences["project"].pinned_sessions.contains("one"));
        assert!(window.find_by_label("Fix build").visible());
        assert!(window.try_find_by_label("Search sessions").is_none());
        window.click("session-search", cx);
        assert_eq!(window.find_by_label("Search sessions").focused(), Some(true));
        window.input("docs", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("Fix build").is_none());
        assert!(window.find_by_label("Write docs").visible());
        assert_eq!(window.find_by_label("Search sessions").value(), Some("docs"));
        window.press("escape", cx);
        assert!(window.try_find_by_label("Search sessions").is_none());
        assert_eq!(window.find("session-search").focused(), Some(true));
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Fix build").visible());
        assert!(window.find_by_label("Write docs").visible());
      })
      .unwrap();
      std::fs::remove_dir_all(dir).unwrap();
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn remaining_pages_are_loaded_from_the_gateway(cx: &mut TestAppContext) {
    use alvin_protocol::ListSessions;

    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let base = std::env::temp_dir().join(format!("alvin-pages-{}", alvin_protocol::EventId::new().as_uuid()));
      let repo = base.join("repo");
      let sessions = base.join("sessions").join("-repo");
      std::fs::create_dir_all(&repo).unwrap();
      std::fs::create_dir_all(&sessions).unwrap();
      let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap();
      assert!(status.success());
      let repo = repo.canonicalize().unwrap();
      let seed =
        include_str!("../../../alvin-app/fixtures/seed-session.jsonl").replace("/tmp/alvin-probe.hnBA/proj", repo.to_str().unwrap());
      for _ in 0..55 {
        let id = alvin_protocol::EventId::new().as_uuid().to_string();
        let record = seed.replace("01a18ff1-ab74-73b8-a3a2-79905565c49b", &id);
        std::fs::write(sessions.join(format!("2026-01-01T00-00-00-000Z_{id}.jsonl")), record).unwrap();
      }
      let gateway = Gateway::new(
        GatewayConfig::builder()
          .sessions_dir(base.join("sessions"))
          .omp(HostConfig::builder().omp(PathBuf::from("omp")).build())
          .build(),
      );
      let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64).unwrap())))
        .await
        .unwrap();
      let projects = alvin_protocol::ProjectsApiClient::list(&client).await.unwrap();
      let project = projects.first().unwrap();
      let key = SessionsKey {
        project: project.id.clone(),
        checkout: Some(project.checkouts.first().unwrap().id.clone()),
      };
      let page = alvin_protocol::SessionsApiClient::list(
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
      assert_eq!(page.sessions.len(), 50);
      assert!(page.next.is_some());
      let model = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.new(|cx| {
          let mut model = Model::new(client, cx);
          model.seed_workspace(projects, vec![(key.clone(), page)], cx);
          model
        })
      });
      let load = cx.update(|cx| model.update(cx, |model, cx| model.load_all_sessions(cx)));
      assert!(load.await);
      cx.update(|cx| {
        let model = model.read(cx);
        let list = model.conversation().workspace().sessions(&key).unwrap();
        assert_eq!(list.sessions.len(), 55);
        assert!(list.next.is_none());
      });
      std::fs::remove_dir_all(base).unwrap();
    });
  }
}
