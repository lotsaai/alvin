//! Retained tool call presentation and independent disclosure state.

mod bash;
mod code;
mod edit;
mod generic;
mod read;
mod resource;
mod script;
mod search;
mod task;
mod write;

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::HashSet;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::base::motion::{MotionReveal, spring};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement as _, ParentElement as _, Render,
  SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, rems,
};
use serde_json::Value;

use super::content::image::PreparedImage;

const PREVIEW_LINES: usize = 12;
const PREVIEW_CHARS: usize = 2400;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ToolInput {
  pub name: String,
  pub intent: Option<String>,
  pub args: Option<Value>,
  pub delivery: Option<Value>,
  /// Task updates can continue after the tool returns its spawn response.
  pub task_progress: Option<Value>,
  pub result: Option<Value>,
  pub stream: Vec<Value>,
  pub running: bool,
  pub failed: bool,
  pub exit_code: Option<i64>,
  pub timestamp: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolStatus {
  Running,
  Failed,
  Completed,
  Pending,
}

impl ToolInput {
  fn exit_code(&self) -> Option<i64> {
    self
      .result
      .as_ref()
      .and_then(|result| result.pointer("/details/exitCode"))
      .and_then(Value::as_i64)
      .or(self.exit_code)
  }

  fn status(&self) -> ToolStatus {
    if self.running {
      ToolStatus::Running
    } else if self.failed || self.exit_code().is_some_and(|code| code != 0) {
      ToolStatus::Failed
    } else if self.result.is_some() || self.exit_code().is_some() {
      ToolStatus::Completed
    } else {
      ToolStatus::Pending
    }
  }
}

#[derive(Clone)]
enum Summary {
  Label(String),
  Resource(String),
  Search { pattern: Option<String>, path: Option<String> },
  Task(Option<String>),
}

impl Summary {
  fn text(&self) -> &str {
    match self {
      Self::Label(text) | Self::Resource(text) => text,
      Self::Search { pattern, path } => pattern.as_deref().or(path.as_deref()).unwrap_or("Results"),
      Self::Task(task) => task.as_deref().unwrap_or("Subagents"),
    }
  }
}

#[derive(Clone)]
enum Body {
  Text(String),
  File(String),
  Read {
    targets: Vec<read::Target>,
    display: Option<read::DisplayContent>,
    output: String,
  },
  Search(search::SearchResults),
  Task {
    agents: Vec<task::Agent>,
    output: String,
  },
  Diff(String),
  Script {
    source: String,
    language: String,
    output: String,
  },
  Raw(String),
}

impl Body {
  fn content(&self) -> &str {
    match self {
      Self::Text(content) | Self::File(content) | Self::Diff(content) | Self::Raw(content) => content,
      Self::Read { display, output, .. } => display.as_ref().map_or(output, |display| &display.text),
      Self::Search(results) => &results.output,
      Self::Task { output, .. } => output,
      Self::Script { output, .. } => output,
    }
  }

  fn metadata(&self) -> String {
    match self {
      Self::Read {
        display: Some(display), ..
      } => {
        let mut parts = Vec::new();
        if let Some(start) = display.start_line {
          parts.push(format!("Starting line {start}"));
        }
        if !display.line_numbers.is_empty() {
          let count = display.line_numbers.len();
          parts.push(format!("{count} reported {}", if count == 1 { "line" } else { "lines" }));
        }
        parts.join(" · ")
      }
      Self::Search(results) => {
        let mut parts = Vec::new();
        if let Some(count) = results.match_count {
          parts.push(format!("{count} {}", if count == 1 { "match" } else { "matches" }));
        }
        if let Some(count) = results.file_count {
          parts.push(format!("{count} {}", if count == 1 { "file" } else { "files" }));
        }
        if results.truncated == Some(true) {
          parts.push("Truncated".to_owned());
        }
        if !results.matches.is_empty() {
          parts.extend(results.matches.iter().map(|entry| match entry.count {
            Some(count) => format!("{} ({count} {})", entry.path, if count == 1 { "match" } else { "matches" }),
            None => entry.path.clone(),
          }));
        } else {
          parts.extend(results.files.iter().cloned());
        }
        parts.join(" · ")
      }
      Self::Task { agents, .. } => format!("{} {}", agents.len(), if agents.len() == 1 { "agent" } else { "agents" }),
      _ => String::new(),
    }
  }
}

struct Presentation {
  summary: Summary,
  body: Body,
}

impl Presentation {
  fn text(summary: String, content: String) -> Self {
    Self {
      summary: Summary::Label(summary),
      body: Body::Text(content),
    }
  }
  fn file(summary: String, content: String) -> Self {
    Self {
      summary: Summary::Resource(summary),
      body: Body::File(content),
    }
  }
  fn diff(summary: String, content: String) -> Self {
    Self {
      summary: Summary::Resource(summary),
      body: Body::Diff(content),
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolCapability {
  Readonly,
  PotentiallyMutating,
}

pub(super) fn capability(name: &str) -> ToolCapability {
  match name {
    "read" | "grep" | "glob" | "find" | "ast_grep" | "web_search" => ToolCapability::Readonly,
    _ => ToolCapability::PotentiallyMutating,
  }
}

fn present(input: &ToolInput) -> Option<Presentation> {
  let args = input.args.as_ref();
  let result = input.result.as_ref();
  match (capability(&input.name), input.name.as_str()) {
    (ToolCapability::PotentiallyMutating, "bash") => bash::parse(args, result),
    (ToolCapability::Readonly, "read") => read::parse(args, result),
    (ToolCapability::PotentiallyMutating, "eval") => match (text(args, "code"), text(args, "language")) {
      (Some(source), Some(language)) if result.is_none_or(|value| value.get("content").and_then(Value::as_array).is_some()) => {
        Some(Presentation {
          summary: Summary::Label("Script".into()),
          body: Body::Script {
            source: source.to_owned(),
            language: language.to_owned(),
            output: result.map(result_text).unwrap_or_default(),
          },
        })
      }
      _ => None,
    },
    (ToolCapability::PotentiallyMutating, "write") => write::parse(args),
    (ToolCapability::PotentiallyMutating, "edit") => edit::parse(args, result).or_else(|| match result {
      Some(result) if result.get("content").and_then(Value::as_array).is_some() => Some(Presentation::text(
        text(args, "path").unwrap_or("Details").to_owned(),
        result_text(result),
      )),
      None => edit::preview(&input.stream),
      _ => None,
    }),
    (ToolCapability::Readonly, "grep" | "glob" | "find") => search::parse(args, result),
    (ToolCapability::PotentiallyMutating, "task") => task::parse(args, result, input.task_progress.as_ref()),
    _ => None,
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

fn raw_text(input: &ToolInput) -> String {
  let data = serde_json::json!({
    "arguments": input.args,
    "result": input.result,
    "stream": input.stream,
    "taskProgress": input.task_progress,
    "delivery": input.delivery,
  });
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
    let name = match input.name.as_str() {
      "bash" => "Bash",
      "eval" => "Eval",
      "read" => "Read",
      "write" => "Write",
      "edit" => "Edit",
      "grep" => "Grep",
      "glob" => "Glob",
      "find" => "Find",
      "task" => "Task",
      other => other,
    };
    let content = v_flex()
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
              .child(SharedString::from(name.to_owned())),
          )
          .when_some(input.intent.clone(), |this, intent| {
            this.child(
              div()
                .min_w_0()
                .flex_1()
                .text_color(theme.muted_foreground)
                .child(SharedString::from(intent)),
            )
          }),
      )
      .child(self.card.clone())
      .into_any_element();
    let icon = match input.name.as_str() {
      "bash" | "eval" => IconName::SquareTerminal,
      "read" => IconName::FileText,
      "write" => IconName::File,
      "edit" => IconName::Replace,
      "grep" | "glob" | "find" => IconName::Search,
      "task" => IconName::Bot,
      _ => IconName::Settings2,
    };
    super::lane::item(self.card.read(cx).id.clone(), icon, content, cx)
  }
}

struct ToolCard {
  id: SharedString,
  input: ToolInput,
  capability: ToolCapability,
  presentation: Presentation,
  script: Option<Entity<script::ScriptPanel>>,
  images: Vec<PreparedImage>,
  raw: String,
  open: Option<bool>,
  full: bool,
  raw_open: bool,
  open_targets: HashSet<SharedString>,
  diff: Option<Entity<DiffState>>,
  extra: String,
  metadata: String,
  overflow: bool,
}

impl ToolCard {
  fn new(id: SharedString, input: &ToolInput, cx: &mut Context<Self>) -> Self {
    let mut card = Self {
      id,
      input: input.clone(),
      capability: capability(&input.name),
      presentation: generic::parse(None),
      script: None,
      images: Vec::new(),
      raw: String::new(),
      open: None,
      full: false,
      raw_open: false,
      open_targets: HashSet::new(),
      diff: None,
      extra: String::new(),
      metadata: String::new(),
      overflow: false,
    };
    card.prepare(input, cx);
    cx.observe_global::<crate::ConversationPreferences>(|_, cx| cx.notify()).detach();
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
    let retain_stream = input.stream.is_empty() && !self.input.stream.is_empty();
    let retain_progress = input.task_progress.is_none() && self.input.task_progress.is_some();
    if retain_stream || retain_progress {
      let stream = if retain_stream {
        std::mem::take(&mut self.input.stream)
      } else {
        Vec::new()
      };
      let progress = if retain_progress { self.input.task_progress.take() } else { None };
      let unchanged = self.input == *input;
      if retain_stream {
        self.input.stream = stream;
      }
      if retain_progress {
        self.input.task_progress = progress;
      }
      if unchanged {
        return;
      }
    }
    let mut normalized = Cow::Borrowed(input);
    if retain_stream {
      normalized.to_mut().stream.clone_from(&self.input.stream);
    }
    if retain_progress {
      normalized.to_mut().task_progress.clone_from(&self.input.task_progress);
    }
    let input = normalized.as_ref();
    if self.input != *input {
      if self.input.name != input.name
        || self.input.args != input.args
        || self.input.result != input.result
        || self.input.delivery != input.delivery
        || self.input.task_progress != input.task_progress
        || self.input.stream != input.stream
      {
        self.prepare(input, cx);
      }
      self.input = normalized.into_owned();
      cx.notify();
    }
  }

  fn prepare(&mut self, input: &ToolInput, cx: &mut Context<Self>) {
    self.capability = capability(&input.name);
    let result = input.result.as_ref();
    if self.input.result != input.result || self.images.is_empty() {
      let mut previous = std::mem::take(&mut self.images).into_iter();
      self.images = result
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        .map(|block| {
          let image = alvin_protocol::ImageContent {
            data: block.get("data").and_then(Value::as_str).map(str::to_owned),
            mime_type: block.get("mimeType").and_then(Value::as_str).map(str::to_owned),
            url: block.get("url").and_then(Value::as_str).map(str::to_owned),
            ..Default::default()
          };
          match previous.next() {
            Some(mut prepared) => {
              prepared.sync(&image);
              prepared
            }
            None => PreparedImage::new(&image),
          }
        })
        .collect();
    }
    let parsed = present(input);
    let raw = raw_text(input);
    let fallback = parsed.is_none();
    let mut presentation = parsed.unwrap_or_else(|| generic::parse(result));
    if let (Body::Task { agents, output }, Some(result), Some(delivery)) = (&mut presentation.body, result, input.delivery.as_ref()) {
      task::settle(agents, output, result, delivery);
    }
    if fallback {
      presentation.body = Body::Raw(raw.clone());
    }
    let additional = result.map(result_text).unwrap_or_default();
    self.extra = if !fallback && !matches!(input.name.as_str(), "bash" | "eval" | "task") && additional != presentation.body.content() {
      additional
    } else {
      String::new()
    };
    if matches!(presentation.body, Body::Diff(_))
      && (!matches!(self.presentation.body, Body::Diff(_))
        || self.presentation.body.content() != presentation.body.content()
        || self.diff.is_none())
    {
      match DiffFile::parse(presentation.body.content()) {
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
          presentation.body = Body::Text(presentation.body.content().to_owned());
          self.diff = None;
        }
      }
    } else if !matches!(presentation.body, Body::Diff(_)) {
      self.diff = None;
    }
    if let Body::Script { source, language, output } = &presentation.body {
      if let Some(script) = &self.script {
        script.update(cx, |script, cx| script.sync(source, language, output, cx));
      } else {
        let script = cx.new(|cx| script::ScriptPanel::new(self.id.clone(), source.clone(), language.clone(), output.clone(), cx));
        cx.observe(&script, |_, _, cx| cx.notify()).detach();
        self.script = Some(script);
      }
    } else {
      self.script = None;
    }
    self.metadata = presentation.body.metadata();
    self.presentation = presentation;
    self.raw = raw;
  }

  fn code(&self, part: &str, value: &str, language: &str, cx: &gpui_kit::App) -> AnyElement {
    code::view(SharedString::from(format!("{}-{part}", self.id)), value, language, cx)
  }
}

impl Render for ToolCard {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui_kit::IntoElement {
    let id = self.id.clone();
    let preferences = crate::ConversationPreferences::current(cx);
    let open = self.open.unwrap_or(match self.capability {
      ToolCapability::Readonly => preferences.expand_readonly_tools,
      ToolCapability::PotentiallyMutating => preferences.expand_mutating_tools,
    });
    let progress = spring(
      (id.clone(), "disclosure"),
      if open { 1.0 } else { 0.0 },
      cx.theme().motion_tokens().spring_control,
      window,
      cx,
    );
    let owner_id = gpui_kit::ElementId::View(cx.entity_id());
    let toggle_focus = crate::session::focus::tab_stop((owner_id.clone(), "toggle"), window, cx);
    let extent_focus = crate::session::focus::tab_stop((owner_id.clone(), "extent"), window, cx);
    let copy_focus = crate::session::focus::tab_stop((owner_id.clone(), "copy"), window, cx);
    let raw_focus = crate::session::focus::tab_stop((owner_id, "raw"), window, cx);
    let end = preview_end(self.presentation.body.content());
    let long = if self.script.is_some() && !self.raw_open {
      false
    } else {
      self.overflow
        || end < self.presentation.body.content().len()
        || preview_end(&self.extra) < self.extra.len()
        || (self.raw_open && preview_end(&self.raw) < self.raw.len())
    };
    let visible = if self.full {
      self.presentation.body.content()
    } else {
      self.presentation.body.content().get(..end).unwrap_or_default()
    };
    let language = if matches!(self.presentation.body, Body::File(_) | Body::Read { .. }) {
      std::path::Path::new(self.presentation.summary.text())
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| {
          extension
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '_'))
        })
        .unwrap_or_default()
    } else if matches!(self.presentation.body, Body::Raw(_)) {
      "json"
    } else if self.script.is_some() {
      "text"
    } else {
      ""
    };
    let content: AnyElement = if self.raw_open {
      let end = if self.full { self.raw.len() } else { preview_end(&self.raw) };
      self.code("raw", self.raw.get(..end).unwrap_or_default(), "json", cx)
    } else if let Some(script) = &self.script {
      script.clone().into_any_element()
    } else if matches!(self.presentation.body, Body::Diff(_)) {
      if let Some(diff) = &self.diff {
        let rows = if self.full {
          self.presentation.body.content().lines().count()
        } else {
          PREVIEW_LINES
        };
        Diff::new(diff)
          .h(rems(1.5 * rows.saturating_add(2) as f32))
          .w_full()
          .into_any_element()
      } else {
        self.code("content", visible, language, cx)
      }
    } else {
      self.code("content", visible, language, cx)
    };
    let exit = self.input.exit_code();
    let status = self.input.status();
    let wall = self
      .input
      .result
      .as_ref()
      .and_then(|result| result.pointer("/details/wallTimeMs"))
      .and_then(Value::as_f64)
      .filter(|wall| wall.is_finite() && *wall >= 0.);
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
      .when(!self.raw_open, |this| match &self.presentation.body {
        Body::Read { targets, .. } => this.children(targets.iter().enumerate().map(|(index, target)| {
          let expanded = self.open_targets.contains(&target.selection_key);
          let key = target.selection_key.clone();
          let focus_scope = gpui_kit::ElementId::from((gpui_kit::ElementId::View(cx.entity_id()), target.selection_key.clone()));
          let focus = crate::session::focus::tab_stop((focus_scope.clone(), "toggle"), window, cx);
          let path_focus = crate::session::focus::tab_stop((focus_scope.clone(), "path-copy"), window, cx);
          let link_focus = crate::session::focus::tab_stop((focus_scope, "link-copy"), window, cx);
          let path = expanded.then(|| SharedString::from(target.path.clone()));
          let link = target.link.clone();
          v_flex()
            .id(SharedString::from(format!("{id}-target-{index}")))
            .test_support()
            .w_full()
            .min_w_0()
            .overflow_x_hidden()
            .child(
              v_flex()
                .id(target.key.clone())
                .min_w_0()
                .child(
                  Button::new("target-toggle")
                    .track_focus(&focus)
                    .ghost()
                    .xsmall()
                    .min_w_0()
                    .justify_start()
                    .toggled(expanded)
                    .accessibility_label(target.label.clone())
                    .child(Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight }))
                    .child(resource::view(&target.path, None, cx))
                    .on_click(cx.listener(move |this, _, _, cx| {
                      if !this.open_targets.remove(&key) {
                        this.open_targets.insert(key.clone());
                      }
                      cx.notify();
                    })),
                )
                .when_some(path, |row, path| {
                  row.child(
                    v_flex()
                      .id("target-details")
                      .test_support()
                      .min_w_0()
                      .w_full()
                      .pl_4()
                      .child(
                        h_flex()
                          .min_w_0()
                          .child(
                            div()
                              .id("target-path")
                              .test_support()
                              .aria_label(path.clone())
                              .min_w_0()
                              .flex_1()
                              .child(path.clone()),
                          )
                          .child(
                            Button::new("target-path-copy")
                              .track_focus(&path_focus)
                              .ghost()
                              .xsmall()
                              .icon(IconName::Copy)
                              .accessibility_label("Copy target path")
                              .on_click(move |_, _, cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path.to_string()))),
                          ),
                      )
                      .when_some(link, |details, link| {
                        let copy_link = link.clone();
                        details.child(
                          h_flex()
                            .min_w_0()
                            .child(
                              div()
                                .id("target-link")
                                .test_support()
                                .aria_label(link.clone())
                                .min_w_0()
                                .flex_1()
                                .child(link),
                            )
                            .child(
                              Button::new("target-link-copy")
                                .track_focus(&link_focus)
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .accessibility_label("Copy target link")
                                .on_click(move |_, _, cx| {
                                  cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy_link.to_string()))
                                }),
                            ),
                        )
                      }),
                  )
                }),
            )
        })),
        Body::Search(results) => this.children(if results.matches.is_empty() {
          results.files.iter().map(|path| resource::view(path, None, cx)).collect::<Vec<_>>()
        } else {
          results
            .matches
            .iter()
            .map(|entry| {
              let count = entry
                .count
                .map(|count| format!("{count} {}", if count == 1 { "match" } else { "matches" }));
              resource::view(&entry.path, count.as_deref(), cx)
            })
            .collect()
        }),
        Body::Task { agents, .. } => this.child(task::view(agents, cx)),
        _ => this,
      });
    let theme = cx.theme();
    let output_content = v_flex()
      .relative()
      .w_full()
      .min_w_0()
      .flex_shrink_0()
      .gap_1()
      .when(!self.raw_open && !self.metadata.is_empty(), |this| {
        this.child(
          div()
            .id(SharedString::from(format!("{id}-metadata")))
            .test_support()
            .aria_label(SharedString::from(self.metadata.clone()))
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(SharedString::from(self.metadata.clone())),
        )
      })
      .when(
        !self.raw_open && matches!(&self.presentation.body, Body::Read { targets, .. } if targets.len() > 1),
        |this| this.child(div().text_sm().text_color(theme.muted_foreground).child("Combined output")),
      )
      .child(content)
      .when(!self.raw_open && !self.extra.is_empty(), |this| {
        let end = if self.full { self.extra.len() } else { preview_end(&self.extra) };
        this.child(self.code("additional", self.extra.get(..end).unwrap_or_default(), "", cx))
      })
      .children(
        self
          .images
          .iter()
          .enumerate()
          .filter(|_| !self.raw_open)
          .map(|(index, image)| image.view(SharedString::from(format!("{id}-image-{index}")))),
      )
      .when(self.raw_open || self.script.is_none(), |this| this.child(measure));
    let output = v_flex()
      .id(SharedString::from(format!("{id}-output-preview")))
      .test_support()
      .w_full()
      .min_w_0()
      .when(
        !self.full && !self.raw_open && matches!(self.presentation.body, Body::Read { .. }),
        |this| this.max_h(rems(20.)).overflow_hidden(),
      )
      .child(output_content);
    let body = v_flex()
      .id(SharedString::from(format!("{id}-body")))
      .test_support()
      .w_full()
      .min_w_0()
      .when(
        !self.full && (self.raw_open || !matches!(self.presentation.body, Body::Read { .. })) && (self.raw_open || self.script.is_none()),
        |this| this.max_h(rems(20.)).overflow_hidden(),
      )
      .child(body_content.child(output));
    let details = v_flex()
      .id(SharedString::from(format!("{id}-details")))
      .test_support()
      .w_full()
      .min_w_0()
      .when(self.raw_open || self.script.is_none(), |this| this.p_2())
      .child(body)
      .into_any_element();
    let reveal = if cx.reduce_motion() {
      details
    } else {
      MotionReveal::new(SharedString::from(format!("{id}-reveal")), progress, details).into_any_element()
    };
    v_flex()
      .w_full()
      .min_w_0()
      .when(
        open && !matches!(self.presentation.body, Body::Read { .. } | Body::Search(_)),
        |this| {
          this
            .rounded_md()
            .border_1()
            .border_color(if status == ToolStatus::Failed {
              theme.danger.opacity(0.8)
            } else {
              theme.border
            })
            .bg(theme.muted.opacity(0.25))
        },
      )
      .child(
        h_flex()
          .id(SharedString::from(format!("{id}-header")))
          .test_support()
          .w_full()
          .min_w_0()
          .when(
            open && !matches!(self.presentation.body, Body::Read { .. } | Body::Search(_)),
            |this| this.border_b_1().border_color(theme.border),
          )
          .child(
            Button::new(SharedString::from(format!("{id}-toggle")))
              .track_focus(&toggle_focus)
              .ghost()
              .xsmall()
              .flex_1()
              .min_w_0()
              .justify_start()
              .toggled(open)
              .accessibility_label(if open { "Collapse tool card" } else { "Expand tool card" })
              .tooltip(self.presentation.summary.text().to_owned())
              .child(Icon::new(if open { IconName::ChevronDown } else { IconName::ChevronRight }))
              .child(
                div()
                  .id(SharedString::from(format!("{id}-summary")))
                  .test_support()
                  .min_w_0()
                  .flex_1()
                  .truncate()
                  .child(match &self.presentation.summary {
                    Summary::Resource(path) => {
                      let suffix = match &self.presentation.body {
                        Body::Read {
                          display: Some(display), ..
                        } => {
                          match (
                            display.line_numbers.first().copied().or(display.start_line),
                            display.line_numbers.last().copied(),
                          ) {
                            (Some(start), Some(end)) if start != end => Some(format!("lines {start}–{end}")),
                            (Some(start), _) => Some(format!("line {start}")),
                            _ => None,
                          }
                        }
                        _ => None,
                      };
                      resource::view(path, suffix.as_deref(), cx)
                    }
                    _ => div()
                      .text_color(theme.foreground)
                      .child(SharedString::from(self.presentation.summary.text().to_owned()))
                      .into_any_element(),
                  }),
              )
              .on_click(cx.listener(move |this, _, _, cx| {
                this.open = Some(!open);
                cx.notify();
              })),
          )
          .when(long, |this| {
            this.child(
              Button::new(SharedString::from(format!("{id}-extent")))
                .track_focus(&extent_focus)
                .ghost()
                .xsmall()
                .icon(if self.full { IconName::Minimize } else { IconName::Maximize })
                .accessibility_label(if self.full { "Show less output" } else { "Show all output" })
                .tooltip(if self.full { "Show less output" } else { "Show all output" })
                .selected(self.full)
                .toggled(self.full)
                .on_click(cx.listener(|this, _, _, cx| {
                  this.full = !this.full;
                  cx.notify();
                })),
            )
          })
          .child(
            Button::new(SharedString::from(format!("{id}-copy")))
              .track_focus(&copy_focus)
              .ghost()
              .xsmall()
              .icon(IconName::Copy)
              .accessibility_label(if self.raw_open { "Copy full raw data" } else { "Copy full output" })
              .tooltip(if self.raw_open { "Copy full raw data" } else { "Copy full output" })
              .on_click(cx.listener(|this, _, _, cx| {
                let mut output = if this.raw_open {
                  this.raw.clone()
                } else if let Body::Read { output, .. } = &this.presentation.body {
                  output.clone()
                } else {
                  this.presentation.body.content().to_owned()
                };
                if !this.raw_open && !matches!(this.presentation.body, Body::Read { .. }) && !this.extra.is_empty() {
                  if !output.is_empty() {
                    output.push('\n');
                  }
                  output.push_str(&this.extra);
                }
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(output));
              })),
          )
          .child(
            Button::new(SharedString::from(format!("{id}-raw-toggle")))
              .track_focus(&raw_focus)
              .ghost()
              .xsmall()
              .icon(IconName::Info)
              .accessibility_label(if self.raw_open { "Hide raw data" } else { "Show raw data" })
              .tooltip(if self.raw_open { "Hide raw data" } else { "Show raw data" })
              .selected(self.raw_open)
              .toggled(self.raw_open)
              .on_click(cx.listener(|this, _, _, cx| {
                this.raw_open = !this.raw_open;
                cx.notify();
              })),
          ),
      )
      .when(open || (!cx.reduce_motion() && progress > 0.0), |this| this.child(reveal))
      .when(status != ToolStatus::Pending || exit.is_some() || wall.is_some(), |this| {
        let (label, icon, color) = match status {
          ToolStatus::Running => ("Running", IconName::Info, theme.info),
          ToolStatus::Failed => ("Failed", IconName::Close, theme.danger),
          ToolStatus::Completed => ("Completed", IconName::Check, theme.success),
          ToolStatus::Pending => ("Pending", IconName::Info, theme.muted_foreground),
        };
        this.child(
          h_flex()
            .border_t_1()
            .border_color(theme.border)
            .px_2()
            .py_2()
            .gap_1()
            .text_sm()
            .child(Icon::new(icon).small().text_color(color))
            .child(
              div()
                .id(SharedString::from(format!("{id}-status")))
                .test_support()
                .aria_label(label)
                .text_color(if theme.mode == gpui_kit::component::ThemeMode::Dark {
                  color
                } else {
                  theme.foreground.blend(color.opacity(0.5))
                })
                .child(label),
            )
            .when_some(exit, |this, code| {
              this.child(
                div()
                  .id(SharedString::from(format!("{id}-exit")))
                  .test_support()
                  .aria_label(SharedString::from(format!("Exit code {code}")))
                  .text_color(theme.foreground)
                  .child(SharedString::from(format!("Exit code {code}"))),
              )
            })
            .when_some(wall, |this, wall| {
              this.child(
                div()
                  .id(SharedString::from(format!("{id}-duration")))
                  .test_support()
                  .aria_label(SharedString::from(format!("{wall} ms")))
                  .text_color(theme.muted_foreground)
                  .child(SharedString::from(format!("{wall} ms"))),
              )
            }),
        )
      })
  }
}
