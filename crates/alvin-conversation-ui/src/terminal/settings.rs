//! Terminal appearance supplied by the application.

/// Cursor shape for a terminal's default cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCursor {
  /// Filled block.
  Block,
  /// Vertical bar.
  Bar,
  /// Underline.
  Underline,
  /// Hollow block.
  Hollow,
}

/// Overrides for the terminal grid, independent of the application theme.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TerminalSettings {
  /// Primary font family.
  pub font_family: Option<String>,
  /// Ordered font fallback families.
  pub font_fallbacks: Vec<String>,
  /// Font size in logical pixels.
  pub font_size: Option<f32>,
  /// Default foreground color.
  pub foreground: Option<[u8; 3]>,
  /// Default background color.
  pub background: Option<[u8; 3]>,
  /// Default cursor color.
  pub cursor_color: Option<[u8; 3]>,
  /// Selection foreground color.
  pub selection_foreground: Option<[u8; 3]>,
  /// Selection background color.
  pub selection_background: Option<[u8; 3]>,
  /// Explicit palette entries.
  pub palette: Vec<(u8, [u8; 3])>,
  /// Default cursor shape.
  pub cursor_shape: Option<TerminalCursor>,
  /// Default cursor blinking state.
  pub cursor_blink: Option<bool>,
}

/// Parse a Ghostty color value using the terminal engine's parser.
pub fn parse_terminal_color(value: &str) -> Result<[u8; 3], String> {
  ghostty_vt::style::RgbColor::parse(value)
    .map(|color| [color.r, color.g, color.b])
    .map_err(|error| error.to_string())
}

/// Parse a Ghostty indexed palette entry using the terminal engine's parser.
pub fn parse_terminal_palette_entry(value: &str) -> Result<(u8, [u8; 3]), String> {
  ghostty_vt::style::Palette::parse_palette_entry(value)
    .map(|(index, color)| (index.0, [color.r, color.g, color.b]))
    .map_err(|error| error.to_string())
}
