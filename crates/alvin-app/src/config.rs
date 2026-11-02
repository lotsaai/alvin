//! alvin-app's settings: command-line flags, `ALVIN_*` environment variables and `config.toml`, merged by confique. Parsing
//! only: whether the omp executable exists or the model is valid is for the gateway to say.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use alvin_gateway::{GatewayConfig, HostConfig};
use confique::Config as _;
use eyre::{bail, eyre};
use url::Url;

/// What the app reads from its sources.
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
// A key nothing reads is a typo; confique would otherwise skip it silently.
#[config(layer_attr(serde(deny_unknown_fields)))]
pub struct AppConfig {
  /// Connect to a running gateway (`ws://` or `wss://`). Absent: embed one.
  #[config(env = "ALVIN_ENDPOINT")]
  #[config(layer_attr(arg(long)))]
  pub endpoint: Option<Url>,
  /// Interface appearance: system, light, or dark.
  #[config(default = "system", env = "ALVIN_APPEARANCE")]
  #[config(layer_attr(arg(long)))]
  pub appearance: String,
  /// The embedded gateway's settings. They do not apply to a remote gateway, which is configured where it runs.
  #[config(nested)]
  #[config(layer_attr(command(flatten)))]
  pub gateway: EmbeddedGateway,
}

/// Settings for the gateway the app embeds.
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
#[config(layer_attr(serde(deny_unknown_fields)))]
pub struct EmbeddedGateway {
  /// The omp executable, found on `PATH` when it has no directory.
  #[config(default = "omp", env = "ALVIN_OMP")]
  #[config(layer_attr(arg(long)))]
  pub omp: PathBuf,
  /// The model sessions start with, as `provider/id`. Absent: omp's own choice.
  #[config(env = "ALVIN_MODEL")]
  #[config(layer_attr(arg(long)))]
  pub model: Option<String>,
  /// omp's sessions directory. Absent: resolved from omp's own rules.
  #[config(env = "ALVIN_SESSIONS_DIR")]
  #[config(layer_attr(arg(long)))]
  pub sessions_dir: Option<PathBuf>,
}

/// alvin-app's command line.
#[derive(clap::Parser)]
#[command(version, about = "The alvin desktop app")]
pub struct Cli {
  /// A config file read after flags and environment, before the user's `config.toml`.
  #[arg(long)]
  pub config_file: Option<PathBuf>,
  #[command(flatten)]
  pub config: <AppConfig as confique::Config>::Layer,
}

/// How the app reaches a gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settings {
  /// Connect to a running gateway.
  Remote(Url),
  /// Run a gateway inside the app.
  Embedded {
    /// The omp executable.
    omp: PathBuf,
    /// The model sessions start with.
    model: Option<String>,
    /// omp's sessions directory, resolved.
    sessions_dir: PathBuf,
  },
}

/// The configuration of the gateway the app embeds.
pub(crate) fn gateway_config(omp: &Path, model: Option<&str>, sessions_dir: &Path) -> GatewayConfig {
  GatewayConfig::builder()
    .sessions_dir(sessions_dir.to_path_buf())
    .omp(HostConfig::builder().omp(omp.to_path_buf()).build())
    .default_model(model.map(str::to_owned))
    .build()
}

impl Cli {
  /// Merges every source into [`Settings`], reading omp's own environment variables to find its sessions directory.
  pub(crate) fn settings(self) -> eyre::Result<Configuration> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("cannot determine the home directory"))?;
    let files = config_files(self.config_file, dirs::config_dir(), &home);
    let path = files
      .first()
      .cloned()
      .ok_or_else(|| eyre!("cannot determine the config directory"))?;
    let config = load(self.config.clone(), &files)?;
    let gateway = settings(config.clone(), &home, |name| std::env::var_os(name))?;
    Appearance::parse(&config.appearance)?;
    Ok(Configuration {
      config,
      gateway,
      path,
      files,
      flags: self.config,
      home,
    })
  }
}

/// Config files, highest precedence first: `--config-file`, then on macOS `~/.config/alvin/config.toml`, then the
/// platform's config directory.
fn config_files(explicit: Option<PathBuf>, config_dir: Option<PathBuf>, home: &Path) -> Vec<PathBuf> {
  let mac_dir = cfg!(target_os = "macos").then(|| home.join(".config"));
  explicit
    .into_iter()
    .chain(
      mac_dir
        .into_iter()
        .chain(config_dir)
        .map(|dir| dir.join("alvin").join("config.toml")),
    )
    .collect()
}

/// Layers, highest precedence first: flags, environment, then `files` in order. A file that does not exist is skipped.
fn load(flags: <AppConfig as confique::Config>::Layer, files: &[PathBuf]) -> Result<AppConfig, confique::Error> {
  let builder = AppConfig::builder().preloaded(flags).env();
  files.iter().fold(builder, |builder, file| builder.file(file)).load()
}

