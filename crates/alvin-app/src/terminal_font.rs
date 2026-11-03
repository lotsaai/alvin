//! Read the terminal appearance fields from Ghostty's local configuration.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use alvin_conversation_ui::{TerminalCursor, TerminalSettings, parse_terminal_color, parse_terminal_palette_entry};
use eyre::{Context as _, bail};

#[derive(Default)]
struct Fields {
  entries: Vec<(String, String)>,
  theme: Option<String>,
}

fn read_file(
  path: &Path,
  optional: bool,
  loaded: &mut HashSet<PathBuf>,
  pending: &mut VecDeque<(PathBuf, bool, bool)>,
  default_file: bool,
  fields: &mut Fields,
  theme_file: bool,
) -> eyre::Result<()> {
  let text = match std::fs::read_to_string(path) {
    Ok(text) => text,
    Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
    Err(error) => return Err(error).wrap_err_with(|| format!("reading Ghostty config {}", path.display())),
  };
  let canonical = std::fs::canonicalize(path).wrap_err_with(|| format!("resolving Ghostty config {}", path.display()))?;
  if !loaded.insert(canonical) {
    if !default_file {
      bail!("Ghostty config include cycle or duplicate at {}", path.display());
    }
    return Ok(());
  }
  let mut includes = Vec::new();
  for line in text.lines().map(str::trim) {
    if line.is_empty() || line.starts_with('#') {
      continue;
    }
    let Some((key, raw)) = line.split_once('=') else { continue };
    let key = key.trim();
    let raw = raw.trim();
    let value = raw.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(raw);
    match key {
      "config-file" if !theme_file && value.is_empty() => {
        includes.clear();
        pending.retain(|(_, _, default_file)| *default_file);
      }
      "config-file" if !theme_file => {
        let optional_value = (!raw.starts_with('"')).then(|| value.strip_prefix('?')).flatten();
        let optional = optional_value.is_some();
        let value = optional_value.unwrap_or(value);
        let include = Path::new(value);
        let include = if include.is_absolute() {
          include.to_path_buf()
        } else {
          path.parent().unwrap_or(Path::new(".")).join(include)
        };
        includes.push((include, optional, false));
      }
      "theme" if !theme_file => fields.theme = (!value.is_empty()).then(|| value.to_owned()),
      "font-family"
      | "font-size"
      | "foreground"
      | "background"
      | "cursor-color"
      | "selection-foreground"
      | "selection-background"
      | "palette"
      | "cursor-style"
      | "cursor-style-blink" => fields.entries.push((key.to_owned(), value.to_owned())),
      _ => {}
    }
  }
  pending.extend(includes);
  Ok(())
}

fn read_files(paths: Vec<PathBuf>) -> (Fields, Vec<String>) {
  let mut fields = Fields::default();
  let mut warnings = Vec::new();
  let mut pending: VecDeque<_> = paths.into_iter().map(|path| (path, true, true)).collect();
  let mut loaded = HashSet::new();
  while let Some((path, optional, default_file)) = pending.pop_front() {
    if let Err(error) = read_file(&path, optional, &mut loaded, &mut pending, default_file, &mut fields, false) {
      warnings.push(format!("Couldn’t read Ghostty appearance: {error}"));
    }
  }
  (fields, warnings)
}

/// Select configured installed families without substituting an unrelated font.
pub(crate) fn resolve_families(
  settings: &mut TerminalSettings,
  previous: &TerminalSettings,
  installed: &[String],
  override_family: Option<&str>,
) -> Vec<String> {
  if let Some(family) = override_family {
    settings.font_family = Some(family.to_owned());
    settings.font_fallbacks.clear();
  }
  if override_family.is_some()
    && let Some(family) = settings.font_family.as_ref().filter(|family| !installed.contains(family))
  {
    let missing = vec![family.clone()];
    settings.font_family.clone_from(&previous.font_family);
    settings.font_fallbacks.clone_from(&previous.font_fallbacks);
    return missing;
  }

  let mut missing = Vec::new();
  let families = settings
    .font_family
    .take()
    .into_iter()
    .chain(std::mem::take(&mut settings.font_fallbacks));
  for family in families {
    if !installed.contains(&family) {
      missing.push(family);
    } else if settings.font_family.is_none() {
      settings.font_family = Some(family);
    } else {
      settings.font_fallbacks.push(family);
    }
  }
  if settings.font_family.is_none() && !missing.is_empty() {
    settings.font_family.clone_from(&previous.font_family);
    settings.font_fallbacks.clone_from(&previous.font_fallbacks);
  }
  missing
}

