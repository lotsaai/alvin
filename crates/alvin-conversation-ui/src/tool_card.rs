//! One card per tool call, keyed by omp's tool call id.
//!
//! Tool arguments and results are open JSON in omp's schema. The built-in tools have a view of their own, read from the
//! fields omp's recorded runs show: bash output, file text, a diff for an edit, match lists for search tools, and the
//! subagents a `task` launched. A tool this crate does not know is the normal case, and a known tool whose result lacks
//! the field its view needs falls back the same way: the card shows its arguments and result as formatted JSON.

use alvin_conversation::ToolExecution;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::collapsible::Collapsible;
use gpui_kit::component::diff::{Diff, DiffFile, DiffState};
use gpui_kit::component::marker::{Marker, MarkerContent, MarkerIcon, MarkerLoadingStyle};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
  Window, div, prelude::FluentBuilder as _, rems,
};
use serde_json::Value;

#[derive(IntoElement)]
/// What a card shows. Built from a tool call in an assistant message, and from the execution tracked while it runs.
pub(crate) struct ToolCard {
  pub(crate) id: String,
  pub(crate) name: String,
  pub(crate) intent: Option<String>,
  pub(crate) args: Option<Value>,
  pub(crate) result: Option<Value>,
  pub(crate) running: bool,
  pub(crate) failed: bool,
}

/// What a card shows of a tool's arguments and result.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Body {
  /// A shell command and what it printed so far.
  Bash { command: String, output: String },
  /// A file's text: what `read` returned, or what `write` was asked to write.
  File { path: String, text: String },
  /// An edit as a unified patch of the change.
  Diff { patch: String },
  /// What a search tool found: a summary and one line per match or file.
  Matches { summary: String, lines: Vec<String> },
  /// The subagents a `task` launched and where each one is.
  Subagents(Vec<Subagent>),
  /// Any other tool: its arguments and result as they are.
  Json { args: Option<Value>, result: Option<Value> },
}

/// One subagent of a `task`, from the progress omp reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subagent {
  pub(crate) id: String,
  pub(crate) agent: String,
  pub(crate) status: String,
  /// The first line of what it was asked.
  pub(crate) task: String,
  pub(crate) tool_calls: i64,
}

/// The text of a tool result: its `content` blocks of type `text`, joined.
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

fn text(value: Option<&Value>, key: &str) -> Option<String> {
  value?.get(key)?.as_str().map(str::to_owned)
}

fn count(value: &Value, key: &str) -> Option<i64> {
  value.get(key)?.as_i64()
}

fn plural(n: i64, one: &str, many: &str) -> String {
  format!("{n} {}", if n == 1 { one } else { many })
}

/// omp's diff of an edit (` N|text`, `-N|text`, `+N|text`) as a one-hunk unified patch, or `None` when a line is not in that
/// shape, as when omp elides unchanged stretches.
fn unified(path: &str, diff: &str) -> Option<String> {
  let (mut start, mut old, mut new) = (None, 0usize, 0usize);
  let mut lines = String::new();
  for line in diff.lines() {
    let sign = line.chars().next().filter(|sign| matches!(sign, ' ' | '+' | '-'))?;
    let (number, content) = line.get(1..)?.split_once('|')?;
    start.get_or_insert(number.trim().parse::<usize>().ok()?);
    match sign {
      ' ' => (old, new) = (old + 1, new + 1),
      '-' => old += 1,
      _ => new += 1,
    }
    lines.push(sign);
    lines.push_str(content);
    lines.push('\n');
  }
  let start = start?;
  Some(format!("--- a/{path}\n+++ b/{path}\n@@ -{start},{old} +{start},{new} @@\n{lines}"))
}

