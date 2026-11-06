//! Projects and checkouts from the `cwd` of each session.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use alvin_protocol::{CheckoutId, CheckoutSummary, ProjectId, ProjectSummary, Vcs};
use code_workspace::Workspace;

use crate::header::SessionHead;
use crate::{Error, Result};

/// Where a `cwd` led: its checkout and repository.
struct Resolved {
  repository: PathBuf,
  checkout: PathBuf,
  vcs: Option<Vcs>,
}

/// An id for a canonical root path: the hex of its blake3 hash.
fn path_id(path: &Path) -> String {
  blake3::hash(path.as_os_str().as_encoded_bytes()).to_hex().to_string()
}

/// Resolves a `cwd` to its workspace. `None` when it no longer resolves (deleted directory, worktree whose
/// repository is gone): such a session belongs to no project.
fn resolve(cwd: &Path) -> Result<Option<Resolved>> {
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
      Ok(Some(Resolved { repository, checkout, vcs }))
    }
    Err(Discovery::NoWorkspaceMarker { .. } | Discovery::PathResolution { .. } | Discovery::InvalidVcsPointer { .. }) => Ok(None),
    Err(source) => Err(Error::ResolveWorkspace {
      cwd: cwd.to_path_buf(),
      source,
    }),
  }
}

/// Groups session working directories by repository and checkout, resolving each distinct `cwd` once.
///
/// Projects are ordered by repository root. A `cwd` that no longer resolves has no project and is left out.
pub fn group(cwds: impl IntoIterator<Item = PathBuf>) -> Result<Vec<ProjectSummary>> {
  let mut repositories: BTreeMap<PathBuf, BTreeMap<PathBuf, Option<Vcs>>> = BTreeMap::new();
  for cwd in cwds.into_iter().collect::<BTreeSet<_>>() {
    if let Some(Resolved { repository, checkout, vcs }) = resolve(&cwd)? {
      repositories.entry(repository).or_default().insert(checkout, vcs);
    }
  }

  Ok(
    repositories
      .into_iter()
      .map(|(root, checkouts)| ProjectSummary {
        id: ProjectId::from(path_id(&root)),
        root,
        checkouts: checkouts
          .into_iter()
          .map(|(root, vcs)| CheckoutSummary {
            id: CheckoutId::from(path_id(&root)),
            root,
            vcs,
          })
          .collect(),
      })
      .collect(),
  )
}

/// The sessions of one project, or of one checkout of it, resolving each relevant distinct `cwd` once.
///
/// Empty when no project has that id, or the project has no such checkout: a project and a checkout only exist
/// through their sessions.
pub fn select(heads: Vec<SessionHead>, project: &ProjectId, checkout: Option<&CheckoutId>) -> Result<Vec<SessionHead>> {
  let mut resolutions: BTreeMap<PathBuf, Option<Resolved>> = BTreeMap::new();
  let mut selected = Vec::new();
  for head in heads {
    if !resolutions.contains_key(&head.cwd) {
      let candidate = checkout.is_none_or(|wanted| {
        head
          .cwd
          .canonicalize()
          .map_or(true, |cwd| cwd.ancestors().any(|ancestor| path_id(ancestor) == wanted.as_str()))
      });
      resolutions.insert(head.cwd.clone(), if candidate { resolve(&head.cwd)? } else { None });
    }
    let Some(Some(Resolved {
      repository,
      checkout: root,
      ..
    })) = resolutions.get(&head.cwd)
    else {
      continue;
    };
    if path_id(repository) == project.as_str() && checkout.is_none_or(|wanted| path_id(root) == wanted.as_str()) {
      selected.push(head);
    }
  }
  Ok(selected)
}

#[cfg(test)]
mod tests {
  use std::path::{Path, PathBuf};

  use alvin_protocol::{CheckoutId, CheckoutSummary, ProjectId, ProjectSummary, Vcs};

  use super::{group, select};
  use crate::scan;
  use crate::test_support::{TempDir, run, run_omp};

  fn blake3_hex(root: &Path) -> String {
    blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex().to_string()
  }

  fn project_id(root: &Path) -> ProjectId {
    ProjectId::from(blake3_hex(root))
  }

  fn checkout(root: &Path, vcs: Option<Vcs>) -> CheckoutSummary {
    let id = CheckoutId::from(blake3_hex(root));
    CheckoutSummary {
      id,
      root: root.to_path_buf(),
      vcs,
    }
  }

