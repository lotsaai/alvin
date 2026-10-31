//! Pure translation from `omp-rpc` events to protocol event bodies.
//!
//! `omp-rpc` and `alvin-protocol` both generate their wire types from omp's schema, so the types match in shape but are
//! distinct Rust types: `alvin-protocol` must not depend on `omp-rpc`. A value crosses by way of its JSON form.

use alvin_protocol::EventBody;
use omp_rpc::{Event, RpcNotification};
use serde::Serialize;
use serde::de::Deserialize;
use serde_json::Value;

/// A frame omp sent that the protocol does not expose.
#[derive(Debug, Clone, PartialEq)]
pub struct Untranslated {
  /// The frame as omp sent it.
  pub frame: Value,
}

impl Untranslated {
  /// The frame's `type`, for logs.
  pub fn kind(&self) -> &str {
    self.frame.get("type").and_then(Value::as_str).unwrap_or("unknown")
  }
}

/// The outcome of translating one event.
#[derive(Debug, Clone, PartialEq)]
pub enum Translation {
  /// A protocol event body.
  Event(EventBody),
  /// A frame the protocol does not expose.
  Untranslated(Untranslated),
}

/// Translates one `omp-rpc` event.
pub fn translate(event: Event) -> Translation {
  match event {
    Event::Notification(notification) => notification_body(notification),
    Event::UnmatchedResponse(response) => untranslated(&response),
    Event::Undecodable { raw, error } => Translation::Event(EventBody::Undecodable { raw, error }),
  }
}

fn notification_body(notification: RpcNotification) -> Translation {
  match notification {
    RpcNotification::RpcAgentEvent(member) => carry(&member, EventBody::Agent),
    RpcNotification::PromptResult(member) => carry(&member, EventBody::PromptResult),
    RpcNotification::SessionSettled(member) => carry(&member, EventBody::SessionSettled),
    RpcNotification::ExtensionUiRequest(member) => carry(&member, EventBody::ExtensionUiRequest),
    RpcNotification::ExtensionError(member) => carry(&member, EventBody::ExtensionError),
    RpcNotification::AvailableCommandsUpdate(member) => carry(&member, EventBody::AvailableCommandsUpdate),
    RpcNotification::CommandOutput(member) => carry(&member, EventBody::CommandOutput),
    RpcNotification::SessionInfoUpdate(member) => carry(&member, EventBody::SessionInfoUpdate),
    RpcNotification::ConfigUpdate(member) => carry(&member, |member| EventBody::ConfigUpdate(Box::new(member))),
    RpcNotification::RpcFrameError(member) => carry(&member, EventBody::RpcFrameError),
    RpcNotification::Unknown(frame) => Translation::Untranslated(Untranslated { frame }),
    // Startup handshake (consumed by the client), subagents, live voice, dictation and btw: not in the protocol.
    other => untranslated(&other),
  }
}

/// Moves `member` into the protocol's type of the same name and wraps it. A value that does not convert becomes an
/// `Undecodable` body carrying its JSON, so it is never silently lost.
fn carry<T: Serialize, U: for<'de> Deserialize<'de>>(member: &T, wrap: fn(U) -> EventBody) -> Translation {
  match serde_json::to_value(member) {
    Ok(frame) => match U::deserialize(&frame) {
      Ok(converted) => Translation::Event(wrap(converted)),
      Err(error) => Translation::Event(EventBody::Undecodable {
        raw: frame,
        error: error.to_string(),
      }),
    },
    Err(error) => Translation::Event(EventBody::Undecodable {
      raw: Value::Null,
      error: error.to_string(),
    }),
  }
}

fn untranslated<T: Serialize>(member: &T) -> Translation {
  match serde_json::to_value(member) {
    Ok(frame) => Translation::Untranslated(Untranslated { frame }),
    Err(error) => Translation::Event(EventBody::Undecodable {
      raw: Value::Null,
      error: error.to_string(),
    }),
  }
}

/// Converts a value between the two generated type families.
pub(crate) fn convert<T: Serialize, U: for<'de> Deserialize<'de>>(what: &'static str, value: &T) -> crate::Result<U> {
  let json = serde_json::to_value(value).map_err(|source| crate::Error::Convert { what, source })?;
  U::deserialize(&json).map_err(|source| crate::Error::Convert { what, source })
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{AssistantMessageEvent, PromptStatus, RpcAgentEvent};
  use omp_rpc::RpcServerFrame;

  use super::*;

  /// The notifications of one real omp run, in the order omp sent them.
  fn recorded() -> Vec<Event> {
    include_str!("../fixtures/frames.jsonl")
      .lines()
      .filter_map(|line| {
        let frame: Value = serde_json::from_str(line).unwrap();
        match RpcServerFrame::deserialize(&frame).unwrap() {
          RpcServerFrame::RpcNotification(notification) => Some(Event::Notification(notification)),
          RpcServerFrame::Response(_) => None,
          other => panic!("unexpected frame {other:?}"),
        }
      })
      .collect()
  }

  #[test]
  fn every_recorded_frame_translates_except_the_startup_handshake() {
    let mut untranslated = Vec::new();
    let mut bodies = Vec::new();
    for event in recorded() {
      match translate(event) {
        Translation::Event(body) => bodies.push(body),
        Translation::Untranslated(frame) => untranslated.push(frame.kind().to_owned()),
      }
    }

    assert_eq!(untranslated, ["ready"]);
    assert!(
      bodies.iter().all(|body| !matches!(body, EventBody::Undecodable { .. })),
      "{bodies:?}"
    );
    assert_eq!(bodies.len(), recorded().len() - 1);
  }

  #[test]
  fn a_text_delta_keeps_its_text_and_a_prompt_result_its_status() {
    let bodies: Vec<EventBody> = recorded()
      .into_iter()
      .filter_map(|event| match translate(event) {
        Translation::Event(body) => Some(body),
        Translation::Untranslated(_) => None,
      })
      .collect();

    let delta = bodies.iter().find_map(|body| match body {
      EventBody::Agent(RpcAgentEvent::MessageUpdate(update)) => match &update.assistant_message_event {
        AssistantMessageEvent::TextDelta(delta) => delta.delta.clone(),
        _ => None,
      },
      _ => None,
    });
    assert_eq!(delta.as_deref(), Some("ok"));
    assert!(matches!(bodies.last(), Some(EventBody::PromptResult(result)) if result.status == PromptStatus::Completed));
  }

  #[test]
  fn a_frame_that_failed_to_decode_becomes_an_event_naming_the_failure() {
    let raw = serde_json::json!({ "type": "turn_end" });
    let translated = translate(Event::Undecodable {
      raw: raw.clone(),
      error: "missing field `message`".to_owned(),
    });

    assert_eq!(
      translated,
      Translation::Event(EventBody::Undecodable {
        raw,
        error: "missing field `message`".to_owned(),
      })
    );
  }

  #[test]
  fn an_unknown_frame_is_untranslated_with_its_raw_json() {
    let frame = serde_json::json!({ "type": "from_the_future", "n": 1 });

    let translated = translate(Event::Notification(RpcNotification::Unknown(frame.clone())));

    assert_eq!(translated, Translation::Untranslated(Untranslated { frame }));
  }
}