/// The view of a tool call. `result` is the final result, or the partial one while the call runs.
pub(crate) fn body(name: &str, args: Option<&Value>, result: Option<&Value>) -> Body {
  let fallback = || Body::Json {
    args: args.cloned(),
    result: result.cloned(),
  };
  match name {
    "bash" => match text(args, "command") {
      Some(command) => Body::Bash {
        command,
        output: result.map(result_text).unwrap_or_default(),
      },
      None => fallback(),
    },
    "read" => match (text(args, "path"), result) {
      (Some(path), Some(result)) => Body::File {
        path,
        text: result
          .get("details")
          .and_then(|details| details.get("displayContent"))
          .and_then(|display| display.get("text"))
          .and_then(Value::as_str)
          .map_or_else(|| result_text(result), str::to_owned),
      },
      _ => fallback(),
    },
    "write" => match (text(args, "path"), text(args, "content")) {
      (Some(path), Some(text)) => Body::File { path, text },
      _ => fallback(),
    },
    "edit" => match result
      .and_then(|result| result.get("details"))
      .and_then(|details| text(Some(details), "diff"))
    {
      Some(diff) => match unified(&text(args, "path").unwrap_or_default(), &diff) {
        Some(patch) => Body::Diff { patch },
        None => fallback(),
      },
      None => fallback(),
    },
    "grep" => match result {
      Some(result) => {
        let details = result.get("details");
        let matches = details.and_then(|details| count(details, "matchCount")).unwrap_or(0);
        let files = details.and_then(|details| count(details, "fileCount")).unwrap_or(0);
        Body::Matches {
          summary: format!("{} in {}", plural(matches, "match", "matches"), plural(files, "file", "files")),
          lines: result_text(result).lines().map(str::to_owned).collect(),
        }
      }
      None => fallback(),
    },
    "glob" | "find" => match result
      .and_then(|result| result.get("details"))
      .and_then(|details| details.get("files"))
      .and_then(Value::as_array)
    {
      Some(files) => Body::Matches {
        summary: plural(i64::try_from(files.len()).unwrap_or(i64::MAX), "file", "files"),
        lines: files.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
      },
      None => fallback(),
    },
    "task" => match result
      .and_then(|result| result.get("details"))
      .and_then(|details| details.get("progress"))
      .and_then(Value::as_array)
    {
      Some(progress) if !progress.is_empty() => Body::Subagents(
        progress
          .iter()
          .map(|agent| Subagent {
            id: text(Some(agent), "id").unwrap_or_default(),
            agent: text(Some(agent), "agent").unwrap_or_default(),
            status: text(Some(agent), "status").unwrap_or_default(),
            task: text(Some(agent), "assignment")
              .and_then(|task| {
                task
                  .lines()
                  .find(|line| !line.trim().is_empty() && !line.starts_with('#'))
                  .map(str::to_owned)
              })
              .unwrap_or_default(),
            tool_calls: count(agent, "toolCount").unwrap_or(0),
          })
          .collect(),
      ),
      _ => fallback(),
    },
    _ => fallback(),
  }
}

impl ToolCard {
  /// The card of a tool call: its arguments from the message, and its progress when an execution is being tracked.
  pub(crate) fn of_call(id: &str, name: &str, intent: Option<&str>, args: Option<Value>, execution: Option<&ToolExecution>) -> Self {
    Self {
      id: id.to_owned(),
      name: name.to_owned(),
      intent: intent.map(str::to_owned),
      args: execution.and_then(|execution| execution.args.clone()).or(args),
      result: execution.and_then(|execution| {
        execution
          .outcome
          .as_ref()
          .and_then(|outcome| outcome.result.clone())
          .or_else(|| execution.partial.clone())
      }),
      running: execution.is_some_and(|execution| execution.outcome.is_none()),
      failed: execution.is_some_and(|execution| execution.outcome.as_ref().is_some_and(|outcome| outcome.is_error)),
    }
  }
}

impl RenderOnce for ToolCard {
  /// The card: a header that names the call and shows how it is going, and the call's arguments and result below it, folded
  /// away until the user asks. Whether it is open is local to the card, kept under its id.
  fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let id = SharedString::from(format!("tool-{}", self.id));
    let open = window.use_keyed_state(ElementId::from(SharedString::from(format!("{id}-open"))), cx, |_, _| false);
    let is_open = *open.read(cx);
    let body = body_element(&id, &body(&self.name, self.args.as_ref(), self.result.as_ref()), window, cx);
    let theme = cx.theme();
    let status = if self.running {
      Some(
        Marker::new()
          .loading(true)
          .with_loading_style(MarkerLoadingStyle::Shimmer)
          .content(MarkerContent::new().text("Running")),
      )
    } else if self.failed {
      Some(
        Marker::new()
          .text_color(theme.danger)
          .icon(MarkerIcon::new().child(Icon::new(IconName::CircleX)))
          .content(MarkerContent::new().text("Failed")),
      )
    } else {
      None
    };
    let toggle = {
      let open = open.clone();
      Button::new(SharedString::from(format!("{id}-toggle")))
        .ghost()
        .xsmall()
        .icon(if is_open { IconName::ChevronUp } else { IconName::ChevronDown })
        .accessibility_label(if is_open { "Hide tool details" } else { "Show tool details" })
        .on_click(move |_, _, cx| {
          open.update(cx, |open, cx| {
            *open = !*open;
            cx.notify();
          })
        })
    };
    v_flex()
      .id(ElementId::from(id.clone()))
      .w_full()
      .p_2()
      .border_1()
      .border_color(theme.border)
      .rounded(theme.radius)
      .child(
        Collapsible::new()
          .open(is_open)
          .gap_1()
          .child(
            h_flex()
              .gap_2()
              .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(self.name.clone())))
              .when_some(self.intent, |this, intent| {
                this.child(
                  div()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.muted_foreground)
                    .child(SharedString::from(intent)),
                )
              })
              .children(status)
              .child(div().flex_1())
              .child(toggle),
          )
          .content(body),
      )
  }
}

