//! Desktop settings loaded from command-line flags, environment, and config files.

use std::path::{Path, PathBuf};

use confique::Config as _;
use eyre::{bail, eyre};
use gpui_kit::{Hsla, Rgba, rgba};
use url::Url;

/// Verdant style keys offered for interface colors.
pub(crate) const COLOR_CHOICES: &[ColorChoice] = &[
  ColorChoice::Style("markup.link.url"),
  ColorChoice::Style("constructor.lua"),
  ColorChoice::Style("attribute.builtin"),
  ColorChoice::Style("keyword"),
  ColorChoice::Style("diff.minus"),
  ColorChoice::Style("markup.italic"),
  ColorChoice::Style("attribute"),
  ColorChoice::Style("type"),
  ColorChoice::Style("string"),
  ColorChoice::Style("character"),
  ColorChoice::Style("constructor"),
  ColorChoice::OPERATOR,
  ColorChoice::Style("function"),
  ColorChoice::MODULE,
];

/// A Verdant style key, an explicit RGBA color, or the user-message accent sentinel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
  /// A theme-dependent Verdant style key or the `accent` sentinel.
  Style(&'static str),
  /// An explicit color packed as `0xRRGGBBAA`.
  Rgba(u32),
}

impl ColorChoice {
  pub(crate) const MODULE: Self = Self::Style("module");
  pub(crate) const OPERATOR: Self = Self::Style("operator");
  pub(crate) const ACCENT: Self = Self::Style("accent");

  /// Converts a picker color to eight-bit RGBA channels.
  pub fn from_color(color: Hsla) -> Self {
    let color = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    Self::Rgba((channel(color.r) << 24) | (channel(color.g) << 16) | (channel(color.b) << 8) | channel(color.a))
  }

  /// Returns the explicit color, or `None` for a theme-dependent choice.
  pub fn color(self) -> Option<Hsla> {
    match self {
      Self::Rgba(color) => Some(rgba(color).into()),
      Self::Style(_) => None,
    }
  }

  /// Returns the theme key or accent sentinel, or `None` for an explicit color.
  pub fn style_key(self) -> Option<&'static str> {
    match self {
      Self::Style(key) => Some(key),
      Self::Rgba(_) => None,
    }
  }
}

impl std::str::FromStr for ColorChoice {
  type Err = String;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    if let Some(hex) = value.strip_prefix('#') {
      if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("unknown color choice: {value}"));
      }
      let color = match hex.len() {
        6 => u32::from_str_radix(hex, 16).map(|rgb| (rgb << 8) | 0xff),
        8 => u32::from_str_radix(hex, 16),
        _ => return Err(format!("unknown color choice: {value}")),
      };
      return color.map(Self::Rgba).map_err(|_| format!("unknown color choice: {value}"));
    }
    COLOR_CHOICES
      .iter()
      .copied()
      .chain([Self::ACCENT])
      .find(|choice| choice.style_key() == Some(value))
      .ok_or_else(|| format!("unknown color choice: {value}"))
  }
}

impl std::fmt::Display for ColorChoice {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::Style(key) => f.write_str(key),
      Self::Rgba(color) if color & 0xff == 0xff => write!(f, "#{:06x}", color >> 8),
      Self::Rgba(color) => write!(f, "#{color:08x}"),
    }
  }
}

impl<'de> confique::serde::Deserialize<'de> for ColorChoice {
  fn deserialize<D: confique::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let value = <String as confique::serde::Deserialize>::deserialize(deserializer)?;
    value.parse().map_err(confique::serde::de::Error::custom)
  }
}

impl confique::serde::Serialize for ColorChoice {
  fn serialize<S: confique::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(self)
  }
}

