//! Projects and checkouts from the `cwd` of each session.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use alvin_protocol::projects::{CheckoutSummary, ProjectSummary, UnresolvedCause, Vcs};
use alvin_protocol::{CheckoutId, ProjectId, SessionId};
use code_workspace::Workspace;

use crate::{Error, Result};

/// Where a `cwd` led: a checkout of a repository, or the reason it did not resolve.
enum Resolution {
  Resolved {
    repository: PathBuf,
    checkout: PathBuf,
    vcs: Option<Vcs>,
  },
  Unresolved(UnresolvedCause),
}

fn digest(path: &Path) -> [u8; 32] {
  *blake3::hash(path.as_os_str().as_encoded_bytes()).as_bytes()
}

fn resolve(cwd: &Path) -> Result<Resolution> {
  use code_workspace::Error as Discovery;

  match Workspace::discover(cwd) {
    Ok(workspace) => {
      // Discovery canonicalizes the repository root but not a plain checkout root, so a symlinked cwd would
      // otherwise yield a second id for the same project.
      let checkout = workspace.root().canonicalize().map_err(|source| Error::CanonicalizeRoot {
        root: workspace.root().to_path_buf(),
        source,
      })?;
      let repository = workspace.repository_root().map_or_else(|| checkout.clone(), Path::to_path_buf);
      let vcs = workspace.vcs().map(|vcs| match vcs {
        code_workspace::Vcs::Git => Vcs::Git,
        code_workspace::Vcs::Jujutsu => Vcs::Jujutsu,
        code_workspace::Vcs::JujutsuColocated => Vcs::JujutsuColocated,
      });
      Ok(Resolution::Resolved { repository, checkout, vcs })
    }
    Err(Discovery::NoWorkspaceMarker { start }) => Ok(Resolution::Unresolved(UnresolvedCause::NoWorkspaceMarker {
      start: PathBuf::from(start),
    })),
    Err(Discovery::PathResolution { path }) => Ok(Resolution::Unresolved(UnresolvedCause::PathResolution { path })),
    Err(Discovery::InvalidVcsPointer { path }) => Ok(Resolution::Unresolved(UnresolvedCause::InvalidVcsPointer { path })),
    Err(source) => Err(Error::ResolveWorkspace {
      cwd: cwd.to_path_buf(),
      source,
    }),
  }
}

/// Groups sessions by repository and checkout, resolving each distinct `cwd` once.
///
/// Resolved projects come first, ordered by repository root, then the unresolved groups ordered by `cwd`.
pub fn group(sessions: impl IntoIterator<Item = (PathBuf, SessionId)>) -> Result<Vec<ProjectSummary>> {
  let mut by_cwd: BTreeMap<PathBuf, Vec<SessionId>> = BTreeMap::new();
  for (cwd, id) in sessions {
    by_cwd.entry(cwd).or_default().push(id);
  }

  let mut repositories: BTreeMap<PathBuf, BTreeMap<PathBuf, Option<Vcs>>> = BTreeMap::new();
  let mut unresolved = Vec::new();
  for (cwd, mut ids) in by_cwd {
    match resolve(&cwd)? {
      Resolution::Resolved { repository, checkout, vcs } => {
        repositories.entry(repository).or_default().insert(checkout, vcs);
      }
      Resolution::Unresolved(cause) => {
        ids.sort();
        unresolved.push(ProjectSummary::Unresolved { cwd, cause, sessions: ids });
      }
    }
  }

  let resolved = repositories.into_iter().map(|(root, checkouts)| ProjectSummary::Resolved {
    id: ProjectId::from_digest(digest(&root)),
    root,
    checkouts: checkouts
      .into_iter()
      .map(|(root, vcs)| CheckoutSummary {
        id: CheckoutId::from_digest(digest(&root)),
        root,
        vcs,
      })
      .collect(),
  });
  Ok(resolved.chain(unresolved).collect())
}
#[cfg(test)]
mod tests {
  use std::path::{Path, PathBuf};

  use alvin_protocol::projects::{CheckoutSummary, ProjectSummary, UnresolvedCause, Vcs};
  use alvin_protocol::{CheckoutId, ProjectId, SessionId};

  use super::group;
  use crate::scan;
  use crate::test_support::{TempDir, run, run_omp};

  fn project_id(root: &Path) -> ProjectId {
    ProjectId::from_digest(*blake3::hash(root.as_os_str().as_encoded_bytes()).as_bytes())
  }

  fn checkout(root: &Path, vcs: Option<Vcs>) -> CheckoutSummary {
    let id = CheckoutId::from_digest(*blake3::hash(root.as_os_str().as_encoded_bytes()).as_bytes());
    CheckoutSummary {
      id,
      root: root.to_path_buf(),
      vcs,
    }
  }

  fn sessions(cwds: &[&Path]) -> Vec<(PathBuf, SessionId)> {
    cwds
      .iter()
      .enumerate()
      .map(|(n, cwd)| (cwd.to_path_buf(), SessionId::from(format!("session-{n}"))))
      .collect()
  }

  #[test]
  fn git_repository_with_two_worktrees_is_one_project_with_three_checkouts() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run(&repo, "git", &["init", "--quiet"]);
    run(&repo, "git", &["commit", "--quiet", "--allow-empty", "-m", "init"]);
    let wt1 = tmp.path().join("wt1");
    let wt2 = tmp.path().join("wt2");
    run(&repo, "git", &["worktree", "add", "--quiet", "../wt1", "-b", "one"]);
    run(&repo, "git", &["worktree", "add", "--quiet", "../wt2", "-b", "two"]);
    let sub = repo.join("src");
    std::fs::create_dir(&sub).unwrap();

