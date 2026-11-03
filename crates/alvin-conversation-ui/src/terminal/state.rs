//! The terminal entity: frames in, input out.
//!
//! Derived from Herdr (Apache-2.0).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{
  App, ClipboardItem, Context, ElementId, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke,
  KeystrokeEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, ScrollEvent, SharedString,
  Subscription, Task, Window,
};

use crate::terminal::appearance::TerminalAppearance;
use crate::terminal::cursor::Blink;
use crate::terminal::engine::{ForegroundProcess, TerminalSnapshot, TerminalStatus};
use crate::terminal::frame::{TerminalColors, TerminalFrame, Viewport};
use crate::terminal::geometry::Geometry;
use crate::terminal::input::{KeyAction, MouseAction, ScrollRequest, TerminalInput};
use crate::terminal::keys::{self, ScrollAccumulator};
use crate::terminal::links::{self, Link};
use crate::terminal::selection::{self, Cell, Selection};
use crate::terminal::source::{FrameHandle, FrameSink, FrameSource, StartOptions};
use gpui_kit::component::{ActiveTheme as _, theme::Theme};

/// Events a host subscribes to.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TerminalEvent {
  /// The terminal took keyboard focus.
  Focused,
  /// The program changed the title; read it with `title`.
  TitleChanged,
  /// The program reported a new working directory; read it with `cwd`.
  CwdChanged,
  /// The program rang the bell.
  Bell,
  /// The program wrote to the clipboard through OSC 52.
  ClipboardWritten,
  /// The terminal could not start or the engine failed.
  Failed(Arc<std::io::Error>),
  /// Input could not be queued and was not delivered.
  InputRejected(crate::terminal::engine::InputRejected),
  /// The font size changed through the zoom actions or
  /// [`TerminalState::set_font_size`]; read it with `font_size`.
  FontSizeChanged,
  /// Another process took the foreground, such as an editor the shell
  /// started, or the shell back at its prompt; read it with
  /// `foreground`.
  ForegroundChanged,
}

/// Colors and appearance a terminal starts with.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct TerminalConfig {
  colors: TerminalColors,
  appearance: TerminalAppearance,
}

impl From<&crate::TerminalSettings> for TerminalConfig {
  fn from(settings: &crate::TerminalSettings) -> Self {
    use crate::terminal::frame::{CursorShape, Rgb};
    let mut config = Self::default();
    config.appearance.font_family = settings.font_family.clone().map(Into::into);
    config.appearance.font_fallbacks.clone_from(&settings.font_fallbacks);
    config.appearance.font_size = settings.font_size.map(gpui_kit::px);
    config.appearance.cursor_shape = settings.cursor_shape.map(|shape| match shape {
      crate::TerminalCursor::Block => CursorShape::Block,
      crate::TerminalCursor::Bar => CursorShape::Bar,
      crate::TerminalCursor::Underline => CursorShape::Underline,
      crate::TerminalCursor::Hollow => CursorShape::Hollow,
    });
    config.appearance.cursor_blink = settings.cursor_blink;
    let rgb = |[r, g, b]: [u8; 3]| Rgb(r, g, b);
    if let Some(color) = settings.foreground {
      config.colors.foreground = rgb(color);
    }
    if let Some(color) = settings.background {
      config.colors.background = rgb(color);
    }
    config.colors.cursor = settings.cursor_color.map(rgb);
    config.colors.selection_foreground = settings.selection_foreground.map(rgb);
    config.colors.selection_background = settings.selection_background.map(rgb);
    for &(index, color) in &settings.palette {
      if let Some(entry) = config.colors.palette.get_mut(usize::from(index)) {
        *entry = rgb(color);
      }
    }
    config
  }
}

