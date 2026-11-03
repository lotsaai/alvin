//! Immutable frames the engine publishes to the UI.
//!
//! A frame is everything a renderer needs to draw one viewport: the rows as
//! text with cell spans and style runs, the cursor, the colors, the scroll
//! metrics and the input modes. Rows are `Arc`s so a frame that changes one
//! row shares the rest with the previous frame.
//!
//! Derived from Herdr (Apache-2.0).

use std::ops::Range;
use std::sync::Arc;

pub use ghostty_vt::render::RowId;
use ghostty_vt::style::Palette;

/// An 8-bit RGB color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A cell color as the program set it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
  /// The terminal default for that slot.
  #[default]
  Default,
  /// A 256-color palette index.
  Indexed(u8),
  /// A direct color.
  Rgb(Rgb),
}

/// Underline styles, in SGR 4:n order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
  /// No underline.
  #[default]
  None,
  /// A single line.
  Single,
  /// Two lines.
  Double,
  /// A wavy line.
  Curly,
  /// A dotted line.
  Dotted,
  /// A dashed line.
  Dashed,
}

/// The visual attributes of a run of cells.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Style {
  /// Foreground color.
  pub foreground: Color,
  /// Background color.
  pub background: Color,
  /// Underline color; `Default` means the foreground.
  pub underline_color: Color,
  /// Bold.
  pub bold: bool,
  /// Italic.
  pub italic: bool,
  /// Faint, drawn at half opacity.
  pub faint: bool,
  /// Blink.
  pub blink: bool,
  /// Inverse video: foreground and background swapped.
  pub inverse: bool,
  /// Invisible: the text is drawn in the background color.
  pub invisible: bool,
  /// Underline style.
  pub underline: Underline,
  /// Strikethrough.
  pub strikethrough: bool,
  /// Overline.
  pub overline: bool,
}

/// One grid cell of a row: its bytes in the row text and its width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct CellSpan {
  /// The cell's bytes in [`TerminalRow::text`]. Empty for a spacer.
  pub text: Range<usize>,
  /// The column of the cell.
  pub column: u16,
  /// Width in columns: 1, or 2 for a wide character. 0 for a spacer.
  pub width: u8,
  /// Whether this is the tail of a wide character or a wrap spacer, which
  /// is never drawn.
  pub spacer: bool,
  /// The OSC 8 hyperlink on the cell.
  pub hyperlink: Option<Arc<str>>,
}

/// A run of consecutive cells with one style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StyleRun {
  /// The run's bytes in [`TerminalRow::text`].
  pub text: Range<usize>,
  /// The run's columns.
  pub columns: Range<u16>,
  /// The style of every cell in the run.
  pub style: Style,
}

/// One row of the viewport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TerminalRow {
  /// The engine's identity for the row. It stays with the row as it
  /// scrolls, so a renderer can key a cache on it.
  pub id: RowId,
  /// The frame revision the row was last rebuilt in.
  pub revision: u64,
  /// The row's text, one grapheme cluster per non-spacer cell, with a
  /// space for an empty cell.
  pub text: Arc<str>,
  /// One span per column.
  pub cells: Arc<[CellSpan]>,
  /// The style runs covering the row.
  pub runs: Arc<[StyleRun]>,
  /// Whether the row continues on the next row.
  pub soft_wrapped: bool,
}

/// Cursor shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
  /// A filled block.
  #[default]
  Block,
  /// A vertical bar.
  Bar,
  /// An underline.
  Underline,
  /// An outlined block, drawn when the terminal is not focused.
  Hollow,
}

/// The cursor as the program left it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Cursor {
  /// Column in the viewport.
  pub column: u16,
  /// Row in the viewport.
  pub row: u16,
  /// Whether the cursor is visible and inside the viewport.
  pub visible: bool,
  /// Whether the program asked for a blinking cursor.
  pub blinking: bool,
  /// The shape the program asked for.
  pub shape: CursorShape,
}

/// The colors a frame is drawn with.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TerminalColors {
  /// Default foreground.
  pub(crate) foreground: Rgb,
  /// Default background.
  pub(crate) background: Rgb,
  /// Cursor color; `None` means the foreground.
  pub(crate) cursor: Option<Rgb>,
  /// Selection background; `None` means the default foreground.
  pub(crate) selection_background: Option<Rgb>,
  /// Selection foreground; `None` means the default background.
  pub(crate) selection_foreground: Option<Rgb>,
  /// The 256-color palette.
  pub(crate) palette: [Rgb; 256],
}

impl Default for TerminalColors {
  /// Ghostty's default palette with a light foreground on a dark
  /// background.
  fn default() -> Self {
    Self {
      foreground: Rgb(0xff, 0xff, 0xff),
      background: Rgb(0x28, 0x2c, 0x34),
      cursor: None,
      selection_background: None,
      selection_foreground: None,
      palette: Palette::default().0.map(|c| Rgb(c.r, c.g, c.b)),
    }
  }
}

#[cfg(test)]
impl TerminalColors {
  /// The default palette with the given foreground and background.
  #[must_use]
  pub fn new(foreground: Rgb, background: Rgb) -> Self {
    Self {
      foreground,
      background,
      ..Self::default()
    }
  }

  /// Set the selection colors.
  #[must_use]
  pub fn with_selection(mut self, background: Rgb, foreground: Rgb) -> Self {
    self.selection_background = Some(background);
    self.selection_foreground = Some(foreground);
    self
  }

  /// The default foreground.
  pub fn foreground(&self) -> Rgb {
    self.foreground
  }

  /// The default background.
  pub fn background(&self) -> Rgb {
    self.background
  }

  /// The cursor color; `None` means the foreground.
  pub fn cursor(&self) -> Option<Rgb> {
    self.cursor
  }