  fn cwds(paths: &[&Path]) -> Vec<PathBuf> {
    paths.iter().map(|path| path.to_path_buf()).collect()
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

    let projects = group(cwds(&[&repo, &sub, &wt1, &wt2])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary {
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

    let projects = group(cwds(&[&repo, &ws1, &ws2])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary {
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
  fn a_deleted_cwd_belongs_to_no_project() {
    let tmp = TempDir::new();
    let gone = tmp.path().join("gone");
    std::fs::create_dir(&gone).unwrap();
    std::fs::remove_dir(&gone).unwrap();

    assert_eq!(group(cwds(&[&gone, &gone])).unwrap(), vec![]);
  }

  #[test]
  fn a_worktree_whose_repository_was_deleted_belongs_to_no_project() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run(&repo, "git", &["init", "--quiet"]);
    run(&repo, "git", &["commit", "--quiet", "--allow-empty", "-m", "init"]);
    let wt = tmp.path().join("wt");
    run(&repo, "git", &["worktree", "add", "--quiet", "../wt", "-b", "one"]);
    std::fs::remove_dir_all(&repo).unwrap();

    assert_eq!(group(cwds(&[&wt])).unwrap(), vec![]);
  }

  #[test]
  fn select_skips_sessions_whose_cwd_no_longer_resolves() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run(&repo, "git", &["init", "--quiet"]);
    let gone = tmp.path().join("gone");
    let head = |cwd: &Path, id: &str| crate::SessionHead {
      path: PathBuf::from(format!("{id}.jsonl")),
      id: alvin_protocol::SessionId::from(id.to_owned()),
      cwd: cwd.to_path_buf(),
      title: None,
      created: jiff::Timestamp::UNIX_EPOCH,
      modified: jiff::Timestamp::UNIX_EPOCH,
      parent: None,
      assistant_in_head: false,
    };

    let selected = select(vec![head(&repo, "alive"), head(&gone, "gone")], &project_id(&repo), None).unwrap();

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id.as_str(), "alive");
  }

  #[test]
  fn checkout_selection_resolves_nested_and_symlinked_cwds_and_checks_project_membership() {
    let tmp = TempDir::new();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    run(&repo, "git", &["init", "--quiet"]);
    let nested = repo.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&repo, &link).unwrap();
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    run(&other, "git", &["init", "--quiet"]);
    let head = |cwd: &Path, id: &str| crate::SessionHead {
      path: PathBuf::from(format!("{id}.jsonl")),
      id: alvin_protocol::SessionId::from(id.to_owned()),
      cwd: cwd.to_path_buf(),
      title: None,
      created: jiff::Timestamp::UNIX_EPOCH,
      modified: jiff::Timestamp::UNIX_EPOCH,
      parent: None,
      assistant_in_head: false,
    };
    let heads = vec![head(&nested, "nested"), head(&link, "link"), head(&other, "other")];
    let checkout = CheckoutId::from(blake3_hex(&repo));
    let selected = select(heads.clone(), &project_id(&repo), Some(&checkout)).unwrap();
    assert_eq!(selected.iter().map(|head| head.id.as_str()).collect::<Vec<_>>(), ["nested", "link"]);
    assert!(select(heads.clone(), &project_id(&other), Some(&checkout)).unwrap().is_empty());
    assert!(
      select(heads, &project_id(&repo), Some(&CheckoutId::from(blake3_hex(&other))))
        .unwrap()
        .is_empty()
    );
  }

  #[test]
  fn workspace_without_vcs_is_its_own_project_and_resolves_through_symlinks() {
    let tmp = TempDir::new();
    let plain = tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("Cargo.toml"), "[package]\nname = \"plain\"\nversion = \"0.1.0\"\n").unwrap();
    let link = tmp.path().join("link");
    std::os::unix::fs::symlink(&plain, &link).unwrap();

    let projects = group(cwds(&[&link])).unwrap();

    assert_eq!(
      projects,
      vec![ProjectSummary {
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
    let projects = group(heads.into_iter().map(|head| head.cwd)).unwrap();

    let resolved: Vec<_> = projects
      .iter()
      .map(|project| (project.root.clone(), project.checkouts.len()))
      .collect();
    assert_eq!(resolved, vec![(git, 3), (jj, 3)]);
  }
}