/// A terminal: the latest frame, the program's status and everything the
/// element needs to draw and to send input.
#[non_exhaustive]
pub struct TerminalState {
  handle: Option<Box<dyn FrameHandle>>,
  closed: bool,
  frame: Arc<TerminalFrame>,
  status: TerminalStatus,
  title: SharedString,
  cwd: Option<PathBuf>,
  foreground: Option<ForegroundProcess>,
  bell_count: u64,
  clipboard_sequence: u64,
  colors: TerminalColors,
  appearance: TerminalAppearance,
  /// The size the font was zoomed to, over the appearance's.
  font_size: Option<Pixels>,
  focus: FocusHandle,
  focused: bool,
  visible: bool,
  pub(crate) geometry: Option<Geometry>,
  geometry_scale: Option<f32>,
  requested: Viewport,
  pub(crate) selection: Option<Selection>,
  pub(crate) marked_text: Option<String>,
  /// Rows prepared in the last frame, for the element to reuse.
  pub(crate) row_cache: crate::terminal::element::RowCache,
  pressed_keys: HashSet<String>,
  right_click_passthrough: bool,
  hovered_link: Option<(u16, Link)>,
  scroll: ScrollAccumulator,
  pub(crate) cursor: Blink,
  frames: Option<Task<()>>,
  _subscriptions: [Subscription; 3],
}

impl TerminalState {
  /// A terminal fed by `source`, started at once.
  pub fn new(source: impl FrameSource, config: TerminalConfig, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let focus = cx.focus_handle();
    let this = cx.weak_entity();
    let mut state = Self {
      handle: None,
      closed: false,
      frame: Arc::new(TerminalFrame::default()),
      status: TerminalStatus::Starting,
      title: SharedString::default(),
      cwd: None,
      foreground: None,
      bell_count: 0,
      clipboard_sequence: 0,
      colors: config.colors,
      appearance: config.appearance,
      font_size: None,
      focus: focus.clone(),
      focused: false,
      visible: true,
      geometry: None,
      geometry_scale: None,
      requested: Viewport::default(),
      selection: None,
      marked_text: None,
      row_cache: crate::terminal::element::RowCache::default(),
      pressed_keys: HashSet::new(),
      right_click_passthrough: true,
      hovered_link: None,
      scroll: ScrollAccumulator::default(),
      cursor: Blink::new(),
      frames: None,
      _subscriptions: [
        cx.on_focus_in(&focus, window, |state: &mut Self, _, cx| {
          state.focus_changed(true, cx);
        }),
        cx.on_focus_out(&focus, window, |state: &mut Self, _, _, cx| {
          state.focus_changed(false, cx);
        }),
        cx.intercept_keystrokes({
          let state = this;
          let focus = focus.clone();
          move |event, window, cx| {
            if !focus.is_focused(window) {
              return;
            }
            if let Some(state) = state.upgrade() {
              state.update(cx, |state, cx| state.claim_key(event, window, cx));
            }
          }
        }),
      ],
    };
    state.start(Box::new(source), cx);
    state
  }

  fn start(&mut self, source: Box<dyn FrameSource>, cx: &mut Context<Self>) {
    let (sink, wake) = FrameSink::new();
    let options = StartOptions {
      viewport: self.requested,
      colors: self.colors.clone(),
      visible: self.visible,
      cursor: self
        .appearance
        .cursor_shape
        .map(|shape| (shape, self.appearance.cursor_blink.unwrap_or(true))),
    };
    match source.start(sink.clone(), options) {
      Ok(handle) => {
        self.handle = Some(handle);
        self.status = TerminalStatus::Starting;
      }
      Err(error) => {
        let error = Arc::new(error);
        self.status = TerminalStatus::Failed(error.clone());
        cx.emit(TerminalEvent::Failed(error));
        return;
      }
    }
    self.frames = Some(cx.spawn(async move |state, cx| {
      while wake.recv().await.is_ok() {
        let Some(snapshot) = sink.take() else {
          continue;
        };
        if state.update(cx, |state, cx| state.apply(&snapshot, cx)).is_err() {
          break;
        }
      }
    }));
  }

