use alvin_protocol::PythonExecutionMessage;
use serde_json::json;

use super::super::tool::ToolInput;

pub(in crate::session::transcript) fn input(message: &PythonExecutionMessage) -> ToolInput {
  ToolInput {
    name: "eval".into(),
    task_progress: None,
    intent: None,
    args: message.code.as_ref().map(|code| json!({"code": code, "language": "python"})),
    result: message
      .output
      .as_ref()
      .map(|output| json!({"content": [{"type": "text", "text": output}]})),
    delivery: None,
    stream: Vec::new(),
    running: false,
    failed: message.cancelled.unwrap_or(false) || message.exit_code.is_some_and(|code| code != 0),
    exit_code: message.exit_code,
    timestamp: message.timestamp,
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::PythonExecutionMessage;

  use super::input;

  #[test]
  fn python_execution_preserves_source_output_and_failure() {
    let message = PythonExecutionMessage {
      code: Some("print('hi')".into()),
      output: Some("hi\n".into()),
      exit_code: Some(1),
      ..PythonExecutionMessage::default()
    };
    let tool = input(&message);
    assert_eq!(tool.args.unwrap(), serde_json::json!({"code":"print('hi')", "language":"python"}));
    assert_eq!(tool.result.unwrap()["content"][0]["text"], "hi\n");
    assert!(tool.failed);
    assert_eq!(tool.exit_code, Some(1));
  }
}
