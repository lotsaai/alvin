use serde_json::Value;

use super::{Presentation, result_text, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let result = result?;
  if result.get("content").and_then(Value::as_array).is_none() && result.pointer("/details/files").and_then(Value::as_array).is_none() {
    return None;
  }
  let title = text(args, "pattern").or_else(|| text(args, "path")).unwrap_or("Results");
  let output = result_text(result);
  let output = if output.is_empty() {
    result
      .pointer("/details/files")
      .and_then(Value::as_array)
      .map(|files| files.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"))
      .unwrap_or_default()
  } else {
    output
  };
  Some(Presentation::text(title.to_owned(), output))
}