  /// Install a snapshot. Public so a host driving its own frames can use
  /// the same path as the engine.
  pub fn apply(&mut self, snapshot: &TerminalSnapshot, cx: &mut Context<Self>) {
    let reflowed = snapshot.frame.content_revision != self.frame.content_revision;
    self.selection = self.selection.and_then(|selection| selection.refresh(&self.frame, &snapshot.frame));
    self.frame = Arc::clone(&snapshot.frame);
    if snapshot.title != self.title.as_ref() {
      self.title = snapshot.title.clone().into();
      cx.emit(TerminalEvent::TitleChanged);
    }
    if snapshot.cwd != self.cwd {
      self.cwd.clone_from(&snapshot.cwd);
      cx.emit(TerminalEvent::CwdChanged);
    }
    if snapshot.foreground != self.foreground {
      let cwd = self.cwd().map(Path::to_path_buf);
      self.foreground.clone_from(&snapshot.foreground);
      cx.emit(TerminalEvent::ForegroundChanged);
      if self.cwd().map(Path::to_path_buf) != cwd {
        cx.emit(TerminalEvent::CwdChanged);
      }
    }
    if snapshot.bell_count != self.bell_count {
      self.bell_count = snapshot.bell_count;
      cx.emit(TerminalEvent::Bell);
    }
    if snapshot.clipboard_sequence != self.clipboard_sequence {
      self.clipboard_sequence = snapshot.clipboard_sequence;
      if let Some(text) = &snapshot.clipboard {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        cx.emit(TerminalEvent::ClipboardWritten);
      }
    }
    if snapshot.status != self.status {
      self.status = snapshot.status.clone();
      match &self.status {
        TerminalStatus::Starting | TerminalStatus::Live | TerminalStatus::Exited(_) => {}
        TerminalStatus::Failed(error) => cx.emit(TerminalEvent::Failed(error.clone())),
      }
      if !matches!(self.status, TerminalStatus::Live) {
        self.cursor.stop();
      }
    }
    if reflowed {
      self.hovered_link = None;
    }
    cx.notify();
  }

  /// The frame being drawn.
  #[must_use]
  pub fn frame(&self) -> &Arc<TerminalFrame> {
    &self.frame
  }

  /// The working directory: the one the shell reported through its
  /// integration (OSC 7), or else the foreground process's.
  #[must_use]
  pub fn cwd(&self) -> Option<&Path> {
    self.cwd.as_deref().or_else(|| self.foreground.as_ref()?.cwd())
  }

  /// The appearance.
  #[must_use]
  pub fn appearance(&self) -> &TerminalAppearance {
    &self.appearance
  }

  pub(crate) fn set_terminal_settings(&mut self, settings: &crate::TerminalSettings, cx: &mut Context<Self>) {
    let config = TerminalConfig::from(settings);
    self.colors = config.colors;
    self.appearance = config.appearance;
    if let Some(handle) = &self.handle {
      let shape = self.appearance.cursor_shape.unwrap_or(crate::terminal::frame::CursorShape::Block);
      handle.set_appearance(self.colors.clone(), Some((shape, self.appearance.cursor_blink.unwrap_or(true))));
    }
    cx.notify();
  }

  /// The font size the grid draws with: the zoomed size, else the
  /// appearance's, else the theme's code font size.
  pub fn font_size(&self, cx: &App) -> Pixels {
    self.font_size.unwrap_or_else(|| self.appearance.resolved(cx.theme()).size())
  }

  /// Draw the grid at `size`, clamped to 1-255 points as Ghostty clamps
  /// it. The grid is measured again at the next paint, which resizes
  /// the program's grid to fit. Emits [`TerminalEvent::FontSizeChanged`].
  pub fn set_font_size(&mut self, size: impl Into<Pixels>, cx: &mut Context<Self>) {
    let size = size.into().clamp(gpui_kit::px(1.), gpui_kit::px(255.));
    if self.font_size == Some(size) {
      return;
    }
    self.font_size = Some(size);
    cx.emit(TerminalEvent::FontSizeChanged);
    cx.notify();
  }

  /// Zoom the font in by `points`, as Ghostty's `increase_font_size`.
  pub fn increase_font_size(&mut self, points: impl Into<Pixels>, cx: &mut Context<Self>) {
    let size = self.font_size(cx) + points.into();
    self.set_font_size(size, cx);
  }

  /// Zoom the font out by `points`, as Ghostty's `decrease_font_size`.
  pub fn decrease_font_size(&mut self, points: impl Into<Pixels>, cx: &mut Context<Self>) {
    let size = self.font_size(cx) - points.into();
    self.set_font_size(size, cx);
  }

  /// Go back to the appearance's font size, or the theme's.
  pub fn reset_font_size(&mut self, cx: &mut Context<Self>) {
    if self.font_size.take().is_some() {
      cx.emit(TerminalEvent::FontSizeChanged);
      cx.notify();
    }
  }