/// A user-selected interface appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
  System,
  Light,
  Dark,
}

impl Appearance {
  pub(crate) fn parse(value: &str) -> eyre::Result<Self> {
    match value {
      "system" => Ok(Self::System),
      "light" => Ok(Self::Light),
      "dark" => Ok(Self::Dark),
      _ => bail!("appearance must be system, light, or dark"),
    }
  }

  pub(crate) fn as_str(self) -> &'static str {
    match self {
      Self::System => "system",
      Self::Light => "light",
      Self::Dark => "dark",
    }
  }
}

/// Config sources and the currently effective settings.
pub(crate) struct Configuration {
  pub(crate) config: AppConfig,
  pub(crate) gateway: Settings,
  path: PathBuf,
  files: Vec<PathBuf>,
  flags: <AppConfig as confique::Config>::Layer,
  home: PathBuf,
}

impl Configuration {
  /// Writes only known keys while retaining the rest of the TOML document.
  pub(crate) fn save(&mut self, config: AppConfig) -> eyre::Result<()> {
    use std::io::Write as _;

    Appearance::parse(&config.appearance)?;
    settings(config.clone(), &self.home, |name| std::env::var_os(name))?;
    let target = match std::fs::symlink_metadata(&self.path) {
      Ok(metadata) if metadata.file_type().is_symlink() => std::fs::canonicalize(&self.path)?,
      Ok(_) => self.path.clone(),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.path.clone(),
      Err(error) => return Err(error.into()),
    };
    let contents = match std::fs::read_to_string(&target) {
      Ok(contents) => contents,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
      Err(error) => return Err(error.into()),
    };
    let mut document = contents.parse::<toml_edit::DocumentMut>()?;
    if config.endpoint != self.config.endpoint {
      set_table_value(document.as_table_mut(), "endpoint", config.endpoint.as_ref().map(Url::as_str));
    }
    if config.appearance != self.config.appearance {
      set_table_value(document.as_table_mut(), "appearance", Some(&config.appearance));
    }
    if config.gateway.omp != self.config.gateway.omp
      || config.gateway.model != self.config.gateway.model
      || config.gateway.sessions_dir != self.config.gateway.sessions_dir
    {
      if !document.contains_key("gateway") {
        document["gateway"] = toml_edit::Item::Table(toml_edit::Table::new());
      }
      let gateway = document["gateway"]
        .as_table_like_mut()
        .ok_or_else(|| eyre!("gateway must be a TOML table"))?;
      if config.gateway.omp != self.config.gateway.omp {
        let omp = config.gateway.omp.to_str().ok_or_else(|| eyre!("omp path is not valid UTF-8"))?;
        set_table_value(gateway, "omp", Some(omp));
      }
      if config.gateway.model != self.config.gateway.model {
        set_table_value(gateway, "model", config.gateway.model.as_deref());
      }
      if config.gateway.sessions_dir != self.config.gateway.sessions_dir {
        let sessions = config
          .gateway
          .sessions_dir
          .as_ref()
          .map(|path| path.to_str().ok_or_else(|| eyre!("sessions directory is not valid UTF-8")))
          .transpose()?;
        set_table_value(gateway, "sessions_dir", sessions);
      }
    }
    let parent = target
      .parent()
      .filter(|path| !path.as_os_str().is_empty())
      .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
      ".alvin-config-{}-{}.toml",
      std::process::id(),
      std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos()
    ));
    let write_result = (|| -> eyre::Result<(AppConfig, Settings)> {
      let mut options = std::fs::OpenOptions::new();
      options.write(true).create_new(true);
      #[cfg(unix)]
      {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
      }
      let mut file = options.open(&temporary)?;
      match std::fs::metadata(&target) {
        Ok(metadata) => file.set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
      }
      file.write_all(document.to_string().as_bytes())?;
      file.sync_all()?;
      let files: Vec<_> = self
        .files
        .iter()
        .map(|path| if path == &self.path { temporary.clone() } else { path.clone() })
        .collect();
      let effective = load(self.flags.clone(), &files)?;
      let gateway = settings(effective.clone(), &self.home, |name| std::env::var_os(name))?;
      Appearance::parse(&effective.appearance)?;
      std::fs::rename(&temporary, &target)?;
      Ok((effective, gateway))
    })();
    if write_result.is_err() && temporary.exists() {
      std::fs::remove_file(&temporary)?;
    }
    let (effective, gateway) = write_result?;
    self.config = effective;
    self.gateway = gateway;
    Ok(())
  }
}

