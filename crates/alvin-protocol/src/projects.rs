//! Projects (repositories) and their checkouts, as listed by `projects/list`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ids::{CheckoutId, ProjectId, SessionId};

/// The version control system that owns a checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Vcs {
  /// A git checkout without jj metadata.
  Git,
  /// A jj checkout without a `.git` entry.
  Jujutsu,
  /// A jj checkout that also carries a `.git` entry.
  JujutsuColocated,
}

/// One working copy of a project: the main checkout, a git worktree, or a jj workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckoutSummary {
  /// Digest of the canonical checkout root.
  pub id: CheckoutId,
  /// Canonical checkout root.
  pub root: PathBuf,
  /// The version control system, absent for a directory with a workspace marker but no VCS.
  pub vcs: Option<Vcs>,
}

/// Why an omp session's `cwd` does not resolve to a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UnresolvedCause {
  /// No workspace marker was found walking up from the directory, typically because it was deleted.
  NoWorkspaceMarker {
    /// Where discovery started.
    start: PathBuf,
  },
  /// A path could not be resolved from the filesystem.
  PathResolution {
    /// The path that failed to resolve.
    path: PathBuf,
  },
  /// A `.git` file or jj repository pointer was malformed, for example a worktree whose repository was deleted.
  InvalidVcsPointer {
    /// The pointer file.
    path: PathBuf,
  },
}

/// A group of sessions: a repository with its checkouts, or the sessions of a `cwd` that no longer resolves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ProjectSummary {
  /// A repository, or a plain workspace directory when there is no VCS.
  Resolved {
    /// Digest of the canonical repository root.
    id: ProjectId,
    /// Canonical repository root.
    root: PathBuf,
    /// Checkouts that have sessions, ordered by root.
    checkouts: Vec<CheckoutSummary>,
  },
  /// A recorded `cwd` that does not resolve. This is a state to show the user, not a fallback.
  Unresolved {
    /// The `cwd` recorded in the session headers.
    cwd: PathBuf,
    /// Why discovery failed.
    cause: UnresolvedCause,
    /// The sessions recorded under this `cwd`.
    sessions: Vec<SessionId>,
  },
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::{ProjectSummary, UnresolvedCause};
  use crate::ids::SessionId;

  #[test]
  fn unresolved_project_has_a_stable_wire_shape() {
    let project = ProjectSummary::Unresolved {
      cwd: PathBuf::from("/gone"),
      cause: UnresolvedCause::NoWorkspaceMarker {
        start: PathBuf::from("/gone"),
      },
      sessions: vec![SessionId::from("s1".to_owned())],
    };
    let json = serde_json::to_value(&project).unwrap();
    assert_eq!(
      json,
      serde_json::json!({
        "kind": "unresolved",
        "cwd": "/gone",
        "cause": { "kind": "noWorkspaceMarker", "start": "/gone" },
        "sessions": ["s1"],
      })
    );
    assert_eq!(serde_json::from_value::<ProjectSummary>(json).unwrap(), project);
  }
}