  /// The appearance with the theme's values filled in and the zoom on top,
  /// as the grid draws with it.
  pub(crate) fn drawn_appearance(&self, theme: &Theme) -> TerminalAppearance {
    let mut appearance = self.appearance.resolved(theme);
    if let Some(size) = self.font_size {
      appearance.font_size = Some(size);
    }
    appearance
  }

  /// Whether a program is running.
  #[must_use]
  pub fn is_live(&self) -> bool {
    matches!(self.status, TerminalStatus::Live) && !self.closed
  }

  /// Whether the terminal is visible.
  #[must_use]
  pub fn is_visible(&self) -> bool {
    self.visible
  }

  /// Hidden terminals keep parsing output but stop producing frames and
  /// stop the cursor cadence. Revealing one asks for a current frame.
  pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
    if self.visible == visible {
      return;
    }
    self.visible = visible;
    if let Some(handle) = &self.handle {
      handle.set_visible(visible);
    }
    if !visible {
      self.cursor.stop();
    }
    cx.notify();
  }

  /// Stop the program and the source. Idempotent.
  pub fn close(&mut self, cx: &mut Context<Self>) {
    if self.closed {
      return;
    }
    self.closed = true;
    if let Some(handle) = &self.handle {
      handle.close();
    }
    self.selection = None;
    self.cursor.stop();
    cx.notify();
  }

  /// Take the stopped engine thread to join on a background executor.
  pub(crate) fn take_thread(&self) -> Option<std::thread::JoinHandle<()>> {
    self.handle.as_ref().and_then(|handle| handle.take_thread())
  }

  fn leaves_right_click(&self, button: MouseButton, modifiers: Modifiers, _: &Window) -> bool {
    button == MouseButton::Right && (!self.right_click_passthrough || modifiers != Modifiers::default() || !self.reports_mouse(modifiers))
  }

  /// Whether a non-empty selection exists.
  #[must_use]
  pub fn has_selection(&self) -> bool {
    self.selection.is_some_and(|s| !s.is_empty())
  }

  /// The link under the pointer while the platform modifier is held.
  #[must_use]
  pub fn hovered_link(&self) -> Option<&str> {
    self.hovered_link.as_ref().map(|(_, link)| link.url.as_str())
  }

  pub(crate) fn hovered_link_span(&self) -> Option<(u16, std::ops::Range<u16>)> {
    self.hovered_link.as_ref().map(|(row, link)| (*row, link.columns.clone()))
  }

  /// Copy the selection to the clipboard. A live engine is asked for the
  /// text because it can reach scrollback the frame does not carry.
  pub fn copy(&mut self, cx: &mut Context<Self>) {
    let Some(selection) = self.selection.filter(|s| !s.is_empty()) else {
      return;
    };
    let trim = self.appearance.trim_trailing_spaces;
    if let Some(reply) = self.handle.as_ref().and_then(|handle| handle.copy(selection.to_cells())) {
      cx.spawn(async move |state, cx| {
        let result = match reply.recv().await {
          Ok(result) => result,
          Err(error) => Err(std::io::Error::other(error)),
        };
        if state
          .update(cx, |_, cx| match result {
            Ok(text) => cx.write_to_clipboard(ClipboardItem::new_string(finish(&text, trim))),
            Err(error) => cx.emit(TerminalEvent::Failed(Arc::new(error))),
          })
          .is_err()
        {}
      })
      .detach();
      return;
    }
    let text = selection.text(&self.frame);
    if !text.is_empty() {
      cx.write_to_clipboard(ClipboardItem::new_string(finish(&text, trim)));
    }
  }

  /// Select the whole viewport.
  pub fn select_all(&mut self, cx: &mut Context<Self>) {
    let rows = self.frame.rows.len().min(usize::from(u16::MAX));
    let columns = self.frame.viewport.columns();
    if rows == 0 || columns == 0 {
      return;
    }
    let last_row = u16::try_from(rows.saturating_sub(1)).unwrap_or(u16::MAX);
    let mut selection = Selection::new(Cell::new(0, 0), self.frame.content_revision, false);
    selection.extend(Cell::new(columns, last_row));
    selection.finish();
    self.selection = Some(selection);
    cx.notify();
  }

  /// Drop the selection. Returns whether there was one.
  pub fn clear_selection(&mut self, cx: &mut Context<Self>) -> bool {
    if self.selection.take().is_some() {
      cx.notify();
      return true;
    }
    false
  }

  /// Send form feed, which clears the screen at most shells.
  pub fn clear(&mut self, cx: &mut Context<Self>) {
    self.selection = None;
    self.send(TerminalInput::Text("\u{c}".to_owned()), cx);
  }

  /// Paste the clipboard text.
  pub fn paste(&mut self, cx: &mut Context<Self>) {
    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
      self.paste_text(text, cx);
    }
  }

  /// Paste text, bracketed when the program asked for it.
  pub fn paste_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
    self.selection = None;
    self.send(TerminalInput::Paste(text.into()), cx);
  }

  /// Scroll the viewport.
  pub fn scroll(&mut self, request: ScrollRequest, cx: &mut Context<Self>) {
    self.send(TerminalInput::Scroll(request), cx);
    cx.notify();
  }

  /// Scroll by whole pages; negative is toward older output.
  pub fn scroll_pages(&mut self, pages: isize, cx: &mut Context<Self>) {
    let rows = isize::try_from(self.frame.viewport.rows()).unwrap_or(24);
    self.scroll(ScrollRequest::Lines(pages.saturating_mul(rows)), cx);
  }

  /// Whether the viewport is scrolled into history.
  #[must_use]
  pub fn is_scrolled(&self) -> bool {
    !self.frame.scroll.at_bottom()
  }

  /// The focus handle.
  #[must_use]
  pub fn focus_handle(&self) -> &FocusHandle {
    &self.focus
  }

  pub(crate) fn send(&mut self, input: TerminalInput, cx: &mut Context<Self>) {
    let Some(handle) = &self.handle else {
      cx.emit(TerminalEvent::InputRejected(crate::terminal::engine::InputRejected::Closed));
      return;
    };
    let scroll = !matches!(input, TerminalInput::Scroll(_) | TerminalInput::Focus(_)) && self.is_scrolled();
    if let Err(error) = handle.input(input) {
      cx.emit(TerminalEvent::InputRejected(error));
      cx.notify();
      return;
    }
    if scroll && let Err(error) = handle.input(TerminalInput::Scroll(ScrollRequest::Bottom)) {
      cx.emit(TerminalEvent::InputRejected(error));
    }
    cx.notify();
  }

  /// Grant credit for the next frame after a real paint. Never notifies,
  /// or it would drive a repaint loop.
  pub(crate) fn frame_painted(&self, painted: bool) {
    if painted
      && self.visible
      && !self.closed
      && matches!(self.status, TerminalStatus::Live)
      && let Some(handle) = &self.handle
    {
      handle.request_frame();
    }
  }

  pub(crate) fn geometry_scale_value(&self) -> f32 {
    self.geometry_scale.unwrap_or(1.0)
  }

  /// Record geometry measured during paint and resize the source only
  /// when the grid or the physical cell size changed.
  pub(crate) fn set_geometry(&mut self, geometry: Geometry, scale: f32, cx: &mut Context<Self>) {
    if self.geometry == Some(geometry) && self.geometry_scale == Some(scale) {
      return;
    }
    let first_measurement = self.geometry.is_none();
    self.geometry = Some(geometry);
    self.geometry_scale = Some(scale);
    let viewport = Viewport::new(
      geometry.columns,
      geometry.rows,
      (f32::from(geometry.cell.width) * scale).round().max(1.0) as u32,
      (f32::from(geometry.cell.height) * scale).round().max(1.0) as u32,
    );
    let Some(viewport) = viewport else {
      cx.emit(TerminalEvent::Failed(Arc::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "terminal geometry is invalid",
      ))));
      return;
    };
    if !first_measurement && self.requested == viewport {
      return;
    }
    self.requested = viewport;
    if let Some(handle) = &self.handle {
      handle.resize(viewport);
    }
    cx.notify();
  }

  fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
    if self.focused == focused {
      return;
    }
    self.focused = focused;
    self.send(TerminalInput::Focus(focused), cx);
    if focused {
      cx.emit(TerminalEvent::Focused);
    } else {
      self.pressed_keys.clear();
      self.cursor.stop();
    }
    cx.notify();
  }

  /// Keys the terminal consumes stop propagation, which is what prevents
  /// printable text arriving twice: once as a key and again as an IME
  /// commit.
  pub(crate) fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
    if self.marked_text.is_some() {
      return;
    }
    let keystroke = &event.keystroke;
    if keystroke.key == "escape" && keystroke.modifiers == Default::default() && self.clear_selection(cx) {
      cx.stop_propagation();
      return;
    }
    if let Some(key) = keys::key(keystroke, keys::key_action(event.is_held)) {
      self.selection = None;
      self.pressed_keys.insert(keystroke.key.clone());
      self.send(TerminalInput::Key(key), cx);
      cx.stop_propagation();
    }
  }

  pub(crate) fn send_keystroke(&mut self, keystroke: &str, window: &mut Window, cx: &mut Context<Self>) {
    let Ok(keystroke) = Keystroke::parse_with_os(keystroke, cx.platform_info().os) else {
      return;
    };
    let event = KeyDownEvent {
      keystroke,
      is_held: false,
      prefer_character_input: false,
    };
    self.on_key_down(&event, window, cx);
  }

  /// Takes a key from a binding outside the terminal that would keep it
  /// from the program, before the binding runs. The terminal is focused.
  fn claim_key(&mut self, event: &KeystrokeEvent, window: &mut Window, cx: &mut Context<Self>) {
    let winner = cx
      .key_bindings()
      .borrow()
      .bindings_for_input(std::slice::from_ref(&event.keystroke), &event.context_stack)
      .0
      .into_iter()
      .next();
    let Some(binding) = winner else {
      return;
    };
    match keys::claim(binding.action().name(), &event.keystroke) {
      keys::Claim::None => {}
      keys::Claim::Send => {
        let key = KeyDownEvent {
          keystroke: event.keystroke.clone(),
          is_held: false,
          prefer_character_input: false,
        };
        self.on_key_down(&key, window, cx);
        cx.stop_propagation();
      }
      keys::Claim::Copy => {
        if self.has_selection() {
          self.copy(cx);
          cx.stop_propagation();
        }
      }
    }
  }

  pub(crate) fn on_key_up(&mut self, event: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
    if !self.pressed_keys.remove(&event.keystroke.key) || self.marked_text.is_some() {
      return;
    }
    if let Some(key) = keys::key(&event.keystroke, KeyAction::Release) {
      self.send(TerminalInput::Key(key), cx);
    }
  }

  pub(crate) fn on_scroll(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
    // The terminal scrolls its own content, so the wheel step is its
    // own even at the end of the scrollback: the page around it must
    // not scroll as well.
    cx.stop_propagation();
    let Some(geometry) = self.geometry else {
      return;
    };
    let delta = event.delta.pixel_delta(geometry.cell.height).y / geometry.cell.height;
    if self.reports_mouse(event.modifiers) {
      self.scroll.reset();
      let action = if delta > 0.0 {
        MouseAction::ScrollUp
      } else {
        MouseAction::ScrollDown
      };
      if delta != 0.0 {
        self.report_mouse(action, None, event.modifiers, event.position, window, cx);
      }
      return;
    }
    if self.frame.modes.alternate_screen && self.frame.modes.alternate_scroll {
      // Mode 1007: the wheel becomes arrow keys on the alternate screen.
      if let Some(ScrollRequest::Lines(lines)) = self.scroll.push(delta) {
        let key = if lines < 0 { "up" } else { "down" };
        for _ in 0..lines.unsigned_abs().min(64) {
          self.send_keystroke(key, window, cx);
        }
      }
      return;
    }
    if let Some(request) = self.scroll.push(delta) {
      self.scroll(request, cx);
    }
  }

  /// Update the hovered link. A link is live only while the platform
  /// modifier is held, so dragging a selection across a URL opens nothing.
  pub(crate) fn hover_link(&mut self, position: Point<Pixels>, active: bool, cx: &mut Context<Self>) {
    let found = active
      .then(|| {
        let geometry = self.geometry?;
        if !geometry.bounds.contains(&position) {
          return None;
        }
        let cell = geometry.cell_at(position);
        let row = self.frame.rows.get(usize::from(cell.row))?;
        links::at(row, cell.column).map(|link| (cell.row, link))
      })
      .flatten();
    if found != self.hovered_link {
      self.hovered_link = found;
      cx.notify();
    }
  }

  fn reports_mouse(&self, modifiers: Modifiers) -> bool {
    keys::reports_mouse(self.frame.modes.mouse_reporting, modifiers)
  }

  fn report_mouse(
    &mut self,
    action: MouseAction,
    button: Option<MouseButton>,
    modifiers: Modifiers,
    position: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) {
    let Some(geometry) = self.geometry else {
      return;
    };
    let scale = window.scale_factor();
    let local = position - geometry.origin;
    let mouse = keys::mouse(
      action,
      button.and_then(keys::mouse_button),
      modifiers,
      f32::from(local.x) * scale,
      f32::from(local.y) * scale,
    );
    self.send(TerminalInput::Mouse(mouse), cx);
  }

  pub(crate) fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
    window.focus(&self.focus, cx);
    if self.leaves_right_click(event.button, event.modifiers, window) {
      return;
    }
    if event.button == MouseButton::Left && event.modifiers.platform {
      self.hover_link(event.position, true, cx);
      if let Some((_, link)) = self.hovered_link.clone() {
        self.selection = None;
        cx.open_url(&link.url);
        cx.stop_propagation();
        return;
      }
    }
    if self.reports_mouse(event.modifiers) {
      if self.selection.take().is_some() {
        cx.notify();
      }
      self.report_mouse(MouseAction::Press, Some(event.button), event.modifiers, event.position, window, cx);
      cx.stop_propagation();
      return;
    }
    if event.button == MouseButton::Left
      && let Some(geometry) = self.geometry
    {
      self.selection = Some(Selection::new(
        geometry.selection_boundary_at(event.position),
        self.frame.content_revision,
        event.modifiers.alt,
      ));
      cx.notify();
    }
  }

  pub(crate) fn on_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
    if self.reports_mouse(event.modifiers) {
      self.report_mouse(MouseAction::Move, event.pressed_button, event.modifiers, event.position, window, cx);
    }
  }

  pub(crate) fn drag_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
    let (Some(geometry), Some(selection)) = (self.geometry, self.selection.as_mut()) else {
      return;
    };
    if !selection.is_dragging() {
      return;
    }
    selection.extend(geometry.selection_boundary_at(position));
    cx.notify();
  }

  pub(crate) fn finish_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
    if !self.selection.is_some_and(Selection::is_dragging) {
      return;
    }
    self.drag_selection(position, cx);
    if let Some(selection) = self.selection.as_mut() {
      selection.finish();
    }
    if self.appearance.copy_on_select && self.has_selection() {
      self.copy(cx);
    }
  }

  pub(crate) fn on_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
    if self.leaves_right_click(event.button, event.modifiers, window) {
      return;
    }
    if self.reports_mouse(event.modifiers) {
      self.report_mouse(
        MouseAction::Release,
        Some(event.button),
        event.modifiers,
        event.position,
        window,
        cx,
      );
    }
  }

  /// The message shown over the grid when the program is not running.
  pub(crate) fn status_message(&self) -> Option<SharedString> {
    match &self.status {
      TerminalStatus::Failed(error) => Some(error.to_string().into()),
      TerminalStatus::Exited(status) => Some(match (status.code, &status.signal) {
        (Some(0), None) => "Process exited".into(),
        (Some(code), None) => format!("Process exited with code {code}").into(),
        (_, Some(signal)) => format!("Process ended by {signal}").into(),
        (None, None) => "Process ended".into(),
      }),
      TerminalStatus::Starting | TerminalStatus::Live => None,
    }
  }
}

fn finish(text: &str, trim: bool) -> String {
  if trim { selection::trim_lines(text) } else { text.to_owned() }
}

impl EventEmitter<TerminalEvent> for TerminalState {}

impl Focusable for TerminalState {
  fn focus_handle(&self, _: &App) -> FocusHandle {
    self.focus.clone()
  }
}

impl Render for TerminalState {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    crate::terminal::element::Terminal::new(
      ElementId::NamedChild(ElementId::View(cx.entity_id()).into(), "terminal".into()),
      &cx.entity(),
    )
  }
}
