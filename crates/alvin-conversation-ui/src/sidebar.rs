//! The project tree: projects, their checkouts and the sessions of each, with the commands to open them.

use std::collections::BTreeMap;
use std::rc::Rc;

use alvin_conversation::{SessionsKey, Workspace};
use alvin_protocol::{CheckoutId, ProjectId, SessionCursor, SessionId};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, h_flex};
use gpui_kit::{
  AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, rems,
};

use crate::model::Model;

/// What a row of the tree stands for. Rows are found by their item id, which is derived from the domain id behind them.
#[derive(Clone)]
enum Node {
  /// A project or a checkout: opens and closes.
  Group,
  /// A listed session.
  Session(SessionId),
  /// Starts a session in a checkout.
  New { project: ProjectId, checkout: CheckoutId },
  /// Loads the next page of a checkout's sessions.
  More { key: SessionsKey, after: SessionCursor },
}

/// The tree view over the projects and session lists the model holds.
pub(crate) struct ProjectTree {
  model: Entity<Model>,
  tree: Entity<TreeState>,
  nodes: Rc<BTreeMap<SharedString, Node>>,
  /// What the tree was last built from, so a model change that leaves it alone keeps the rows and their expansion.
  built_from: Option<Workspace>,
  _subscriptions: Vec<Subscription>,
}

impl ProjectTree {
  pub(crate) fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
    let tree = cx.new(|cx| TreeState::new(cx));
    let subscriptions = vec![cx.observe(&model, |this, _, cx| this.sync(cx))];
    let mut this = Self {
      model,
      tree,
      nodes: Rc::default(),
      built_from: None,
      _subscriptions: subscriptions,
    };
    this.sync(cx);
    this
  }

  fn sync(&mut self, cx: &mut Context<Self>) {
    let workspace = self.model.read(cx).conversation().workspace();
    if self.built_from.as_ref() == Some(workspace) {
      return;
    }
    let (items, nodes) = items(workspace);
    self.built_from = Some(workspace.clone());
    self.nodes = Rc::new(nodes);
    self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
    cx.notify();
  }
}

fn item_id(kind: &str, id: &str) -> SharedString {
  SharedString::from(format!("{kind}:{id}"))
}

/// The tree items for the workspace, and what each stands for.
fn items(workspace: &Workspace) -> (Vec<TreeItem>, BTreeMap<SharedString, Node>) {
  let mut nodes = BTreeMap::new();
  let mut projects = Vec::new();
  for project in workspace.projects() {
    let mut checkouts = Vec::new();
    for checkout in &project.checkouts {
      let key = SessionsKey {
        project: project.id.clone(),
        checkout: Some(checkout.id.clone()),
      };
      let list = workspace.sessions(&key);
      let mut children = Vec::new();

      let id = item_id("new", checkout.id.as_str());
      nodes.insert(
        id.clone(),
        Node::New {
          project: project.id.clone(),
          checkout: checkout.id.clone(),
        },
      );
      children.push(TreeItem::new(id, "New session"));

      for session in list.iter().flat_map(|list| &list.sessions) {
        let id = item_id("session", session.id.as_str());
        nodes.insert(id.clone(), Node::Session(session.id.clone()));
        let title = session.title.clone().unwrap_or_else(|| session.id.as_str().to_owned());
        children.push(TreeItem::new(id, title));
      }

      if let Some(after) = list.and_then(|list| list.next.clone()) {
        let id = item_id("more", checkout.id.as_str());
        nodes.insert(id.clone(), Node::More { key, after });
        children.push(TreeItem::new(id, "More sessions"));
      }

      let id = item_id("checkout", checkout.id.as_str());
      nodes.insert(id.clone(), Node::Group);
      checkouts.push(
        TreeItem::new(id, checkout.root.display().to_string())
          .expanded(true)
          .children(children),
      );
    }
    let id = item_id("project", project.id.as_str());
    nodes.insert(id.clone(), Node::Group);
    let name = project
      .root
      .file_name()
      .map_or_else(|| project.root.display().to_string(), |name| name.to_string_lossy().into_owned());
    projects.push(TreeItem::new(id, name).expanded(true).children(checkouts));
  }
  (projects, nodes)
}

impl Render for ProjectTree {
  fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    if self.nodes.is_empty() {
      return div()
        .size_full()
        .p_3()
        .text_color(cx.theme().muted_foreground)
        .child("No projects yet")
        .into_any_element();
    }
    let model = self.model.clone();
    let nodes = self.nodes.clone();
    div()
      .size_full()
      .child(tree(&self.tree, move |_, entry, selected, _, _| {
        let item = entry.item();
        let node = nodes.get(&item.id);
        let icon = match node {
          Some(Node::Session(_)) => IconName::File,
          Some(Node::New { .. }) => IconName::Plus,
          Some(Node::More { .. }) => IconName::Ellipsis,
          Some(Node::Group) | None if entry.is_expanded() => IconName::FolderOpen,
          Some(Node::Group) | None => IconName::Folder,
        };
        let row = ListItem::new(item.id.clone()).selected(selected).child(
          h_flex()
            .gap_2()
            .pl(rems(0.75) * entry.depth() as f32)
            .child(Icon::new(icon))
            .child(item.label.clone()),
        );
        let model = model.clone();
        match node.cloned() {
          Some(Node::Session(session)) => row.on_click(move |_, _, cx| {
            model.update(cx, |model, cx| model.open_session(session.clone(), cx)).detach();
          }),
          Some(Node::New { project, checkout }) => row.on_click(move |_, _, cx| {
            model
              .update(cx, |model, cx| model.new_session(project.clone(), checkout.clone(), cx))
              .detach();
          }),
          Some(Node::More { key, after }) => row.on_click(move |_, _, cx| {
            model
              .update(cx, |model, cx| model.load_sessions(key.clone(), Some(after.clone()), cx))
              .detach();
          }),
          Some(Node::Group) | None => row,
        }
      }))
      .into_any_element()
  }
}