fn apply(settings: &mut TerminalSettings, entries: &[(String, String)]) -> eyre::Result<()> {
  for (key, value) in entries {
    let color = || parse_terminal_color(value).map_err(|error| eyre::eyre!("invalid Ghostty {key}: {error}"));
    match key.as_str() {
      "font-family" if value.is_empty() => {
        settings.font_family = None;
        settings.font_fallbacks.clear();
      }
      "font-family" if settings.font_family.is_none() => settings.font_family = Some(value.clone()),
      "font-family" => settings.font_fallbacks.push(value.clone()),
      "font-size" if value.is_empty() => settings.font_size = None,
      "font-size" => {
        let points: f32 = value.parse().wrap_err("invalid Ghostty font-size")?;
        if !points.is_finite() || points <= 0.0 {
          bail!("invalid Ghostty font-size");
        }
        settings.font_size = Some(points * if cfg!(target_os = "macos") { 1.0 } else { 96.0 / 72.0 });
      }
      "foreground" => settings.foreground = if value.is_empty() { None } else { Some(color()?) },
      "background" => settings.background = if value.is_empty() { None } else { Some(color()?) },
      "cursor-color" => settings.cursor_color = if value.is_empty() { None } else { Some(color()?) },
      "selection-foreground" => settings.selection_foreground = if value.is_empty() { None } else { Some(color()?) },
      "selection-background" => settings.selection_background = if value.is_empty() { None } else { Some(color()?) },
      "palette" if value.is_empty() => settings.palette.clear(),
      "palette" => settings
        .palette
        .push(parse_terminal_palette_entry(value).map_err(|error| eyre::eyre!("invalid Ghostty palette: {error}"))?),
      "cursor-style" if value.is_empty() => settings.cursor_shape = None,
      "cursor-style" => {
        settings.cursor_shape = Some(match value.as_str() {
          "block" => TerminalCursor::Block,
          "bar" => TerminalCursor::Bar,
          "underline" => TerminalCursor::Underline,
          "block_hollow" => TerminalCursor::Hollow,
          _ => bail!("unsupported Ghostty cursor-style {value}"),
        })
      }
      "cursor-style-blink" if value.is_empty() => settings.cursor_blink = None,
      "cursor-style-blink" => settings.cursor_blink = Some(value.parse().wrap_err("invalid Ghostty cursor-style-blink")?),
      _ => {}
    }
  }
  Ok(())
}

fn config_paths(home: &Path, xdg: Option<&std::ffi::OsStr>, platform_config: Option<&Path>, macos: bool) -> (PathBuf, Vec<PathBuf>) {
  let base = match xdg {
    Some(value) if !value.is_empty() => PathBuf::from(value),
    Some(_) => home.join(".config"),
    None if macos => home.join(".config"),
    None => platform_config.map(Path::to_path_buf).unwrap_or_else(|| home.join(".config")),
  };
  let ghostty = base.join("ghostty");
  let mut paths = vec![ghostty.join("config"), ghostty.join("config.ghostty")];
  if macos {
    let app = platform_config
      .map(Path::to_path_buf)
      .unwrap_or_else(|| home.join("Library/Application Support"))
      .join("com.mitchellh.ghostty");
    paths.extend([app.join("config"), app.join("config.ghostty")]);
  }
  (ghostty, paths)
}

// Ghostty locates resources beside its executable using a terminfo sentinel:
// https://github.com/ghostty-org/ghostty/blob/main/src/os/resourcesdir.zig
fn bundled_resources(executable: &Path) -> Option<PathBuf> {
  let executable = std::fs::canonicalize(executable).ok()?;
  for ancestor in executable.ancestors().skip(1) {
    if cfg!(target_os = "macos") {
      let resources = ancestor.join("Contents/Resources");
      if resources.join("terminfo/78/xterm-ghostty").is_file() {
        return Some(resources.join("ghostty"));
      }
    }
    let share = ancestor.join(if cfg!(target_os = "freebsd") { "local/share" } else { "share" });
    let sentinels: &[&str] = if cfg!(windows) {
      &["terminfo/ghostty.terminfo"]
    } else if cfg!(target_os = "macos") {
      &["terminfo/78/xterm-ghostty"]
    } else if cfg!(target_os = "freebsd") {
      &["site-terminfo/g/ghostty", "site-terminfo/x/xterm-ghostty"]
    } else {
      &["terminfo/g/ghostty", "terminfo/x/xterm-ghostty"]
    };
    if sentinels.iter().any(|sentinel| share.join(sentinel).is_file()) {
      return Some(share.join("ghostty"));
    }
  }
  None
}

