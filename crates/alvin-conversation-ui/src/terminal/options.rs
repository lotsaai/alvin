//! Terminal engine options.

/// Engine limits, with Ghostty's defaults: 10,000,000 bytes of scrollback
/// (`scrollback-limit` in `ghostty +show-config --default`), so a busy
/// terminal holds as much history as Ghostty's and no more.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EngineOptions {
  /// Maximum scrollback in bytes. Zero disables scrollback.
  pub(crate) scrollback_bytes: usize,
  /// Maximum scrollback in lines; `None` is unlimited.
  pub(crate) scrollback_lines: Option<usize>,
  /// The terminfo name reported to XTGETTCAP.
  pub(crate) terminfo_name: String,
  /// Maximum decoded bytes per Kitty clipboard write.
  pub(crate) clipboard_write_max_bytes: usize,
}

impl Default for EngineOptions {
  fn default() -> Self {
    Self {
      scrollback_bytes: 10_000_000,
      scrollback_lines: None,
      terminfo_name: ghostty_vt::terminfo::TERM.to_owned(),
      clipboard_write_max_bytes: 64 << 20,
    }
  }
}

#[cfg(test)]
mod tests {
  #[test]
  fn the_scrollback_limit_is_ghosttys_default() {
    assert_eq!(super::EngineOptions::default().scrollback_bytes, 10_000_000);
  }
}
