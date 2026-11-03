//! ANSI SGR formatting for extension status text.

use std::ops::Range;

use anstyle_parse::{Params, Parser, Perform};
use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle, px, rgb};

#[derive(Default)]
struct Text {
  text: String,
  highlights: Vec<(Range<usize>, HighlightStyle)>,
  style: HighlightStyle,
  start: usize,
}

impl Text {
  fn finish_run(&mut self) {
    let end = self.text.len();
    if self.start < end && self.style != HighlightStyle::default() {
      self.highlights.push((self.start..end, self.style));
    }
    self.start = end;
  }
}

impl Perform for Text {
  fn print(&mut self, c: char) {
    self.text.push(c);
  }

  fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: u8) {
    if action != b'm' || ignore || !intermediates.is_empty() {
      return;
    }
    self.finish_run();
    let mut values = params.iter().flat_map(|value| value.iter().copied()).peekable();
    if values.peek().is_none() {
      self.style = HighlightStyle::default();
      return;
    }
    while let Some(value) = values.next() {
      match value {
        0 => self.style = HighlightStyle::default(),
        1 => self.style.font_weight = Some(FontWeight::BOLD),
        2 => self.style.fade_out = Some(0.5),
        3 => self.style.font_style = Some(FontStyle::Italic),
        4 => {
          self.style.underline = Some(UnderlineStyle {
            thickness: px(1.),
            ..Default::default()
          })
        }
        9 => {
          self.style.strikethrough = Some(StrikethroughStyle {
            thickness: px(1.),
            ..Default::default()
          })
        }
        22 => {
          self.style.font_weight = None;
          self.style.fade_out = None;
        }
        23 => self.style.font_style = None,
        24 => self.style.underline = None,
        29 => self.style.strikethrough = None,
        30..=37 => self.style.color = Some(palette(value - 30)),
        40..=47 => self.style.background_color = Some(palette(value - 40)),
        90..=97 => self.style.color = Some(palette(value - 90 + 8)),
        100..=107 => self.style.background_color = Some(palette(value - 100 + 8)),
        39 => self.style.color = None,
        49 => self.style.background_color = None,
        38 | 48 => {
          let color = match values.next() {
            Some(5) => values.next().filter(|n| *n < 256).map(palette),
            Some(2) => match (values.next(), values.next(), values.next()) {
              (Some(r), Some(g), Some(b)) if r < 256 && g < 256 && b < 256 => {
                Some(rgb((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)).into())
              }
              _ => None,
            },
            _ => None,
          };
          if let Some(color) = color {
            if value == 38 {
              self.style.color = Some(color);
            } else {
              self.style.background_color = Some(color);
            }
          }
        }
        _ => {}
      }
    }
  }
}

fn palette(index: u16) -> Hsla {
  const COLORS: [u32; 16] = [
    0x000000, 0x800000, 0x008000, 0x808000, 0x000080, 0x800080, 0x008080, 0xc0c0c0, 0x808080, 0xff0000, 0x00ff00, 0xffff00, 0x0000ff,
    0xff00ff, 0x00ffff, 0xffffff,
  ];
  let value = if let Some(color) = COLORS.get(usize::from(index)) {
    *color
  } else if index < 232 {
    let n = index.saturating_sub(16);
    let component = |v: u16| if v == 0 { 0 } else { 55 + 40 * u32::from(v) };
    (component(n / 36) << 16) | (component((n / 6) % 6) << 8) | component(n % 6)
  } else {
    let c = 8 + 10 * u32::from(index.saturating_sub(232));
    (c << 16) | (c << 8) | c
  };
  rgb(value).into()
}

pub(crate) fn parse(input: &str) -> (String, Vec<(Range<usize>, HighlightStyle)>) {
  let mut text = Text::default();
  let mut parser = Parser::<anstyle_parse::Utf8Parser>::new();
  for byte in input.bytes() {
    parser.advance(&mut text, byte);
  }
  text.finish_run();
  (text.text, text.highlights)
}

#[cfg(test)]
mod tests {
  use super::parse;

  #[test]
  fn sgr_formats_unicode_and_resets() {
    let (text, highlights) = parse("plain \x1b[1;38;2;255;0;0m赤\x1b[0m end");
    assert_eq!(text, "plain 赤 end");
    assert_eq!(highlights.len(), 1);
    assert_eq!(&text[highlights[0].0.clone()], "赤");
    assert!(highlights[0].1.color.is_some());
    assert!(highlights[0].1.font_weight.is_some());
  }
}
