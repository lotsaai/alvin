use serde_json::Value;

use super::{Presentation, result_text, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let result = result?;
  let progress = result.pointer("/details/progress")?.as_array()?;
  if progress.is_empty() {
    return None;
  }
  let mut output = String::new();
  for agent in progress {
    if !output.is_empty() {
      output.push('\n');
    }
    let id = text(Some(agent), "id").unwrap_or_default();
    let kind = text(Some(agent), "agent").unwrap_or_default();
    let status = text(Some(agent), "status").unwrap_or_default();
    let assignment = text(Some(agent), "assignment").unwrap_or_default();
    let calls = agent.get("toolCount").and_then(Value::as_i64);
    output.push_str(&format!("{id} {kind} {status}"));
    if let Some(calls) = calls {
      output.push_str(&format!(" · {calls} tool calls"));
    }
    output.push_str(&format!("\n{assignment}"));
  }
  let additional = result_text(result);
  if !additional.is_empty() {
    output.push('\n');
    output.push_str(&additional);
  }
  Some(Presentation::text(text(args, "task").unwrap_or("Subagents").to_owned(), output))
}
