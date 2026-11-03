//! Font, padding and behavior settings for the element.
//!
//! Derived from Herdr (Apache-2.0).

use gpui_kit::{Font, FontFallbacks, FontFeatures, FontStyle, FontWeight, Hsla, Pixels, SharedString, Size, TextRun, Window, px, size};

use crate::terminal::frame::CursorShape;
use gpui_kit::component::theme::Theme;

/// Font, padding and behavior settings for a terminal's grid.
///
/// The font family, the font size and the padding follow the theme until
/// a builder sets them: the theme's code font, its code font size, and
/// `metrics.terminal_padding`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TerminalAppearance {
  pub(crate) font_family: Option<SharedString>,
  pub(crate) font_fallbacks: Vec<String>,
  pub(crate) font_size: Option<Pixels>,
  pub(crate) font_weight: FontWeight,
  pub(crate) cell_height_adjust: f32,
  pub(crate) cell_height_offset: Pixels,
  pub(crate) padding: Option<Pixels>,
  pub(crate) padding_balance: bool,
  pub(crate) cursor_blink: Option<bool>,
  pub(crate) cursor_shape: Option<CursorShape>,
  pub(crate) copy_on_select: bool,
  pub(crate) trim_trailing_spaces: bool,
}

impl Default for TerminalAppearance {
  fn default() -> Self {
    Self {
      font_family: None,
      font_fallbacks: Vec::new(),
      font_size: None,
      font_weight: FontWeight::NORMAL,
      cell_height_adjust: 0.0,
      cell_height_offset: px(0.),
      padding: None,
      padding_balance: false,
      cursor_blink: None,
      cursor_shape: None,
      copy_on_select: false,
      trim_trailing_spaces: true,
    }
  }
}

impl TerminalAppearance {
  #[cfg(test)]
  /// Set the extra cell height as a fraction of the natural height; 0.2
  /// adds a fifth.
  #[must_use]
  pub fn with_cell_height_adjust(mut self, fraction: f32) -> Self {
    self.cell_height_adjust = fraction;
    self
  }

  #[cfg(test)]
  /// Add `offset` to the cell height after the fractional adjustment, as
  /// Ghostty's `adjust-cell-height = 2` does. It may be negative.
  #[must_use]
  pub fn with_cell_height_offset(mut self, offset: impl Into<Pixels>) -> Self {
    self.cell_height_offset = offset.into();
    self
  }

  #[cfg(test)]
  /// Override the program's cursor blink request.
  #[must_use]
  pub fn with_cursor_blink(mut self, blink: Option<bool>) -> Self {
    self.cursor_blink = blink;
    self
  }

  /// The appearance with the theme's values in the slots left open.
  pub(crate) fn resolved(&self, theme: &Theme) -> Self {
    let mut resolved = self.clone();
    resolved.font_family.get_or_insert_with(|| theme.mono_font_family.clone());
    resolved.font_size.get_or_insert(theme.mono_font_size);
    resolved.padding.get_or_insert(px(0.));
    resolved
  }

  /// The font size, after [`Self::resolved`].
  pub(crate) fn size(&self) -> Pixels {
    self.font_size.unwrap_or_default()
  }

  /// The padding, after [`Self::resolved`].
  pub(crate) fn inset(&self) -> Pixels {
    self.padding.unwrap_or_default()
  }

  /// The grid font, after [`Self::resolved`]: ligatures off so glyphs
  /// stay on cell boundaries.
  pub(crate) fn font(&self) -> Font {
    let mut font = gpui_kit::font(self.font_family.clone().unwrap_or_default());
    if !self.font_fallbacks.is_empty() {
      font.fallbacks = Some(FontFallbacks::from_fonts(self.font_fallbacks.clone()));
    }
    font.features = FontFeatures::disable_ligatures();
    font.weight = self.font_weight;
    font.style = FontStyle::Normal;
    font
  }

  /// Cell height after the adjustment, snapped to a physical pixel so rows
  /// tile without seams.
  pub(crate) fn cell_height(&self, natural: Pixels, scale: f32) -> Pixels {
    let adjusted = f32::from(natural) * (1.0 + self.cell_height_adjust) + f32::from(self.cell_height_offset);
    let scale = if scale > 0.0 { scale } else { 1.0 };
    px(((adjusted * scale).round() / scale).max(1.0))
  }

  /// Measure one cell for this font, after [`Self::resolved`].
  pub(crate) fn cell_size(&self, window: &mut Window) -> Size<Pixels> {
    let font = self.font();
    let sample = window.text_system().shape_line(
      "M".into(),
      self.size(),
      &[TextRun {
        len: 1,
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
      }],
      None,
    );
    size(
      sample.width.ceil().max(px(1.0)),
      self.cell_height(sample.ascent + sample.descent, window.scale_factor()),
    )
  }

  /// Whether the cursor blinks, preferring the override over the program's
  /// request.
  pub(crate) fn cursor_blinks(&self, frame_requests_blink: bool) -> bool {
    self.cursor_blink.unwrap_or(frame_requests_blink)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn cell_height_snaps_to_physical_pixels_and_never_collapses() {
    let appearance = TerminalAppearance::default();
    assert_eq!(appearance.cell_height(px(17.0), 1.0), px(17.0));
    let taller = appearance.clone().with_cell_height_adjust(0.2);
    assert_eq!(taller.cell_height(px(10.0), 1.0), px(12.0));
    assert_eq!(taller.cell_height(px(17.3), 2.0), px(21.0));
    let offset = appearance.clone().with_cell_height_offset(px(4.));
    assert_eq!(offset.cell_height(px(20.), 1.0), px(24.));
    let crushed = appearance.with_cell_height_adjust(-2.0);
    assert_eq!(crushed.cell_height(px(17.0), 1.0), px(1.0));
  }

  #[test]
  fn an_explicit_cursor_blink_setting_overrides_the_program_request() {
    let appearance = TerminalAppearance::default();
    assert!(appearance.cursor_blinks(true));
    assert!(!appearance.cursor_blinks(false));
    assert!(!appearance.clone().with_cursor_blink(Some(false)).cursor_blinks(true));
    assert!(appearance.with_cursor_blink(Some(true)).cursor_blinks(false));
  }
}
