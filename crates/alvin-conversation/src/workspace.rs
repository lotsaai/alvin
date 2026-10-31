//! Projects with their checkouts, and the paged session list of each project or checkout.

use std::collections::BTreeMap;

use alvin_protocol::{CheckoutId, ProjectId, ProjectSummary, SessionCursor, SessionPage, SessionSummary};

/// Which sessions a list holds: a whole project, or one of its checkouts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionsKey {
  /// The project.
  pub project: ProjectId,
  /// The checkout, or `None` for every checkout of the project.
  pub checkout: Option<CheckoutId>,
}

/// The pages of one session list loaded so far, newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionList {
  /// The sessions loaded.
  pub sessions: Vec<SessionSummary>,
  /// The cursor for the next page, or `None` when the list is complete.
  pub next: Option<SessionCursor>,
}

/// Every project and the session lists loaded for them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workspace {
  projects: Vec<ProjectSummary>,
  sessions: BTreeMap<SessionsKey, SessionList>,
}

impl Workspace {
  /// The projects, as `projects/list` returned them.
  pub fn projects(&self) -> &[ProjectSummary] {
    &self.projects
  }

  /// The session list loaded for `key`, if any.
  pub fn sessions(&self, key: &SessionsKey) -> Option<&SessionList> {
    self.sessions.get(key)
  }

  /// Replaces the projects. Session lists of projects that are gone are dropped.
  pub fn set_projects(&mut self, projects: Vec<ProjectSummary>) {
    self
      .sessions
      .retain(|key, _| projects.iter().any(|project| project.id == key.project));
    self.projects = projects;
  }

  /// Takes a page of `key`'s sessions. A page requested without a cursor is the first and replaces the list; one requested
  /// after a cursor extends it.
  pub fn load_sessions(&mut self, key: SessionsKey, after: Option<&SessionCursor>, page: SessionPage) {
    let list = self.sessions.entry(key).or_default();
    if after.is_none() {
      list.sessions.clear();
    }
    list.sessions.extend(page.sessions);
    list.next = page.next;
  }
}
