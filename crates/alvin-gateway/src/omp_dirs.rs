//! Where omp keeps its session files. The rules are omp's own (`packages/utils/src/dirs.ts`); faces resolve the
//! directory with them and hand the result to the gateway.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// omp's sessions directory for the given environment. `var` reads one environment variable (`None` when unset) and
/// `home` is the user's home directory.
///
/// Rules, in order: a named profile (`OMP_PROFILE`, else `PI_PROFILE`) selects `<root>/profiles/<name>/agent`; without
/// one, `PI_CODING_AGENT_DIR` replaces the agent directory; the default agent directory is `<root>/agent` where the root
/// is `<home>/<PI_CONFIG_DIR or .omp>`. On Linux and macOS, with the agent directory at its default and
/// `$XDG_DATA_HOME/omp` (or its `profiles/<name>`) present, sessions live there instead.
pub fn sessions_dir(var: impl Fn(&str) -> Option<OsString>, home: &Path) -> PathBuf {
  let non_empty = |name: &str| var(name).filter(|value| !value.is_empty());

  let root = home.join(non_empty("PI_CONFIG_DIR").unwrap_or_else(|| OsString::from(".omp")));
  // An OMP_PROFILE that is set, even empty, wins over PI_PROFILE: an empty one selects the default profile.
  let profile = var("OMP_PROFILE")
    .or_else(|| var("PI_PROFILE"))
    .map(|name| name.to_string_lossy().trim().to_owned())
    .filter(|name| !name.is_empty() && name != "default");

  let profile_root = match &profile {
    Some(name) => root.join("profiles").join(name),
    None => root,
  };
  let default_agent_dir = profile_root.join("agent");
  let agent_dir = match (&profile, non_empty("PI_CODING_AGENT_DIR")) {
    (None, Some(dir)) => PathBuf::from(dir),
    _ => default_agent_dir.clone(),
  };

  let xdg_data = Some(agent_dir == default_agent_dir)
    .filter(|is_default| *is_default && cfg!(any(target_os = "linux", target_os = "macos")))
    .and_then(|_| non_empty("XDG_DATA_HOME"))
    .map(|base| match &profile {
      Some(name) => Path::new(&base).join("omp").join("profiles").join(name),
      None => Path::new(&base).join("omp"),
    })
    .filter(|dir| dir.exists());

  xdg_data.unwrap_or(agent_dir).join("sessions")
}

#[cfg(test)]
mod tests {
  use std::collections::HashMap;

  use alvin_protocol::EventId;

  use super::*;

  fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let vars: HashMap<String, OsString> = pairs
      .iter()
      .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
      .collect();
    move |name| vars.get(name).cloned()
  }

  /// A directory that exists until dropped.
  struct Scratch(PathBuf);

  impl Scratch {
    fn new() -> Self {
      let dir = std::env::temp_dir().join(format!("alvin-omp-dirs-{}", EventId::new().as_uuid()));
      std::fs::create_dir_all(&dir).unwrap();
      Self(dir)
    }
  }

  impl Drop for Scratch {
    fn drop(&mut self) {
      std::fs::remove_dir_all(&self.0).unwrap();
    }
  }

  #[test]
  fn defaults_to_the_agent_directory_under_the_home() {
    assert_eq!(
      sessions_dir(env(&[]), Path::new("/home/me")),
      Path::new("/home/me/.omp/agent/sessions")
    );
  }

  #[test]
  fn the_config_directory_name_and_agent_directory_can_be_overridden() {
    let home = Path::new("/home/me");
    assert_eq!(
      sessions_dir(env(&[("PI_CONFIG_DIR", ".pi")]), home),
      Path::new("/home/me/.pi/agent/sessions")
    );
    assert_eq!(
      sessions_dir(env(&[("PI_CODING_AGENT_DIR", "/srv/agent")]), home),
      Path::new("/srv/agent/sessions")
    );
  }

  #[test]
  fn a_named_profile_has_its_own_agent_directory_and_ignores_the_agent_dir_override() {
    let home = Path::new("/home/me");
    let expected = Path::new("/home/me/.omp/profiles/work/agent/sessions");
    assert_eq!(
      sessions_dir(env(&[("OMP_PROFILE", "work"), ("PI_CODING_AGENT_DIR", "/srv/agent")]), home),
      expected
    );
    assert_eq!(sessions_dir(env(&[("PI_PROFILE", "work")]), home), expected);
  }

  #[test]
  fn an_empty_omp_profile_selects_the_default_even_when_the_legacy_one_is_set() {
    let dir = sessions_dir(env(&[("OMP_PROFILE", ""), ("PI_PROFILE", "work")]), Path::new("/home/me"));
    assert_eq!(dir, Path::new("/home/me/.omp/agent/sessions"));
  }

  #[test]
  fn an_existing_xdg_omp_directory_replaces_the_default_agent_directory() {
    let xdg = Scratch::new();
    let home = Path::new("/home/me");
    let vars = [("XDG_DATA_HOME", xdg.0.to_str().unwrap())];

    assert_eq!(sessions_dir(env(&vars), home), Path::new("/home/me/.omp/agent/sessions"));

    std::fs::create_dir(xdg.0.join("omp")).unwrap();
    let expected = xdg.0.join("omp").join("sessions");
    assert_eq!(
      sessions_dir(env(&vars), home),
      if cfg!(any(target_os = "linux", target_os = "macos")) {
        expected
      } else {
        PathBuf::from("/home/me/.omp/agent/sessions")
      }
    );

    // An agent directory override opts out of XDG.
    let overridden = [vars[0], ("PI_CODING_AGENT_DIR", "/srv/agent")];
    assert_eq!(sessions_dir(env(&overridden), home), Path::new("/srv/agent/sessions"));
  }
}