/// What the app reads from its sources.
#[derive(confique::Config, Debug, Clone)]
#[config(layer_attr(derive(clap::Args, Clone)))]
// A key nothing reads is a typo; confique would otherwise skip it silently.
#[config(layer_attr(serde(deny_unknown_fields)))]
pub struct AppConfig {
  /// Connect to a running gateway (`ws://` or `wss://`).
  #[config(default = "ws://127.0.0.1:7410", env = "ALVIN_ENDPOINT")]
  #[config(layer_attr(arg(long)))]
  pub endpoint: Url,
  /// Interface appearance: system, light, or dark.
  #[config(default = "system", env = "ALVIN_APPEARANCE")]
  #[config(layer_attr(arg(long)))]
  pub appearance: String,
  /// Verdant theme used in light appearance.
  #[config(default = "catppuccin::latte", env = "ALVIN_LIGHT_THEME")]
  #[config(layer_attr(arg(long)))]
  pub light_theme: String,
  /// Verdant theme used in dark appearance.
  #[config(default = "catppuccin::mocha", env = "ALVIN_DARK_THEME")]
  #[config(layer_attr(arg(long)))]
  pub dark_theme: String,
  /// Verdant style key or custom `#RRGGBB` / `#RRGGBBAA` primary color.
  #[config(default = "module", env = "ALVIN_ACCENT_COLOR")]
  #[config(layer_attr(arg(long)))]
  pub accent_color: ColorChoice,
  /// Verdant style key, custom hex color, or `accent` to follow the accent.
  #[config(default = "operator", env = "ALVIN_USER_MESSAGE_COLOR")]
  #[config(layer_attr(arg(long)))]
  pub user_message_color: ColorChoice,
  /// Terminal font family; absent follows Ghostty or the app's monospace default.
  #[config(env = "ALVIN_TERMINAL_FONT_FAMILY")]
  #[config(layer_attr(arg(long)))]
  pub terminal_font_family: Option<String>,
  /// Terminal font size in logical pixels.
  #[config(env = "ALVIN_TERMINAL_FONT_SIZE")]
  #[config(layer_attr(arg(long)))]
  pub terminal_font_size: Option<f32>,
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

impl Cli {
  /// Merges the configuration sources.
  pub(crate) fn settings(self) -> eyre::Result<Configuration> {
    let home = dirs::home_dir().ok_or_else(|| eyre!("cannot determine the home directory"))?;
    let files = config_files(self.config_file, dirs::config_dir(), &home);
    let path = files
      .first()
      .cloned()
      .ok_or_else(|| eyre!("cannot determine the config directory"))?;
    let config = load(self.config.clone(), &files)?;
    validate_endpoint(&config.endpoint)?;
    Appearance::parse(&config.appearance)?;
    validate_themes(&config)?;
    Ok(Configuration {
      config,
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

fn validate_themes(config: &AppConfig) -> eyre::Result<()> {
  if config.accent_color == ColorChoice::ACCENT {
    bail!("accent_color must be a Verdant color key or custom hex color");
  }
  for name in [&config.light_theme, &config.dark_theme] {
    if !verdant_themes::THEMES.contains(&name.as_str()) {
      bail!("unknown Verdant theme: {name}");
    }
  }
  Ok(())
}

/// Config sources and the currently effective settings.
pub(crate) struct Configuration {
  pub(crate) config: AppConfig,
  path: PathBuf,
  files: Vec<PathBuf>,
  flags: <AppConfig as confique::Config>::Layer,
  home: PathBuf,
}

impl Configuration {
  pub(crate) fn project_preferences_path(&self) -> PathBuf {
    self.path.with_file_name("projects.json")
  }

  pub(crate) fn home(&self) -> &std::path::Path {
    &self.home
  }

  #[cfg(test)]
  pub(crate) fn for_test(path: PathBuf) -> eyre::Result<Self> {
    use confique::Layer as _;

    let flags = <AppConfig as confique::Config>::Layer::empty();
    let files = vec![path.clone()];
    let config = load(flags.clone(), &files)?;
    let home = path.parent().ok_or_else(|| eyre!("config path has no parent"))?.to_path_buf();
    validate_endpoint(&config.endpoint)?;
    Ok(Self {
      config,
      path,
      files,
      flags,
      home,
    })
  }

  /// Writes only known keys while retaining the rest of the TOML document.
  pub(crate) fn save(&mut self, config: AppConfig) -> eyre::Result<()> {
    use std::io::Write as _;

    Appearance::parse(&config.appearance)?;
    validate_themes(&config)?;
    validate_endpoint(&config.endpoint)?;
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
      set_table_value(document.as_table_mut(), "endpoint", Some(config.endpoint.as_str()));
    }
    if config.appearance != self.config.appearance {
      set_table_value(document.as_table_mut(), "appearance", Some(&config.appearance));
    }
    if config.light_theme != self.config.light_theme {
      set_table_value(document.as_table_mut(), "light_theme", Some(&config.light_theme));
    }
    if config.dark_theme != self.config.dark_theme {
      set_table_value(document.as_table_mut(), "dark_theme", Some(&config.dark_theme));
    }
    if config.accent_color != self.config.accent_color {
      set_table_value(document.as_table_mut(), "accent_color", Some(&config.accent_color.to_string()));
    }
    if config.user_message_color != self.config.user_message_color {
      set_table_value(
        document.as_table_mut(),
        "user_message_color",
        Some(&config.user_message_color.to_string()),
      );
    }
    if config.terminal_font_family != self.config.terminal_font_family {
      set_table_value(
        document.as_table_mut(),
        "terminal_font_family",
        config.terminal_font_family.as_deref(),
      );
    }
    if config.terminal_font_size != self.config.terminal_font_size {
      if let Some(size) = config.terminal_font_size {
        let item = &mut document["terminal_font_size"];
        let decor = item.as_value().map(|value| value.decor().clone());
        *item = toml_edit::value(f64::from(size));
        if let (Some(decor), Some(value)) = (decor, item.as_value_mut()) {
          *value.decor_mut() = decor;
        }
      } else {
        document.remove("terminal_font_size");
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
    let write_result = (|| -> eyre::Result<AppConfig> {
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
      validate_endpoint(&effective.endpoint)?;
      Appearance::parse(&effective.appearance)?;
      validate_themes(&effective)?;
      std::fs::rename(&temporary, &target)?;
      Ok(effective)
    })();
    if write_result.is_err() && temporary.exists() {
      std::fs::remove_file(&temporary)?;
    }
    self.config = write_result?;
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
fn validate_endpoint(endpoint: &Url) -> eyre::Result<()> {
  if !matches!(endpoint.scheme(), "ws" | "wss") {
    bail!("`endpoint` must be a ws:// or wss:// URL, not {}://", endpoint.scheme());
  }
  Ok(())
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

  #[test]
  fn saving_preserves_comments_and_other_settings() {
    let tmp = Scratch::new();
    let path = tmp.file(
      "config.toml",
      "# header\nappearance = 'system' # theme\nendpoint = 'ws://localhost:7410' # server\n",
    );
    let files = vec![path.clone()];
    let config = load(no_flags(), &files).unwrap();
    let mut configuration = Configuration {
      config,
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
    assert!(saved.contains("endpoint = 'ws://localhost:7410' # server"));
    assert_eq!(configuration.config.appearance, "dark");
  }

  #[test]
  fn terminal_font_override_can_be_saved_and_cleared() {
    let tmp = Scratch::new();
    let path = tmp.file("config.toml", "# retained\n");
    let mut configuration = Configuration::for_test(path.clone()).unwrap();
    let mut next = configuration.config.clone();
    next.terminal_font_family = Some("Selected Font".into());
    configuration.save(next).unwrap();
    assert_eq!(
      Configuration::for_test(path.clone())
        .unwrap()
        .config
        .terminal_font_family
        .as_deref(),
      Some("Selected Font")
    );

    let mut next = configuration.config.clone();
    next.terminal_font_family = None;
    configuration.save(next).unwrap();
    assert_eq!(Configuration::for_test(path.clone()).unwrap().config.terminal_font_family, None);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# retained\n");
  }

  #[test]
  fn color_choices_default_persist_reload_and_obey_flags() {
    for &choice in COLOR_CHOICES.iter().chain([&ColorChoice::ACCENT]) {
      assert_eq!(choice.to_string().parse::<ColorChoice>().unwrap(), choice);
    }
    let tmp = Scratch::new();
    let path = tmp.file("config.toml", "# retained\n");
    let mut configuration = Configuration::for_test(path.clone()).unwrap();
    assert_eq!(configuration.config.accent_color, ColorChoice::MODULE);
    assert_eq!(configuration.config.user_message_color, ColorChoice::OPERATOR);
    let mut next = configuration.config.clone();
    next.accent_color = "function".parse().unwrap();
    next.user_message_color = ColorChoice::ACCENT;
    configuration.save(next).unwrap();
    let reloaded = Configuration::for_test(path.clone()).unwrap();
    assert_eq!(reloaded.config.accent_color.to_string(), "function");
    assert_eq!(reloaded.config.user_message_color, ColorChoice::ACCENT);
    let mut flags = no_flags();
    flags.accent_color = Some(ColorChoice::MODULE);
    assert_eq!(load(flags, std::slice::from_ref(&path)).unwrap().accent_color, ColorChoice::MODULE);
    std::fs::write(&path, "accent_color = 'not-a-key'\n").unwrap();
    assert!(load(no_flags(), &[path]).is_err());
  }

  #[test]
  fn custom_colors_round_trip_and_reject_invalid_hex() {
    for (input, expected) in [
      ("#123AbC", "#123abc"),
      ("#123abcff", "#123abc"),
      ("#123ABC80", "#123abc80"),
      ("#00000000", "#00000000"),
    ] {
      let choice: ColorChoice = input.parse().unwrap();
      assert_eq!(choice.to_string(), expected);
      assert_eq!(ColorChoice::from_color(choice.color().unwrap()), choice);
      assert_eq!(choice.style_key(), None);
      let json = serde_json::to_string(&choice).unwrap();
      assert_eq!(serde_json::from_str::<ColorChoice>(&json).unwrap(), choice);
    }
    for input in ["#123", "#12345", "#1234567", "#123456789", "#12345g", "#+12345", "#é1234", "123456"] {
      assert!(input.parse::<ColorChoice>().is_err(), "{input}");
    }
  }

  #[test]
  fn custom_colors_save_reload_and_preserve_flag_precedence() {
    let tmp = Scratch::new();
    let path = tmp.file("config.toml", "# retained\n");
    let mut configuration = Configuration::for_test(path.clone()).unwrap();
    let mut next = configuration.config.clone();
    next.accent_color = "#123abc80".parse().unwrap();
    next.user_message_color = "#fedcba00".parse().unwrap();
    configuration.save(next).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("#123abc80"));
    assert!(saved.contains("#fedcba00"));
    let reloaded = Configuration::for_test(path.clone()).unwrap();
    assert_eq!(reloaded.config.accent_color.to_string(), "#123abc80");
    assert_eq!(reloaded.config.user_message_color.to_string(), "#fedcba00");
    let mut flags = no_flags();
    flags.accent_color = Some("#aabbcc".parse().unwrap());
    assert_eq!(
      load(flags, std::slice::from_ref(&path)).unwrap().accent_color.to_string(),
      "#aabbcc"
    );

    let mut invalid = configuration.config.clone();
    invalid.accent_color = ColorChoice::ACCENT;
    assert!(configuration.save(invalid).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
  }

  #[test]
  fn theme_preferences_persist_and_invalid_names_are_rejected() {
    let tmp = Scratch::new();
    let path = tmp.file("config.toml", "# retained\n");
    let files = vec![path.clone()];
    let config = load(no_flags(), &files).unwrap();
    let mut configuration = Configuration {
      config,
      path: path.clone(),
      files,
      flags: no_flags(),
      home: tmp.0.clone(),
    };
    let mut next = configuration.config.clone();
    next.light_theme = "github::light".into();
    next.dark_theme = "one::cool".into();
    configuration.save(next).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    let reloaded = Configuration::for_test(path.clone()).unwrap();
    assert_eq!(reloaded.config.light_theme, "github::light");
    assert_eq!(reloaded.config.dark_theme, "one::cool");
    let mut invalid = configuration.config.clone();
    invalid.dark_theme = "not-a-theme".into();
    assert!(configuration.save(invalid).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), saved);
  }

  #[test]
  fn invalid_edit_does_not_replace_config_file() {
    let tmp = Scratch::new();
    let contents = "# keep me\nappearance = 'system'\n";
    let path = tmp.file("config.toml", contents);
    let files = vec![path.clone()];
    let config = load(no_flags(), &files).unwrap();
    let mut configuration = Configuration {
      config,
      path: path.clone(),
      files,
      flags: no_flags(),
      home: tmp.0.clone(),
    };
    let mut next = configuration.config.clone();
    next.endpoint = "https://example.com".parse().unwrap();
    assert!(configuration.save(next).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), contents);
  }

  #[test]
  fn default_endpoint_and_source_precedence() {
    let tmp = Scratch::new();
    let config = load(no_flags(), &[tmp.0.join("absent.toml")]).unwrap();
    assert_eq!(config.endpoint.as_str(), "ws://127.0.0.1:7410/");
    let first = tmp.file("first.toml", "endpoint = 'wss://first.example'\n");
    let second = tmp.file("second.toml", "endpoint = 'ws://second.example'\n");
    let files = [first, second];
    assert_eq!(load(no_flags(), &files).unwrap().endpoint.as_str(), "wss://first.example/");
    let mut flags = no_flags();
    flags.endpoint = Some("ws://flag.example".parse().unwrap());
    assert_eq!(load(flags, &files).unwrap().endpoint.as_str(), "ws://flag.example/");
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
  fn endpoint_rejects_invalid_and_non_websocket_urls() {
    let tmp = Scratch::new();
    for value in ["http://example.com", "not a url"] {
      let file = tmp.file("invalid.toml", &format!("endpoint = '{value}'\n"));
      assert!(Configuration::for_test(file).is_err());
    }
  }
}