fn set_value(item: &mut toml_edit::Item, value: Option<&str>) {
  if let Some(value) = value {
    if item.as_str() == Some(value) {
      return;
    }
    let decor = item.as_value().map(|value| value.decor().clone());
    *item = toml_edit::value(value);
    if let (Some(decor), Some(value)) = (decor, item.as_value_mut()) {
      *value.decor_mut() = decor;
    }
  } else {
    *item = toml_edit::Item::None;
  }
}

fn set_table_value(table: &mut dyn toml_edit::TableLike, key: &str, value: Option<&str>) {
  match value {
    Some(value) => {
      if let Some(item) = table.get_mut(key) {
        set_value(item, Some(value));
      } else {
        table.insert(key, toml_edit::value(value));
      }
    }
    None => {
      table.remove(key);
    }
  }
}
/// Converts the loaded config once. Only checks syntax: the endpoint is a WebSocket URL, and a remote endpoint is not
/// combined with settings of an embedded gateway.
fn settings(config: AppConfig, home: &Path, var: impl Fn(&str) -> Option<OsString>) -> eyre::Result<Settings> {
  match config.endpoint {
    Some(endpoint) => {
      if !matches!(endpoint.scheme(), "ws" | "wss") {
        bail!("`endpoint` must be a ws:// or wss:// URL, not {}://", endpoint.scheme());
      }
      if config.gateway.model.is_some() || config.gateway.sessions_dir.is_some() {
        bail!(
          "`endpoint` connects to a running gateway, which is configured where it runs; remove `gateway.model` and `gateway.sessions_dir`"
        );
      }
      Ok(Settings::Remote(endpoint))
    }
    None => Ok(Settings::Embedded {
      omp: config.gateway.omp,
      model: config.gateway.model,
      sessions_dir: config
        .gateway
        .sessions_dir
        .unwrap_or_else(|| alvin_gateway::sessions_dir(var, home)),
    }),
  }
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
      let dir = std::env::temp_dir().join(format!("alvin-app-config-{}", EventId::new().as_uuid()));
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

  fn no_flags() -> <AppConfig as confique::Config>::Layer {
    <AppConfig as confique::Config>::Layer::empty()
  }

  fn no_env(_: &str) -> Option<OsString> {
    None
  }

  #[test]
  fn saving_preserves_comments_and_other_settings() {
    let tmp = Scratch::new();
    let path = tmp.file(
      "config.toml",
      "# header\nappearance = 'system' # theme\n[gateway]\nomp = '/opt/omp' # executable\nmodel = 'provider/model'\n",
    );
    let files = vec![path.clone()];
    let config = load(no_flags(), &files).unwrap();
    let gateway = embedded(config.clone());
    let mut configuration = Configuration {
      config,
      gateway,
      path: path.clone(),
      files,
      flags: no_flags(),
      home: tmp.0.clone(),
    };
    let mut next = configuration.config.clone();
    next.appearance = "dark".into();
    configuration.save(next).unwrap();
    let saved = std::fs::read_to_string(path).unwrap();
    assert!(saved.contains("# header"));
    assert!(saved.contains("# theme"));
    assert!(saved.contains("omp = '/opt/omp' # executable"));
    assert!(saved.contains("model = 'provider/model'"));
    assert_eq!(configuration.config.appearance, "dark");
  }

  #[test]
  fn invalid_edit_does_not_replace_config_file() {
    let tmp = Scratch::new();
    let contents = "# keep me\nappearance = 'system'\n";
    let path = tmp.file("config.toml", contents);
    let files = vec![path.clone()];
    let config = load(no_flags(), &files).unwrap();
    let gateway = embedded(config.clone());
    let mut configuration = Configuration {
      config,
      gateway,
      path: path.clone(),
      files,
      flags: no_flags(),
      home: tmp.0.clone(),
    };
    let mut next = configuration.config.clone();
    next.endpoint = Some("https://example.com".parse().unwrap());
    assert!(configuration.save(next).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
  }

  fn embedded(config: AppConfig) -> Settings {
    settings(config, Path::new("/home/me"), no_env).unwrap()
  }

  #[test]
  fn with_no_source_at_all_the_app_embeds_a_gateway_over_omps_default_sessions() {
    let tmp = Scratch::new();
    let config = load(no_flags(), &[tmp.0.join("absent.toml")]).unwrap();
    assert_eq!(
      embedded(config),
      Settings::Embedded {
        omp: PathBuf::from("omp"),
        model: None,
        sessions_dir: PathBuf::from("/home/me/.omp/agent/sessions"),
      }
    );
  }

  #[test]
  fn an_earlier_file_wins_over_a_later_one_and_flags_win_over_both() {
    let tmp = Scratch::new();
    let first = tmp.file("first.toml", "[gateway]\nmodel = \"zai/first\"\n");
    let second = tmp.file("second.toml", "[gateway]\nmodel = \"zai/second\"\nomp = \"/opt/omp\"\n");
    let files = [first, second];

    let config = load(no_flags(), &files).unwrap();
    assert_eq!(config.gateway.model.as_deref(), Some("zai/first"));
    assert_eq!(config.gateway.omp, PathBuf::from("/opt/omp"));

    let mut flags = no_flags();
    flags.gateway.model = Some("zai/flag".to_owned());
    assert_eq!(load(flags, &files).unwrap().gateway.model.as_deref(), Some("zai/flag"));
  }

  #[test]
  fn the_explicit_config_file_comes_before_the_user_files() {
    let home = Path::new("/home/me");
    let files = config_files(Some(PathBuf::from("/etc/alvin.toml")), Some(PathBuf::from("/home/me/cfg")), home);
    assert_eq!(files.first(), Some(&PathBuf::from("/etc/alvin.toml")));
    assert_eq!(files.last(), Some(&PathBuf::from("/home/me/cfg/alvin/config.toml")));
  }

  #[test]
  fn an_unknown_key_is_an_error() {
    let tmp = Scratch::new();
    let file = tmp.file("typo.toml", "[gateway]\nmodle = \"zai/x\"\n");
    assert!(load(no_flags(), &[file]).is_err());
  }

  #[test]
  fn a_malformed_file_is_an_error_not_a_skip() {
    let tmp = Scratch::new();
    let file = tmp.file("bad.toml", "endpoint = [not toml\n");
    assert!(load(no_flags(), &[file]).is_err());
  }

  #[test]
  fn an_endpoint_selects_a_remote_gateway() {
    let tmp = Scratch::new();
    let file = tmp.file("remote.toml", "endpoint = \"ws://127.0.0.1:7410\"\n");
    let config = load(no_flags(), &[file]).unwrap();
    assert_eq!(embedded(config), Settings::Remote("ws://127.0.0.1:7410".parse().unwrap()));
  }

  #[test]
  fn an_endpoint_must_be_a_websocket_url() {
    let tmp = Scratch::new();
    let file = tmp.file("http.toml", "endpoint = \"http://127.0.0.1:7410\"\n");
    let config = load(no_flags(), &[file]).unwrap();
    let error = settings(config, Path::new("/home/me"), no_env).unwrap_err();
    assert!(error.to_string().contains("ws://"), "{error}");
  }

  #[test]
  fn an_endpoint_together_with_embedded_gateway_keys_is_an_error_naming_both() {
    let tmp = Scratch::new();
    for key in ["model = \"zai/x\"", "sessions_dir = \"/s\""] {
      let file = tmp.file("conflict.toml", &format!("endpoint = \"ws://127.0.0.1:7410\"\n[gateway]\n{key}\n"));
      let config = load(no_flags(), &[file]).unwrap();
      let error = settings(config, Path::new("/home/me"), no_env).unwrap_err().to_string();
      assert!(
        error.contains("endpoint") && error.contains("gateway.model") && error.contains("gateway.sessions_dir"),
        "{error}"
      );
    }
  }

  #[test]
  fn a_configured_sessions_directory_overrides_omps_rules() {
    let tmp = Scratch::new();
    let file = tmp.file("dir.toml", "[gateway]\nsessions_dir = \"/srv/sessions\"\n");
    let config = load(no_flags(), &[file]).unwrap();
    let vars = |name: &str| (name == "PI_CODING_AGENT_DIR").then(|| OsString::from("/srv/agent"));
    assert_eq!(
      settings(config, Path::new("/home/me"), vars).unwrap(),
      Settings::Embedded {
        omp: PathBuf::from("omp"),
        model: None,
        sessions_dir: PathBuf::from("/srv/sessions"),
      }
    );
  }

  #[test]
  fn omps_environment_decides_the_sessions_directory_when_none_is_configured() {
    let tmp = Scratch::new();
    let config = load(no_flags(), &[tmp.0.join("absent.toml")]).unwrap();
    let vars = |name: &str| (name == "PI_CODING_AGENT_DIR").then(|| OsString::from("/srv/agent"));
    let Settings::Embedded { sessions_dir, .. } = settings(config, Path::new("/home/me"), vars).unwrap() else {
      panic!("expected an embedded gateway");
    };
    assert_eq!(sessions_dir, PathBuf::from("/srv/agent/sessions"));
  }

  #[test]
  fn an_embedded_gateway_gets_its_settings() {
    let config = gateway_config(Path::new("/opt/omp"), Some("zai/x"), Path::new("/s"));
    assert_eq!(config.sessions_dir, PathBuf::from("/s"));
    assert_eq!(config.default_model.as_deref(), Some("zai/x"));
  }
}