/// How many lines of code or output a block shows before it is clipped. The block's own copy action takes all of it.
const BLOCK_LINES: usize = 40;

/// `text` as a Markdown code block. The fence is longer than any run of backticks in the text, so the text cannot end it.
pub(crate) fn fenced(text: &str) -> String {
  let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or_default();
  let fence = "`".repeat(longest.max(2) + 1);
  format!("{fence}\n{text}\n{fence}")
}

/// A code block: selectable, with the copy action `TextView` gives its fences.
fn code_block(id: &SharedString, part: &str, text: &str) -> TextView {
  TextView::markdown(SharedString::from(format!("{id}-{part}")), fenced(text)).max_lines(BLOCK_LINES)
}

fn caption(text: String, cx: &App) -> gpui_kit::Div {
  div()
    .text_sm()
    .text_color(cx.theme().muted_foreground)
    .child(SharedString::from(text))
}

/// Formatted JSON as a code block.
fn json_block(id: &SharedString, part: &str, value: &Value) -> TextView {
  let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
  code_block(id, part, &text)
}

fn body_element(id: &SharedString, body: &Body, window: &mut Window, cx: &mut App) -> AnyElement {
  match body {
    Body::Bash { command, output } => v_flex()
      .gap_1()
      .child(code_block(id, "command", &format!("$ {command}")))
      .when(!output.is_empty(), |this| this.child(code_block(id, "output", output)))
      .into_any_element(),
    Body::File { path, text } => v_flex()
      .gap_1()
      .child(caption(path.clone(), cx))
      .child(code_block(id, "text", text))
      .into_any_element(),
    Body::Diff { patch } => {
      let files = window.use_keyed_state(ElementId::from(SharedString::from(format!("{id}-diff"))), cx, |_, cx| {
        let files = DiffFile::parse(patch).unwrap_or_else(|error| {
          tracing::warn!(%error, "an edit's diff is not a unified patch");
          Vec::new()
        });
        DiffState::new(files, cx)
      });
      let rows = patch.lines().count();
      Diff::new(&files)
        .h(rems(1.5 * (rows + 2).min(20) as f32))
        .w_full()
        .into_any_element()
    }
    Body::Matches { summary, lines } => v_flex()
      .gap_1()
      .child(caption(summary.clone(), cx))
      .child(code_block(id, "matches", &lines.join("\n")))
      .into_any_element(),
    Body::Subagents(agents) => v_flex()
      .gap_1()
      .children(agents.iter().map(|agent| {
        h_flex()
          .gap_2()
          .text_sm()
          .child(div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(agent.id.clone())))
          .child(caption(format!("{}, {}", agent.agent, agent.status), cx))
          .child(caption(plural(agent.tool_calls, "tool call", "tool calls"), cx))
          .child(div().flex_1().min_w_0().truncate().child(SharedString::from(agent.task.clone())))
      }))
      .into_any_element(),
    Body::Json { args, result } => v_flex()
      .gap_1()
      .children(args.as_ref().map(|args| json_block(id, "args", args)))
      .children(result.as_ref().map(|result| json_block(id, "result", result)))
      .into_any_element(),
  }
}

#[cfg(test)]
mod tests {
  use serde_json::Value;

  use super::{Body, body};

  /// The frames of a recorded run's tool calls: `(type, tool name, args, result or partial result)` in arrival order. omp
  /// sends the arguments with the start and the updates but not the end, so the end carries those of its call.
  fn frames(events: &str) -> Vec<(String, String, Option<Value>, Option<Value>)> {
    let events: Vec<Value> = serde_json::from_str(events).unwrap();
    let mut args_of: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    events
      .iter()
      .filter_map(|event| {
        let frame = event.get("body")?.get("frame")?;
        let kind = frame.get("type")?.as_str()?;
        if !kind.starts_with("tool_execution_") {
          return None;
        }
        let id = frame["toolCallId"].as_str().unwrap_or_default().to_owned();
        if let Some(args) = frame.get("args") {
          args_of.insert(id.clone(), args.clone());
        }
        Some((
          kind.to_owned(),
          frame["toolName"].as_str().unwrap_or_default().to_owned(),
          args_of.get(&id).cloned(),
          frame.get("result").or_else(|| frame.get("partialResult")).cloned(),
        ))
      })
      .collect()
  }