    let projects = group(sessions(&[&repo, &sub, &wt1, &wt2])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary::Resolved {
        id: project_id(&repo),
        root: repo.clone(),
        checkouts: vec![
          checkout(&repo, Some(Vcs::Git)),
          checkout(&wt1, Some(Vcs::Git)),
          checkout(&wt2, Some(Vcs::Git))
        ],
      }]
    );
  }

  #[test]
  fn jj_repository_with_two_workspaces_is_one_project_with_three_checkouts() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    run(tmp.path(), "jj", &["git", "init", "repo"]);
    run(&repo, "jj", &["workspace", "add", "../ws1"]);
    run(&repo, "jj", &["workspace", "add", "../ws2"]);
    let ws1 = tmp.path().join("ws1");
    let ws2 = tmp.path().join("ws2");
    let vcs_of = |root: &Path| {
      Some(if root.join(".git").exists() {
        Vcs::JujutsuColocated
      } else {
        Vcs::Jujutsu
      })
    };

    let projects = group(sessions(&[&repo, &ws1, &ws2])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary::Resolved {
        id: project_id(&repo),
        root: repo.clone(),
        checkouts: vec![
          checkout(&repo, vcs_of(&repo)),
          checkout(&ws1, vcs_of(&ws1)),
          checkout(&ws2, vcs_of(&ws2))
        ],
      }]
    );
  }

  #[test]
  fn deleted_cwd_lands_in_an_unresolved_group_with_its_sessions() {
    let tmp = TempDir::new();
    let gone = tmp.path().join("gone");
    std::fs::create_dir(&gone).unwrap();
    std::fs::remove_dir(&gone).unwrap();
    // Discovery walks up from the nearest existing ancestor of a deleted cwd.
    let nearest_ancestor = tmp.path().to_path_buf();

    let projects = group(sessions(&[&gone, &gone])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary::Unresolved {
        cwd: gone.clone(),
        cause: UnresolvedCause::NoWorkspaceMarker { start: nearest_ancestor },
        sessions: vec![SessionId::from("session-0".to_owned()), SessionId::from("session-1".to_owned())],
      }]
    );
  }

  #[test]
  fn worktree_whose_repository_was_deleted_is_unresolved() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run(&repo, "git", &["init", "--quiet"]);
    run(&repo, "git", &["commit", "--quiet", "--allow-empty", "-m", "init"]);
    let wt = tmp.path().join("wt");
    run(&repo, "git", &["worktree", "add", "--quiet", "../wt", "-b", "one"]);
    std::fs::remove_dir_all(&repo).unwrap();

    let projects = group(sessions(&[&wt])).unwrap();

    assert!(matches!(
      projects.as_slice(),
      [ProjectSummary::Unresolved {
        cause: UnresolvedCause::InvalidVcsPointer { .. },
        ..
      }]
    ));
  }

  #[test]
  fn workspace_without_vcs_is_its_own_project_and_resolves_through_symlinks() {
    let tmp = TempDir::new();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("Cargo.toml"), "[package]\nname = \"plain\"\nversion = \"0.1.0\"\n").unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&plain, &link).unwrap();

    let projects = group(sessions(&[&link])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary::Resolved {
        id: project_id(&plain),
        root: plain.clone(),
        checkouts: vec![checkout(&plain, None)]
      }]
    );
  }

  #[test]
  fn real_omp_sessions_in_git_and_jj_checkouts_group_into_two_projects() {
    let tmp = TempDir::new();
    let git = tmp.path().join("git-repo");
    std::fs::create_dir(&git).unwrap();
    run(&git, "git", &["init", "--quiet"]);
    run(&git, "git", &["commit", "--quiet", "--allow-empty", "-m", "init"]);
    run(&git, "git", &["worktree", "add", "--quiet", "../git-wt1", "-b", "one"]);
    run(&git, "git", &["worktree", "add", "--quiet", "../git-wt2", "-b", "two"]);
    run(tmp.path(), "jj", &["git", "init", "jj-repo"]);
    let jj = tmp.path().join("jj-repo");
    run(&jj, "jj", &["workspace", "add", "../jj-ws1"]);
    run(&jj, "jj", &["workspace", "add", "../jj-ws2"]);
    let names = ["git-repo", "git-wt1", "git-wt2", "jj-repo", "jj-ws1", "jj-ws2"];
    let cwds: Vec<PathBuf> = names.iter().map(|name| tmp.path().join(name)).collect();

    let root = tmp.path().join("sessions");
    std::thread::scope(|scope| {
      for cwd in &cwds {
        let dir = root.join(cwd.file_name().unwrap());
        scope.spawn(move || run_omp(cwd, &dir, "Reply with the single word ok."));
      }
    });

    let heads: Vec<_> = scan(&root).unwrap().into_iter().map(|head| head.unwrap()).collect();
    let projects = group(heads.into_iter().map(|head| (head.cwd, head.id))).unwrap();

    let resolved: Vec<_> = projects
      .iter()
      .map(|project| match project {
        ProjectSummary::Resolved { root, checkouts, .. } => (root.clone(), checkouts.len()),
        ProjectSummary::Unresolved { cwd, .. } => panic!("unresolved: {}", cwd.display()),
      })
      .collect();
    assert_eq!(resolved, vec![(git, 3), (jj, 3)]);
  }
}
