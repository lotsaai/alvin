use serde_json::Value;

use super::{Presentation, result_text, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let path = text(args, "path")?;
  let result = result?;
  if result.pointer("/details/displayContent/text").and_then(Value::as_str).is_none()
    && result.get("content").and_then(Value::as_array).is_none()
  {
    return None;
  }
  let content = result
    .pointer("/details/displayContent/text")
    .and_then(Value::as_str)
    .map_or_else(|| result_text(result), str::to_owned);
  Some(Presentation::file(path.to_owned(), content))
}
