//! alvind's settings: command-line flags, `ALVIN_*` environment variables and `alvind.toml`, merged by confique. Parsing
//! only: whether the sessions directory exists or the port is free is the gateway's business.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use alvin_gateway::GatewayConfig;
use confique::Config as _;
use eyre::eyre;

/// What alvind reads from its sources.
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
pub struct AlvindConfig {
  /// Address the WebSocket listener binds.
  #[config(default = "127.0.0.1:7410", env = "ALVIN_LISTEN")]
  #[config(layer_attr(arg(long)))]
  pub listen: SocketAddr,
  /// omp's sessions directory. Absent: resolved from omp's own rules.
  #[config(env = "ALVIN_SESSIONS_DIR")]
  #[config(layer_attr(arg(long)))]
  pub sessions_dir: Option<PathBuf>,
}

/// alvind's command line.
#[derive(clap::Parser)]
#[command(version, about = "Serves the alvin gateway over WebSocket")]
pub struct Cli {
  /// A config file read after flags and environment, before the user's `alvind.toml`.
  #[arg(long)]
  config_file: Option<PathBuf>,
  #[command(flatten)]
  config: <AlvindConfig as confique::Config>::Layer,
}

/// The gateway's configuration plus where to listen.
#[derive(Debug)]
pub struct Settings {
  pub listen: SocketAddr,
  pub gateway: GatewayConfig,
}

impl Cli {
  /// Merges every source into [`Settings`].
  pub fn settings(self) -> eyre::Result<Settings> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("cannot determine the home directory"))?;
    let files = config_files(self.config_file, dirs::config_dir(), &home);
    let config = load(self.config, &files)?;
    let sessions_dir = config
      .sessions_dir
      .unwrap_or_else(|| alvin_gateway::sessions_dir(|name| std::env::var_os(name), &home));
    Ok(Settings {
      listen: config.listen,
      gateway: GatewayConfig::builder().sessions_dir(sessions_dir).build(),
    })
  }
}

/// Config files, highest precedence first: `--config-file`, then on macOS `~/.config/alvin/alvind.toml`, then the
/// platform's config directory.
fn config_files(explicit: Option<PathBuf>, config_dir: Option<PathBuf>, home: &Path) -> Vec<PathBuf> {
  let mac_dir = cfg!(target_os = "macos").then(|| home.join(".config"));
  explicit
    .into_iter()
    .chain(
      mac_dir
        .into_iter()
        .chain(config_dir)
        .map(|dir| dir.join("alvin").join("alvind.toml")),
    )
    .collect()
}

/// Layers, highest precedence first: flags, environment, then `files` in order. A file that does not exist is skipped.
fn load(flags: <AlvindConfig as confique::Config>::Layer, files: &[PathBuf]) -> Result<AlvindConfig, confique::Error> {
  let builder = AlvindConfig::builder().preloaded(flags).env();
  files.iter().fold(builder, |builder, file| builder.file(file)).load()
}

#[cfg(test)]
mod tests {
  use alvin_protocol::EventId;
  use confique::Layer as _;

  use super::*;

  /// A directory that exists until dropped.
  struct Scratch(PathBuf);

  impl Scratch {
    fn new() -> Self {
      let dir = std::env::temp_dir().join(format!("alvin-settings-{}", EventId::new().as_uuid()));
      std::fs::create_dir_all(&dir).unwrap();
      Self(dir)
    }

    fn file(&self, name: &str, text: &str) -> PathBuf {
      let path = self.0.join(name);
      std::fs::write(&path, text).unwrap();
      path
    }
  }

  impl Drop for Scratch {
    fn drop(&mut self) {
      std::fs::remove_dir_all(&self.0).unwrap();
    }
  }

  fn no_flags() -> <AlvindConfig as confique::Config>::Layer {
    <AlvindConfig as confique::Config>::Layer::empty()
  }

  #[test]
  fn schema_defaults_apply_when_no_file_exists() {
    let tmp = Scratch::new();
    let config = load(no_flags(), &[tmp.0.join("absent.toml")]).unwrap();
    assert_eq!(config.listen, "127.0.0.1:7410".parse().unwrap());
    assert_eq!(config.sessions_dir, None);
  }

  #[test]
  fn an_earlier_file_wins_over_a_later_one_and_flags_win_over_both() {
    let tmp = Scratch::new();
    let first = tmp.file("first.toml", "listen = \"127.0.0.1:1111\"\n");
    let second = tmp.file("second.toml", "listen = \"127.0.0.1:2222\"\nsessions_dir = \"/srv/sessions\"\n");
    let files = [first, second];

    let config = load(no_flags(), &files).unwrap();
    assert_eq!(config.listen, "127.0.0.1:1111".parse().unwrap());
    assert_eq!(config.sessions_dir, Some(PathBuf::from("/srv/sessions")));

    let mut flags = no_flags();
    flags.listen = Some("127.0.0.1:3333".parse().unwrap());
    assert_eq!(load(flags, &files).unwrap().listen, "127.0.0.1:3333".parse().unwrap());
  }

  #[test]
  fn a_malformed_file_is_an_error_not_a_skip() {
    let tmp = Scratch::new();
    let file = tmp.file("bad.toml", "listen = [not toml\n");
    assert!(load(no_flags(), &[file]).is_err());
  }

  #[test]
  fn explicit_file_comes_before_the_user_files() {
    let home = Path::new("/home/me");
    let files = config_files(Some(PathBuf::from("/etc/alvind.toml")), Some(PathBuf::from("/home/me/cfg")), home);
    assert_eq!(files.first(), Some(&PathBuf::from("/etc/alvind.toml")));
    assert_eq!(files.last(), Some(&PathBuf::from("/home/me/cfg/alvin/alvind.toml")));
  }
}
