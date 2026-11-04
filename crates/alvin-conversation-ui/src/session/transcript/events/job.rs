use alvin_protocol::CustomMessage;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::{AnyElement, App, IntoElement as _, ParentElement as _, SharedString, Styled as _, div};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Job {
  pub(super) job_id: Option<String>,
  pub(super) agent_id: Option<String>,
  pub(super) label: Option<String>,
  pub(super) kind: Option<String>,
  pub(super) status: Option<String>,
  pub(super) duration_ms: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Jobs {
  pub(super) jobs: Vec<Job>,
}

pub(super) fn parse(message: &CustomMessage) -> Option<Jobs> {
  if message.custom_type.as_deref() != Some("async-result") {
    return None;
  }
  let values = message.details.as_ref()?.get("jobs")?.as_array()?;
  if values.is_empty() {
    return None;
  }
  let jobs = values
    .iter()
    .map(|value| Job {
      job_id: field(value, "jobId"),
      agent_id: field(value, "agentId"),
      label: field(value, "label"),
      kind: field(value, "type"),
      status: field(value, "status"),
      duration_ms: value.get("durationMs").and_then(Value::as_f64),
    })
    .collect();
  Some(Jobs { jobs })
}

fn field(value: &Value, key: &str) -> Option<String> {
  value
    .get(key)
    .and_then(Value::as_str)
    .filter(|value| !value.is_empty())
    .map(str::to_owned)
}

pub(super) fn view(jobs: &Jobs, cx: &mut App) -> AnyElement {
  let theme = cx.theme();
  v_flex()
    .w_full()
    .min_w_0()
    .gap_2()
    .children(jobs.jobs.iter().map(|job| {
      let name = job
        .label
        .as_deref()
        .or(job.agent_id.as_deref())
        .or(job.job_id.as_deref())
        .unwrap_or("Background job");
      let mut metadata = Vec::new();
      if let Some(kind) = &job.kind {
        metadata.push(kind.clone());
      }
      if let Some(status) = &job.status {
        metadata.push(status.clone());
      }
      if let Some(duration) = job.duration_ms {
        metadata.push(format!("{duration} ms"));
      }
      v_flex().gap_1().min_w_0().child(SharedString::from(name.to_owned())).child(
        div()
          .text_sm()
          .text_color(theme.muted_foreground)
          .child(SharedString::from(metadata.join(" · "))),
      )
    }))
    .into_any_element()
}

#[cfg(test)]
mod tests {
  use super::parse;
  use alvin_protocol::AgentMessage;
  use serde_json::json;

  #[test]
  fn recorded_delivery_preserves_job_and_result() {
    let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/task_turn.messages.json")).unwrap();
    let message: AgentMessage = serde_json::from_value(recording["messages"][7].clone()).unwrap();
    let AgentMessage::Custom(message) = message else {
      panic!("custom message expected")
    };
    let parsed = parse(&message).unwrap();
    assert_eq!(parsed.jobs[0].job_id.as_deref(), Some("BraveChipmunk"));
    assert_eq!(parsed.jobs[0].status.as_deref(), Some("completed"));
    assert_eq!(parsed.jobs[0].duration_ms, Some(3706.0));
  }

  #[test]
  fn delivery_keeps_failure_cancellation_and_unknown_status() {
    let message: AgentMessage = serde_json::from_value(json!({"role":"custom","customType":"async-result","content":"all results", "details":{"jobs":[{"jobId":"a","status":"failed"},{"jobId":"b","status":"cancelled"},{"status":"waiting-for-review"}]}})).unwrap();
    let AgentMessage::Custom(message) = message else {
      panic!("custom message expected")
    };
    let parsed = parse(&message).unwrap();
    assert_eq!(
      parsed.jobs.iter().map(|job| job.status.as_deref()).collect::<Vec<_>>(),
      [Some("failed"), Some("cancelled"), Some("waiting-for-review")]
    );
    assert_eq!(parsed.jobs[2].duration_ms, None);
  }
}