  /// The selection background; `None` means the default foreground, as
  /// in Ghostty.
  pub fn selection_background(&self) -> Option<Rgb> {
    self.selection_background
  }

  /// The selection foreground; `None` means the default background, as
  /// in Ghostty.
  pub fn selection_foreground(&self) -> Option<Rgb> {
    self.selection_foreground
  }

  /// The 256-color palette.
  pub fn palette(&self) -> &[Rgb; 256] {
    &self.palette
  }
}

/// A linear or rectangular selection in viewport cells.
///
/// `start` and `end` are `(column, row)`, both inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CellSelection {
  /// The first cell.
  pub start: (u16, u16),
  /// The last cell.
  pub end: (u16, u16),
  /// Whether the selection is a rectangle instead of a run of lines.
  pub rectangle: bool,
}

impl CellSelection {
  /// A selection from `start` to `end`.
  #[must_use]
  pub fn new(start: (u16, u16), end: (u16, u16), rectangle: bool) -> Self {
    Self { start, end, rectangle }
  }
}

/// Where the viewport sits in the scrollable area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScrollMetrics {
  /// Rows in scrollback plus the active area.
  pub total_rows: usize,
  /// The viewport's first row within `total_rows`.
  pub offset: usize,
  /// Rows in the viewport.
  pub viewport_rows: usize,
}

impl ScrollMetrics {
  /// Whether the viewport shows the bottom of the scrollback.
  #[must_use]
  pub fn at_bottom(self) -> bool {
    self.offset.saturating_add(self.viewport_rows) >= self.total_rows
  }
}

/// Terminal modes the UI has to know about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputModes {
  /// A program is receiving mouse reports.
  pub mouse_reporting: bool,
  /// The alternate screen is active.
  pub alternate_screen: bool,
  /// Bracketed paste is on.
  pub bracketed_paste: bool,
  /// Focus reporting (mode 1004) is on.
  pub focus_reporting: bool,
  /// Alternate scroll (mode 1007): wheel events become arrow keys on the
  /// alternate screen.
  pub alternate_scroll: bool,
}

/// The grid size and the cell size in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Viewport {
  columns: u16,
  rows: u16,
  cell_width: u32,
  cell_height: u32,
}

impl Default for Viewport {
  fn default() -> Self {
    Self {
      columns: 80,
      rows: 24,
      cell_width: 8,
      cell_height: 16,
    }
  }
}

/// The largest grid the engine accepts on either axis.
pub const MAX_GRID: u16 = 1000;

impl Viewport {
  /// A viewport, rejecting empty grids, grids over [`MAX_GRID`] and cells
  /// over 4096 pixels.
  pub fn new(columns: u16, rows: u16, cell_width: u32, cell_height: u32) -> Option<Self> {
    if columns == 0
      || rows == 0
      || columns > MAX_GRID
      || rows > MAX_GRID
      || cell_width == 0
      || cell_height == 0
      || cell_width > 4096
      || cell_height > 4096
    {
      return None;
    }
    Some(Self {
      columns,
      rows,
      cell_width,
      cell_height,
    })
  }
  /// Columns.
  #[must_use]
  pub fn columns(self) -> u16 {
    self.columns
  }
  /// Rows.
  #[must_use]
  pub fn rows(self) -> u16 {
    self.rows
  }
  /// Cell width in device pixels.
  #[must_use]
  pub fn cell_width(self) -> u32 {
    self.cell_width
  }
  /// Cell height in device pixels.
  #[must_use]
  pub fn cell_height(self) -> u32 {
    self.cell_height
  }
}

/// One rendered viewport.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TerminalFrame {
  /// Increments on every published frame.
  pub revision: u64,
  /// Increments when the rows, the viewport or the scroll position change,
  /// so a selection anchored to content knows when it moved.
  pub content_revision: u64,
  /// The grid this frame was built for.
  pub viewport: Viewport,
  /// The viewport rows, top to bottom.
  pub rows: Arc<[Arc<TerminalRow>]>,
  /// The cursor.
  pub cursor: Cursor,
  /// The colors.
  pub colors: TerminalColors,
  /// The scroll position.
  pub scroll: ScrollMetrics,
  /// The input modes.
  pub modes: InputModes,
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_default_colors_are_ghosttys_built_in_ones() {
    // Ghostty's src/config/Config.zig at the pinned commit: `background`
    // #282c34, `foreground` #ffffff, and no cursor or selection color.
    let colors = TerminalColors::default();
    assert_eq!(colors.background(), Rgb(0x28, 0x2c, 0x34));
    assert_eq!(colors.foreground(), Rgb(0xff, 0xff, 0xff));
    assert_eq!(colors.cursor(), None);
    assert_eq!(colors.selection_background(), None);
    assert_eq!(colors.selection_foreground(), None);
    assert_eq!(colors.palette()[1], Rgb(0xcc, 0x66, 0x66));
  }

  #[test]
  fn viewport_rejects_bad_sizes() {
    assert!(Viewport::new(0, 1, 8, 16).is_none());
    assert!(Viewport::new(1, 0, 8, 16).is_none());
    assert!(Viewport::new(1001, 1, 8, 16).is_none());
    assert!(Viewport::new(1, 1, 0, 16).is_none());
    assert!(Viewport::new(80, 24, 8, 16).is_some());
  }

  #[test]
  fn scroll_metrics_know_the_bottom() {
    assert!(
      ScrollMetrics {
        total_rows: 30,
        offset: 6,
        viewport_rows: 24
      }
      .at_bottom()
    );
    assert!(
      !ScrollMetrics {
        total_rows: 30,
        offset: 0,
        viewport_rows: 24
      }
      .at_bottom()
    );
  }
}