  /// The body of the finished call of a tool in a recorded run.
  fn finished(events: &str, tool: &str) -> Body {
    let (_, name, args, result) = frames(events)
      .into_iter()
      .find(|(kind, name, ..)| kind == "tool_execution_end" && name == tool)
      .unwrap_or_else(|| panic!("the run called {tool}"));
    body(&name, args.as_ref(), result.as_ref())
  }

  const TOOLS: &str = include_str!("../fixtures/tools_turn.events.json");
  const TASK: &str = include_str!("../fixtures/task_turn.events.json");

  #[test]
  fn bash_shows_the_command_and_its_output() {
    assert_eq!(
      finished(TOOLS, "bash"),
      Body::Bash {
        command: "echo alvin-bash".to_owned(),
        output: "alvin-bash\n".to_owned()
      }
    );
  }

  #[test]
  fn read_shows_the_file_text_and_write_shows_what_it_wrote() {
    assert_eq!(
      finished(TOOLS, "read"),
      Body::File {
        path: "notes.txt".to_owned(),
        text: "alpha\nbeta".to_owned()
      }
    );
    assert_eq!(
      finished(TOOLS, "write"),
      Body::File {
        path: "notes.txt".to_owned(),
        text: "alpha\nbeta\n".to_owned()
      }
    );
  }

  #[test]
  fn edit_shows_omps_diff_of_the_change() {
    let Body::Diff { patch } = finished(TOOLS, "edit") else {
      panic!("an edit with a diff renders as one");
    };
    let files = gpui_kit::component::diff::DiffFile::parse(&patch).unwrap();
    assert_eq!(
      (files[0].path().as_ref(), files[0].additions(), files[0].deletions()),
      ("notes.txt", 1, 1)
    );
    assert!(patch.lines().any(|line| line == "-beta"), "{patch}");
    assert!(patch.lines().any(|line| line == "+gamma"), "{patch}");
  }

  #[test]
  fn grep_and_glob_show_what_they_found() {
    let Body::Matches { summary, lines } = finished(TOOLS, "grep") else {
      panic!("grep renders as matches");
    };
    assert_eq!(summary, "1 match in 1 file");
    assert!(lines.iter().any(|line| line.ends_with("gamma")), "{lines:?}");

    assert_eq!(
      finished(TOOLS, "glob"),
      Body::Matches {
        summary: "1 file".to_owned(),
        lines: vec!["notes.txt".to_owned()]
      }
    );
  }

  #[test]
  fn task_shows_the_subagents_it_launched() {
    let launched = frames(TASK)
      .into_iter()
      .filter(|(kind, name, ..)| kind == "tool_execution_end" && name == "task")
      .map(|(_, name, args, result)| body(&name, args.as_ref(), result.as_ref()))
      .find_map(|body| match body {
        Body::Subagents(agents) => Some(agents),
        _ => None,
      })
      .expect("a task call that launched a subagent");
    assert_eq!(launched.len(), 1);
    assert!(!launched[0].id.is_empty() && !launched[0].agent.is_empty() && !launched[0].status.is_empty());
  }

  #[test]
  fn a_running_bash_shows_the_output_streamed_so_far() {
    let (_, name, args, partial) = frames(TOOLS)
      .into_iter()
      .find(|(kind, name, ..)| kind == "tool_execution_update" && name == "bash")
      .expect("the run streamed bash output");
    assert!(matches!(body(&name, args.as_ref(), partial.as_ref()), Body::Bash { output, .. } if output.contains("alvin-bash")));
  }

  #[test]
  fn a_tool_this_crate_does_not_know_keeps_its_arguments_and_result_as_json() {
    let args = serde_json::json!({ "anything": 1 });
    let result = serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] });

    assert_eq!(
      body("mystery", Some(&args), Some(&result)),
      Body::Json {
        args: Some(args.clone()),
        result: Some(result.clone())
      }
    );
  }

  #[test]
  fn a_known_tool_without_the_fields_its_view_needs_falls_back_to_json() {
    let args = serde_json::json!({ "path": "notes.txt" });
    let failed = serde_json::json!({ "content": [{ "type": "text", "text": "No such file" }] });

    // An edit that failed has no diff to show.
    assert_eq!(
      body("edit", Some(&args), Some(&failed)),
      Body::Json {
        args: Some(args),
        result: Some(failed)
      }
    );
  }
}
