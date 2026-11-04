//! Retained tool call presentation and independent disclosure state.

mod bash;
mod edit;
mod generic;
mod read;
mod search;
mod task;
mod write;

#[cfg(test)]
mod tests;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::motion::{MotionReveal, spring};
use gpui_kit::component::attachment::{Attachment, AttachmentContent, AttachmentDescription, AttachmentMedia, AttachmentTitle};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Render,
  SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, rems,
};
use serde_json::Value;

const PREVIEW_LINES: usize = 12;
const PREVIEW_CHARS: usize = 2400;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolInput {
  pub name: String,
  pub intent: Option<String>,
  pub args: Option<Value>,
  pub result: Option<Value>,
  pub stream: Vec<Value>,
  pub running: bool,
  pub failed: bool,
  pub exit_code: Option<i64>,
  pub timestamp: Option<f64>,
}

struct Presentation {
  summary: String,
  content: String,
  file: bool,
  diff: bool,
}

impl Presentation {
  fn text(summary: String, content: String) -> Self {
    Self {
      summary,
      content,
      file: false,
      diff: false,
    }
  }
  fn file(summary: String, content: String) -> Self {
    Self {
      summary,
      content,
      file: true,
      diff: false,
    }
  }
  fn diff(summary: String, content: String) -> Self {
    Self {
      summary,
      content,
      file: false,
      diff: true,
    }
  }
}

fn text<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a str> {
  value?.get(key)?.as_str()
}

fn result_text(result: &Value) -> String {
  result
    .get("content")
    .and_then(Value::as_array)
    .map(|blocks| {
      blocks
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
    })
    .unwrap_or_default()
}

fn preview_end(text: &str) -> usize {
  let mut lines = 0usize;
  for (count, (offset, ch)) in text.char_indices().enumerate() {
    if count >= PREVIEW_CHARS || lines >= PREVIEW_LINES {
      return offset;
    }
    if ch == '\n' {
      lines += 1;
    }
  }
  text.len()
}

fn fenced(text: &str, language: &str) -> String {
  let longest = text.split(|ch| ch != '`').map(str::len).max().unwrap_or_default();
  let fence = "`".repeat(longest.max(2).saturating_add(1));
  format!("{fence}{language}\n{text}\n{fence}")
}

fn redact(value: &mut Value) {
  match value {
    Value::Object(map) => {
      if map.get("type").and_then(Value::as_str) == Some("image") && map.contains_key("data") {
        map.insert("data".into(), Value::String("[image data omitted]".into()));
      }
      for value in map.values_mut() {
        redact(value);
      }
    }
    Value::Array(values) => values.iter_mut().for_each(redact),
    _ => {}
  }
}

fn raw_text(input: &ToolInput) -> String {
  let mut data = serde_json::json!({"arguments": input.args, "result": input.result, "stream": input.stream});
  redact(&mut data);
  format!("{data:#}")
}

pub(super) struct ToolBlock {
  card: Entity<ToolCard>,
}

impl ToolBlock {
  pub(super) fn new(id: SharedString, input: ToolInput, cx: &mut Context<Self>) -> Self {
    let card = cx.new(|cx| ToolCard::new(id, &input, cx));
    cx.observe(&card, |_, _, cx| cx.notify()).detach();
    Self { card }
  }

  pub(super) fn sync(&mut self, input: &ToolInput, cx: &mut Context<Self>) {
    self.card.update(cx, |card, cx| card.sync(input, cx));
  }
}

impl Render for ToolBlock {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let input = &self.card.read(cx).input;
    let theme = cx.theme();
    let status = if input.running {
      Some(("Running", theme.info))
    } else if input.failed {
      Some(("Failed", theme.danger))
    } else if input.result.is_some() {
      Some(("Completed", theme.success))
    } else {
      None
    };
    let wall = input
      .result
      .as_ref()
      .and_then(|result| result.pointer("/details/wallTimeMs"))
      .and_then(Value::as_f64);
    v_flex()
      .w_full()
      .min_w_0()
      .gap_1()
      .child(
        h_flex()
          .w_full()
          .gap_2()
          .child(
            div()
              .font_weight(FontWeight::SEMIBOLD)
              .text_color(theme.foreground)
              .child(SharedString::from(input.name.clone())),
          )
          .when_some(input.intent.clone(), |this, intent| {
            this.child(
              div()
                .min_w_0()
                .flex_1()
                .text_color(theme.foreground)
                .child(SharedString::from(intent)),
            )
          })
          .when_some(status, |this, (status, color)| {
            this.child(
              div()
                .text_sm()
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(color.opacity(0.45))
                .bg(color.opacity(0.12))
                .text_color(theme.foreground)
                .child(status),
            )
          }),
      )
      .child(self.card.clone())
      .when_some(wall, |this, wall| {
        this.child(
          div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(SharedString::from(format!("{wall} ms"))),
        )
      })
  }
}

struct ToolCard {
  id: SharedString,
  input: ToolInput,
  presentation: Presentation,
  raw: String,
  open: bool,
  full: bool,
  raw_open: bool,
  diff: Option<Entity<DiffState>>,
  extra: String,
  overflow: bool,
  fallback: bool,
}

