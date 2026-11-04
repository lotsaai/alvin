use serde_json::Value;

use super::{Presentation, result_text, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let command = text(args, "command")?;
  if result.is_some_and(|value| value.get("content").and_then(Value::as_array).is_none()) {
    return None;
  }
  let output = result.map(result_text).unwrap_or_default();
  let content = if output.is_empty() {
    format!("$ {command}")
  } else {
    format!("$ {command}\n{output}")
  };
  Some(Presentation::text(command.to_owned(), content))
}
