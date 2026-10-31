//! Projects (repositories) and their checkouts, as listed by `projects/list`.

use std::path::PathBuf;

use jsonrpsee::proc_macros::rpc;
use serde::{Deserialize, Serialize};

use crate::ids::{CheckoutId, ProjectId};

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

/// A repository, or a plain workspace directory when there is no VCS, with the checkouts that have sessions.
/// A `cwd` that no longer resolves to a workspace has no project: its sessions are not listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSummary {
  /// Digest of the canonical repository root.
  pub id: ProjectId,
  /// Canonical repository root.
  pub root: PathBuf,
  /// Checkouts that have sessions, ordered by root.
  pub checkouts: Vec<CheckoutSummary>,
}

/// Repositories and their checkouts, from omp's session files.
#[cfg_attr(feature = "server", rpc(client, server, namespace = "projects", namespace_separator = "/"))]
#[cfg_attr(not(feature = "server"), rpc(client, namespace = "projects", namespace_separator = "/"))]
pub trait ProjectsApi {
  /// Every project whose sessions' `cwd` still resolves to a workspace, with the checkouts that have sessions.
  #[method(name = "list")]
  async fn list(&self) -> jsonrpsee::core::RpcResult<Vec<ProjectSummary>>;
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use super::{CheckoutSummary, ProjectSummary, Vcs};
  use crate::ids::{CheckoutId, ProjectId};

  #[test]
  fn a_project_has_a_stable_wire_shape() {
    let project = ProjectSummary {
      id: ProjectId::from("p".to_owned()),
      root: PathBuf::from("/repo"),
      checkouts: vec![CheckoutSummary {
        id: CheckoutId::from("c".to_owned()),
        root: PathBuf::from("/repo"),
        vcs: Some(Vcs::JujutsuColocated),
      }],
    };
    let json = serde_json::to_value(&project).unwrap();
    assert_eq!(
      json,
      serde_json::json!({
        "id": "p",
        "root": "/repo",
        "checkouts": [{ "id": "c", "root": "/repo", "vcs": "jujutsuColocated" }],
      })
    );
    assert_eq!(serde_json::from_value::<ProjectSummary>(json).unwrap(), project);
  }
}