impl ToolCard {
  fn new(id: SharedString, input: &ToolInput, cx: &mut Context<Self>) -> Self {
    let mut card = Self {
      id,
      input: input.clone(),
      presentation: generic::parse(None),
      raw: String::new(),
      open: true,
      full: false,
      raw_open: false,
      diff: None,
      extra: String::new(),
      overflow: false,
      fallback: false,
    };
    card.prepare(input, cx);
    cx.observe_global::<gpui_kit::component::Theme>(|card, cx| {
      if let Some(diff) = &card.diff {
        let theme = cx.theme().highlight_theme.clone();
        diff.update(cx, |state, cx| {
          state.set_syntax_highlighter(super::super::markdown::syntax_highlighter(theme), cx)
        });
      }
    })
    .detach();
    card
  }

  fn sync(&mut self, input: &ToolInput, cx: &mut Context<Self>) {
    if self.input != *input {
      if self.input.name != input.name || self.input.args != input.args || self.input.result != input.result {
        self.prepare(input, cx);
      } else if self.input.stream != input.stream {
        self.raw = raw_text(input);
        if self.fallback {
          self.presentation.content.clone_from(&self.raw);
        }
      }
      self.input.clone_from(input);
      cx.notify();
    }
  }

  fn prepare(&mut self, input: &ToolInput, cx: &mut Context<Self>) {
    let args = input.args.as_ref();
    let result = input.result.as_ref();
    let parsed = match input.name.as_str() {
      "bash" => bash::parse(args, result),
      "read" => read::parse(args, result),
      "write" => write::parse(args),
      "edit" => edit::parse(args, result),
      "grep" | "glob" | "find" => search::parse(args, result),
      "task" => task::parse(args, result),
      _ => None,
    };
    let raw = raw_text(input);
    let fallback = parsed.is_none();
    self.fallback = fallback;
    let mut presentation = parsed.unwrap_or_else(|| generic::parse(result));
    if fallback {
      presentation.content = raw.clone();
    }
    let additional = result.map(result_text).unwrap_or_default();
    self.extra = if !fallback && !matches!(input.name.as_str(), "bash" | "task") && additional != presentation.content {
      additional
    } else {
      String::new()
    };
    if presentation.diff && (!self.presentation.diff || self.presentation.content != presentation.content || self.diff.is_none()) {
      match DiffFile::parse(&presentation.content) {
        Ok(files) => {
          if let Some(diff) = &self.diff {
            diff.update(cx, |state, cx| state.set_files(files, cx));
          } else {
            let theme = cx.theme().highlight_theme.clone();
            let diff = cx.new(|cx| DiffState::new(files, cx).with_syntax_highlighter(super::super::markdown::syntax_highlighter(theme)));
            cx.observe(&diff, |_, _, cx| cx.notify()).detach();
            self.diff = Some(diff);
          }
        }
        Err(error) => {
          tracing::warn!(%error, "invalid tool diff");
          presentation.diff = false;
          self.diff = None;
        }
      }
    } else if !presentation.diff {
      self.diff = None;
    }
    self.presentation = presentation;
    self.raw = raw;
  }

  fn code(&self, part: &str, value: &str, language: &str) -> TextView {
    TextView::markdown(SharedString::from(format!("{}-{part}", self.id)), fenced(value, language))
  }
}

