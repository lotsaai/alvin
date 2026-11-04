use alvin_protocol::ToolResultMessage;
use serde_json::json;

use super::super::tool::ToolInput;

pub(in crate::session::transcript) fn input(message: &ToolResultMessage) -> ToolInput {
  let result = match serde_json::to_value(message) {
    Ok(value) => value,
    Err(error) => json!({"content":[{"type":"text","text":format!("Tool result decoding failed: {error}")}]}),
  };
  ToolInput {
    name: message.tool_name.clone().unwrap_or_else(|| "tool".into()),
    intent: None,
    args: None,
    result: Some(result),
    stream: Vec::new(),
    running: false,
    failed: message.is_error.unwrap_or(false),
    exit_code: None,
    timestamp: message.timestamp,
  }
}
