use alvin_protocol::BashExecutionMessage;
use serde_json::json;

use super::super::tool::ToolInput;

pub(in crate::session::transcript) fn input(message: &BashExecutionMessage) -> ToolInput {
  let mut content = Vec::new();
  if let Some(output) = &message.output {
    content.push(json!({"type":"text","text":output}));
  }
  for image in message.images.iter().flatten() {
    let image = match serde_json::to_value(image) {
      Ok(mut value) => {
        if let Some(object) = value.as_object_mut() {
          object.insert("type".into(), json!("image"));
        }
        value
      }
      Err(error) => json!({"type":"text","text":format!("Image decoding failed: {error}")}),
    };
    content.push(image);
  }
  ToolInput {
    name: "bash".into(),
    intent: None,
    args: message.command.as_ref().map(|command| json!({"command": command})),
    result: (!content.is_empty()).then(|| json!({"content":content})),
    stream: Vec::new(),
    running: false,
    failed: message.exit_code.is_some_and(|code| code != 0),
    exit_code: message.exit_code,
    timestamp: message.timestamp,
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{BashExecutionMessage, ImageContent};

  use super::input;

  #[test]
  fn command_output_keeps_reported_images_and_exit_code() {
    let message = BashExecutionMessage {
      command: Some("show".into()),
      output: Some("output".into()),
      exit_code: Some(7),
      images: Some(vec![ImageContent {
        data: Some("aW1hZ2U=".into()),
        mime_type: Some("image/png".into()),
        ..ImageContent::default()
      }]),
      ..BashExecutionMessage::default()
    };
    let input = input(&message);
    assert_eq!(input.exit_code, Some(7));
    let content = input.result.unwrap();
    assert_eq!(content["content"][0]["text"], "output");
    assert_eq!(content["content"][1]["type"], "image");
    assert_eq!(content["content"][1]["data"], "aW1hZ2U=");
  }
}