impl Render for ToolCard {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let id = self.id.clone();
    let progress = spring(
      (id.clone(), "disclosure"),
      if self.open { 1.0 } else { 0.0 },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    let theme = cx.theme();
    let end = preview_end(&self.presentation.content);
    let images = self
      .input
      .result
      .iter()
      .filter_map(|result| result.get("content").and_then(Value::as_array))
      .flatten()
      .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
      .collect::<Vec<_>>();
    let long = self.overflow
      || end < self.presentation.content.len()
      || preview_end(&self.extra) < self.extra.len()
      || (self.raw_open && preview_end(&self.raw) < self.raw.len());
    let visible = if self.full {
      self.presentation.content.as_str()
    } else {
      self.presentation.content.get(..end).unwrap_or_default()
    };
    let language = if self.presentation.file {
      std::path::Path::new(&self.presentation.summary)
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
          extension
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '_'))
        })
        .unwrap_or_default()
    } else {
      ""
    };
    let content: AnyElement = if self.presentation.diff {
      if let Some(diff) = &self.diff {
        let rows = if self.full {
          self.presentation.content.lines().count()
        } else {
          PREVIEW_LINES
        };
        Diff::new(diff)
          .h(rems(1.5 * rows.saturating_add(2) as f32))
          .w_full()
          .into_any_element()
      } else {
        self.code("content", visible, language).into_any_element()
      }
    } else {
      self.code("content", visible, language).into_any_element()
    };
    let exit = self
      .input
      .result
      .as_ref()
      .and_then(|result| result.pointer("/details/exitCode"))
      .and_then(Value::as_i64)
      .or(self.input.exit_code);
    let raw_end = if self.full { self.raw.len() } else { preview_end(&self.raw) };
    let raw = self.raw.get(..raw_end).unwrap_or_default();
    let owner = cx.entity();
    let overflow = self.overflow;
    let measure = gpui_kit::canvas(
      move |bounds, window, _| {
        let next = bounds.size.height > window.rem_size() * 20.;
        if next != overflow {
          window.on_next_frame(move |_, cx| {
            owner.update(cx, |this, cx| {
              if this.overflow != next {
                this.overflow = next;
                cx.notify();
              }
            })
          });
        }
      },
      |_, (), _, _| {},
    )
    .absolute()
    .size_full();
    let body_content = v_flex()
      .relative()
      .w_full()
      .min_w_0()
      .flex_shrink_0()
      .gap_1()
      .child(content)
      .when(!self.extra.is_empty(), |this| {
        let end = if self.full { self.extra.len() } else { preview_end(&self.extra) };
        this.child(self.code("additional", self.extra.get(..end).unwrap_or_default(), ""))
      })
      .children(images.into_iter().map(|block| {
        let mime = block.get("mimeType").and_then(Value::as_str).unwrap_or("unknown format");
        let description = match block.get("data").and_then(Value::as_str) {
          Some(data) => format!("{mime} · {} encoded bytes", data.len()),
          None => mime.to_owned(),
        };
        let mut media = AttachmentMedia::new();
        if let Some(url) = block.get("url").and_then(Value::as_str) {
          media = media.src(url.to_owned());
        }
        Attachment::new().media(media).content(
          AttachmentContent::new()
            .title(AttachmentTitle::new("Image"))
            .description(AttachmentDescription::new(description)),
        )
      }))
      .when(self.raw_open, |this| this.child(self.code("raw", raw, "json")))
      .child(measure);
    let body = v_flex()
      .id(SharedString::from(format!("{id}-body")))
      .test_support()
      .w_full()
      .min_w_0()
      .when(!self.full, |this| this.max_h(rems(20.)).overflow_hidden())
      .child(body_content);
    let details = v_flex()
      .id(SharedString::from(format!("{id}-details")))
      .test_support()
      .w_full()
      .min_w_0()
      .p_2()
      .gap_1()
      .when(long, |this| {
        this.child(
          Button::new(SharedString::from(format!("{id}-extent")))
            .ghost()
            .xsmall()
            .label(if self.full { "Show less" } else { "Show all" })
            .selected(self.full)
            .toggled(self.full)
            .on_click(cx.listener(|this, _, _, cx| {
              this.full = !this.full;
              cx.notify();
            })),
        )
      })
      .when(long, |this| {
        this.child(
          Button::new(SharedString::from(format!("{id}-copy")))
            .ghost()
            .xsmall()
            .label("Copy full output")
            .on_click(cx.listener(|this, _, _, cx| {
              let mut output = this.presentation.content.clone();
              if !this.extra.is_empty() {
                if !output.is_empty() {
                  output.push('\n');
                }
                output.push_str(&this.extra);
              }
              cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(output));
            })),
        )
      })
      .child(
        Button::new(SharedString::from(format!("{id}-raw-toggle")))
          .ghost()
          .xsmall()
          .label(if self.raw_open { "Hide raw data" } else { "Show raw data" })
          .selected(self.raw_open)
          .toggled(self.raw_open)
          .on_click(cx.listener(|this, _, _, cx| {
            this.raw_open = !this.raw_open;
            cx.notify();
          })),
      )
      .when(self.raw_open && preview_end(&self.raw) < self.raw.len(), |this| {
        this.child(
          Button::new(SharedString::from(format!("{id}-copy-raw")))
            .ghost()
            .xsmall()
            .label("Copy full raw data")
            .on_click(cx.listener(|this, _, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(this.raw.clone())))),
        )
      })
      .child(body)
      .when_some(exit.filter(|_| self.input.name == "bash"), |this, code| {
        this.child(
          div()
            .text_sm()
            .text_color(theme.foreground)
            .child(SharedString::from(format!("Exit code {code}"))),
        )
      })
      .into_any_element();
    let reveal = if cx.reduce_motion() {
      details
    } else {
      MotionReveal::new(SharedString::from(format!("{id}-reveal")), progress, details).into_any_element()
    };
    v_flex()
      .w_full()
      .min_w_0()
      .rounded_md()
      .border_1()
      .border_color(theme.primary.opacity(0.4))
      .bg(theme.primary.opacity(0.06))
      .child(
        Button::new(SharedString::from(format!("{id}-toggle")))
          .ghost()
          .xsmall()
          .w_full()
          .justify_start()
          .selected(self.open)
          .toggled(self.open)
          .accessibility_label(if self.open { "Collapse tool card" } else { "Expand tool card" })
          .child(Icon::new(if self.open { IconName::ChevronDown } else { IconName::ChevronRight }))
          .child(
            div()
              .min_w_0()
              .flex_1()
              .truncate()
              .text_color(theme.foreground)
              .child(SharedString::from(self.presentation.summary.clone())),
          )
          .on_click(cx.listener(|this, _, _, cx| {
            this.open = !this.open;
            cx.notify();
          })),
      )
      .when(self.open || (!cx.reduce_motion() && progress > 0.0), |this| this.child(reveal))
  }
}