fn installed_resources(path: &std::ffi::OsStr) -> Option<PathBuf> {
  let name = if cfg!(windows) { "ghostty.exe" } else { "ghostty" };
  std::env::split_paths(path)
    .map(|dir| dir.join(name))
    .find(|executable| executable.is_file())
    .and_then(|executable| bundled_resources(&executable))
}

pub(crate) fn load(home: &Path, dark: bool) -> (TerminalSettings, Option<String>) {
  let xdg = std::env::var_os("XDG_CONFIG_HOME");
  let platform_config = if cfg!(windows) {
    dirs::config_local_dir()
  } else {
    dirs::config_dir()
  };
  let (ghostty, paths) = config_paths(home, xdg.as_deref(), platform_config.as_deref(), cfg!(target_os = "macos"));
  let (fields, mut warnings) = read_files(paths);
  let mut settings = TerminalSettings::default();
  if let Some(theme) = &fields.theme {
    let name = if theme.starts_with("light:") || theme.starts_with("dark:") {
      let prefix = if dark { "dark:" } else { "light:" };
      theme
        .split(',')
        .map(str::trim)
        .find_map(|entry| entry.strip_prefix(prefix))
        .unwrap_or("")
        .trim()
    } else {
      theme.as_str()
    };
    let name_path = Path::new(name);
    let candidates = if name_path.is_absolute() {
      vec![name_path.to_path_buf()]
    } else {
      let mut candidates = vec![ghostty.join("themes").join(name)];
      let resources = std::env::var_os("GHOSTTY_RESOURCES_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("PATH").and_then(|path| installed_resources(&path)));
      if let Some(resources) = resources {
        candidates.push(resources.join("themes").join(name));
      }
      candidates
    };
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
      let mut theme_fields = Fields::default();
      if let Err(error) = read_file(
        path,
        false,
        &mut HashSet::new(),
        &mut VecDeque::new(),
        false,
        &mut theme_fields,
        true,
      ) {
        warnings.push(format!("Couldn’t read Ghostty theme: {error}"));
      }
      for entry in &theme_fields.entries {
        if let Err(error) = apply(&mut settings, std::slice::from_ref(entry)) {
          warnings.push(error.to_string());
        }
      }
    } else {
      warnings.push(format!("Ghostty theme {name:?} could not be found; terminal colors may differ."));
    }
  }
  for entry in &fields.entries {
    if let Err(error) = apply(&mut settings, std::slice::from_ref(entry)) {
      warnings.push(error.to_string());
    }
  }
  (settings, (!warnings.is_empty()).then(|| warnings.join(" ")))
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use alvin_conversation_ui::TerminalSettings;

  use super::{apply, config_paths, installed_resources, read_files, resolve_families};

  struct TempDir(PathBuf);

  impl TempDir {
    fn new() -> Self {
      let path = std::env::temp_dir().join(format!("alvin-terminal-config-{}", alvin_protocol::EventId::new().as_uuid()));
      std::fs::create_dir_all(&path).unwrap();
      Self(path)
    }
  }

  impl Drop for TempDir {
    fn drop(&mut self) {
      let _ = std::fs::remove_dir_all(&self.0);
    }
  }

  #[test]
  fn includes_are_loaded_after_parent_and_font_reset_clears_fallbacks() {
    let dir = TempDir::new();
    let main = dir.0.join("config");
    std::fs::write(&main, "font-family = Old\nconfig-file = included\nfont-family = Before\n").unwrap();
    std::fs::write(
      dir.0.join("included"),
      "font-family = \"\"\nfont-family = New\nfont-family = Fallback\n",
    )
    .unwrap();
    let (fields, warnings) = read_files(vec![main]);
    assert!(warnings.is_empty());
    let mut settings = TerminalSettings::default();
    apply(&mut settings, &fields.entries).unwrap();
    assert_eq!(settings.font_family.as_deref(), Some("New"));
    assert_eq!(settings.font_fallbacks, ["Fallback"]);
  }

  #[test]
  fn theme_values_are_overridden_by_explicit_config_colors() {
    let mut settings = TerminalSettings::default();
    apply(
      &mut settings,
      &[("background".into(), "112233".into()), ("foreground".into(), "ffffff".into())],
    )
    .unwrap();
    apply(&mut settings, &[("background".into(), "445566".into())]).unwrap();
    assert_eq!(settings.background, Some([0x44, 0x55, 0x66]));
    assert_eq!(settings.foreground, Some([0xff, 0xff, 0xff]));
  }

  #[test]
  fn required_missing_include_and_cycles_report_errors() {
    let dir = TempDir::new();
    let main = dir.0.join("config");
    std::fs::write(&main, "config-file = absent\n").unwrap();
    assert!(!read_files(vec![main.clone()]).1.is_empty());
    std::fs::write(&main, "config-file = config\n").unwrap();
    assert!(!read_files(vec![main]).1.is_empty());
  }

  #[test]
  fn includes_follow_all_default_files_in_queue_order() {
    let dir = TempDir::new();
    let legacy = dir.0.join("config");
    let preferred = dir.0.join("config.ghostty");
    std::fs::write(&legacy, "background = 111111\nconfig-file = first\n").unwrap();
    std::fs::write(&preferred, "background = 222222\nconfig-file = second\n").unwrap();
    std::fs::write(dir.0.join("first"), "background = 333333\nconfig-file = nested\n").unwrap();
    std::fs::write(dir.0.join("second"), "background = 444444\n").unwrap();
    std::fs::write(dir.0.join("nested"), "background = 555555\n").unwrap();
    let (fields, warnings) = read_files(vec![legacy, preferred]);
    assert!(warnings.is_empty());
    let mut settings = TerminalSettings::default();
    apply(&mut settings, &fields.entries).unwrap();
    assert_eq!(settings.background, Some([0x55, 0x55, 0x55]));
  }

  #[test]
  fn optional_missing_include_is_not_an_error() {
    let dir = TempDir::new();
    let main = dir.0.join("config");
    std::fs::write(&main, "config-file = ?absent\nfont-family = Example\n").unwrap();
    let (fields, warnings) = read_files(vec![main]);
    assert!(warnings.is_empty());
    let mut settings = TerminalSettings::default();
    apply(&mut settings, &fields.entries).unwrap();
    assert_eq!(settings.font_family.as_deref(), Some("Example"));
  }

  #[test]
  fn empty_appearance_values_reset_previous_values() {
    let mut settings = TerminalSettings::default();
    apply(
      &mut settings,
      &[("background".into(), "123456".into()), ("palette".into(), "1=abcdef".into())],
    )
    .unwrap();
    apply(&mut settings, &[("background".into(), "".into()), ("palette".into(), "".into())]).unwrap();
    assert_eq!(settings.background, None);
    assert!(settings.palette.is_empty());
  }

  #[test]
  fn config_directories_follow_xdg_and_platform_precedence() {
    let dir = TempDir::new();
    let home = dir.0.join("home");
    let platform = dir.0.join("platform-config");
    let xdg = dir.0.join("xdg");
    let (ghostty, paths) = config_paths(&home, Some(xdg.as_os_str()), Some(&platform), true);
    assert_eq!(ghostty, xdg.join("ghostty"));
    assert_eq!(paths.first(), Some(&xdg.join("ghostty/config")));
    assert_eq!(paths.last(), Some(&platform.join("com.mitchellh.ghostty/config.ghostty")));

    let (ghostty, paths) = config_paths(&home, None, Some(&platform), false);
    assert_eq!(ghostty, platform.join("ghostty"));
    assert_eq!(paths.len(), 2);
    let (ghostty, _) = config_paths(&home, None, Some(&platform), true);
    assert_eq!(ghostty, home.join(".config/ghostty"));
    let (ghostty, _) = config_paths(&home, Some(std::ffi::OsStr::new("")), Some(&platform), false);
    assert_eq!(ghostty, home.join(".config/ghostty"));
  }

  #[test]
  fn unavailable_first_family_uses_next_configured_family_and_keeps_appearance() {
    let mut settings = TerminalSettings::default();
    apply(
      &mut settings,
      &[
        ("font-family".into(), "Monaspace Neon Var".into()),
        ("font-family".into(), "Monaspace Neon".into()),
        ("font-family".into(), "Other Installed".into()),
        ("background".into(), "123456".into()),
      ],
    )
    .unwrap();
    let installed = vec!["Monaspace Neon".to_owned(), "Other Installed".to_owned()];
    let missing = resolve_families(&mut settings, &TerminalSettings::default(), &installed, None);
    assert_eq!(missing, ["Monaspace Neon Var"]);
    assert_eq!(settings.font_family.as_deref(), Some("Monaspace Neon"));
    assert_eq!(settings.font_fallbacks, ["Other Installed"]);
    assert_eq!(settings.background, Some([0x12, 0x34, 0x56]));
  }

  #[test]
  fn explicit_missing_family_keeps_previous_font_but_not_previous_colors() {
    let mut previous = TerminalSettings::default();
    previous.font_family = Some("Previous".into());
    let mut settings = TerminalSettings::default();
    settings.font_family = Some("Missing".into());
    settings.font_fallbacks.push("Installed".into());
    settings.background = Some([1, 2, 3]);
    let missing = resolve_families(&mut settings, &previous, &["Installed".into()], Some("Missing"));
    assert_eq!(missing, ["Missing"]);
    assert_eq!(settings.font_family.as_deref(), Some("Previous"));
    assert_eq!(settings.background, Some([1, 2, 3]));
  }

  #[test]
  fn explicit_font_suppresses_inherited_families_until_override_is_cleared() {
    let mut inherited = TerminalSettings::default();
    apply(
      &mut inherited,
      &[
        ("font-family".into(), "Missing Ghostty".into()),
        ("font-family".into(), "Installed Ghostty".into()),
        ("background".into(), "123456".into()),
      ],
    )
    .unwrap();
    let installed = vec!["Installed Alvin".to_owned(), "Installed Ghostty".to_owned()];
    let mut selected = inherited.clone();
    let warnings = resolve_families(&mut selected, &TerminalSettings::default(), &installed, Some("Installed Alvin"));
    assert!(warnings.is_empty());
    assert_eq!(selected.font_family.as_deref(), Some("Installed Alvin"));
    assert!(selected.font_fallbacks.is_empty());
    assert_eq!(selected.background, Some([0x12, 0x34, 0x56]));

    let warnings = resolve_families(&mut inherited, &selected, &installed, None);
    assert_eq!(warnings, ["Missing Ghostty"]);
    assert_eq!(inherited.font_family.as_deref(), Some("Installed Ghostty"));
    assert_eq!(inherited.background, Some([0x12, 0x34, 0x56]));
  }

  #[test]
  fn bundled_resources_follow_installed_executable_and_terminfo() {
    let dir = TempDir::new();
    let root = if cfg!(target_os = "macos") {
      dir.0.join("Ghostty.app")
    } else {
      dir.0.join("prefix")
    };
    let executable_dir = root.join(if cfg!(target_os = "macos") { "Contents/MacOS" } else { "bin" });
    let name = if cfg!(windows) { "ghostty.exe" } else { "ghostty" };
    std::fs::create_dir_all(&executable_dir).unwrap();
    std::fs::write(executable_dir.join(name), "").unwrap();
    let resources = root.join(if cfg!(target_os = "macos") {
      "Contents/Resources"
    } else if cfg!(target_os = "freebsd") {
      "local/share"
    } else {
      "share"
    });
    let sentinel = if cfg!(windows) {
      "terminfo/ghostty.terminfo"
    } else if cfg!(target_os = "macos") {
      "terminfo/78/xterm-ghostty"
    } else if cfg!(target_os = "freebsd") {
      "site-terminfo/g/ghostty"
    } else {
      "terminfo/g/ghostty"
    };
    let sentinel = resources.join(sentinel);
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, "").unwrap();
    let path = std::env::join_paths([&executable_dir]).unwrap();
    assert_eq!(
      installed_resources(&path),
      Some(std::fs::canonicalize(&resources).unwrap().join("ghostty"))
    );
    std::fs::remove_file(sentinel).unwrap();
    assert_eq!(installed_resources(&path), None);
  }
}
