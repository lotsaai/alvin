use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, InteractiveElement as _, IntoElement as _, ParentElement as _, Role, SharedString, StatefulInteractiveElement as _,
  Styled as _, div, prelude::FluentBuilder as _,
};
use serde_json::Value;

use super::{Body, Presentation, Summary, result_text, text};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Agent {
  id: Option<String>,
  kind: Option<String>,
  status: Option<String>,
  assignment: Option<String>,
  calls: Option<i64>,
  duration_ms: Option<f64>,
  recent_tools: Vec<RecentTool>,
  recent_output: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
struct RecentTool {
  name: Option<String>,
  args: Option<String>,
  intent: Option<String>,
  failed: Option<bool>,
}
pub(super) fn parse(args: Option<&Value>, result: Option<&Value>, progress_snapshot: Option<&Value>) -> Option<Presentation> {
  let result = result?;
  let source = if result.pointer("/details/async/state").and_then(Value::as_str) == Some("running") {
    progress_snapshot.unwrap_or(result)
  } else {
    result
  };
  let progress = source.pointer("/details/progress")?.as_array()?;
  if progress.is_empty() {
    return None;
  }
  let mut agents: Vec<Agent> = Vec::new();
  for value in progress {
    let agent = Agent {
      id: text(Some(value), "id").filter(|id| !id.is_empty()).map(str::to_owned),
      kind: text(Some(value), "agent").map(str::to_owned),
      status: text(Some(value), "status").map(str::to_owned),
      assignment: text(Some(value), "assignment").map(str::to_owned),
      calls: value.get("toolCount").and_then(Value::as_i64),
      duration_ms: value.get("durationMs").and_then(Value::as_f64),
      recent_tools: value
        .get("recentTools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|tool| RecentTool {
          name: text(Some(tool), "tool").map(str::to_owned),
          args: text(Some(tool), "args").map(str::to_owned),
          intent: text(Some(tool), "intent").map(str::to_owned),
          failed: tool.get("isError").and_then(Value::as_bool),
        })
        .collect(),
      recent_output: strings(value.get("recentOutput")),
    };
    if let Some(existing) = agent
      .id
      .as_ref()
      .and_then(|id| agents.iter_mut().find(|existing| existing.id.as_ref() == Some(id)))
    {
      *existing = agent;
    } else {
      agents.push(agent);
    }
  }
  Some(Presentation {
    summary: Summary::Task(text(args, "task").map(str::to_owned)),
    body: Body::Task {
      agents,
      output: result_text(result),
    },
  })
}

pub(super) fn settle(agents: &mut [Agent], output: &mut String, result: &Value, delivery: &Value) {
  let Some(job_id) = result
    .pointer("/details/async/jobId")
    .and_then(Value::as_str)
    .filter(|id| !id.is_empty())
  else {
    return;
  };
  let Some(jobs) = delivery.pointer("/details/jobs").and_then(Value::as_array) else {
    return;
  };
  let wait = delivery.get("toolName").and_then(Value::as_str) == Some("wait");
  let (job_key, agent_key) = if wait { ("id", "agentUrlId") } else { ("jobId", "agentId") };
  let mut matching = jobs.iter().filter(|job| job.get(job_key).and_then(Value::as_str) == Some(job_id));
  let Some(job) = matching.next() else { return };
  if matching.next().is_some() {
    return;
  }
  let Some(agent_id) = job.get(agent_key).and_then(Value::as_str).filter(|id| !id.is_empty()) else {
    return;
  };
  let Some(status) = job.get("status").and_then(Value::as_str) else {
    return;
  };
  if let Some(agent) = agents.iter_mut().find(|agent| agent.id.as_deref() == Some(agent_id)) {
    agent.status = Some(status.to_owned());
    if let Some(duration) = job.get("durationMs").and_then(Value::as_f64) {
      agent.duration_ms = Some(duration);
    }
    let delivered = if wait {
      job.get("resultText").and_then(Value::as_str)
    } else if jobs.len() == 1 {
      // Custom deliveries carry one shared body, not per-job output.
      delivery.get("content").and_then(Value::as_str)
    } else {
      None
    };
    if let Some(delivered) = delivered {
      delivered.clone_into(output);
    }
  }
}

fn strings(value: Option<&Value>) -> Vec<String> {
  value
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::to_owned)
    .collect()
}

pub(super) fn view(agents: &[Agent], cx: &App) -> AnyElement {
  let theme = cx.theme();
  v_flex()
    .w_full()
    .min_w_0()
    .gap_2()
    .children(agents.iter().enumerate().map(|(index, agent)| {
      let mut metadata = Vec::new();
      if let Some(kind) = &agent.kind {
        metadata.push(kind.clone());
      }
      if let Some(status) = &agent.status {
        metadata.push(status.clone());
      }
      if let Some(calls) = agent.calls {
        metadata.push(format!("{calls} tool calls"));
      }
      if let Some(duration) = agent.duration_ms {
        metadata.push(format!("{duration} ms"));
      }
      let metadata = SharedString::from(metadata.join(" · "));
      let name = SharedString::from(agent.id.as_deref().unwrap_or("Agent").to_owned());
      v_flex()
        .id(("task-agent", index))
        .role(Role::Group)
        .aria_label(name.clone())
        .min_w_0()
        .gap_1()
        .child(
          h_flex().gap_2().child(name).child(
            div()
              .id("metadata")
              .role(Role::Status)
              .aria_label(metadata.clone())
              .text_sm()
              .text_color(theme.muted_foreground)
              .child(metadata),
          ),
        )
        .when_some(agent.assignment.as_ref(), |row, assignment| {
          row.child(SharedString::from(assignment.clone()))
        })
        .children(agent.recent_tools.iter().map(|tool| {
          let mut parts = Vec::new();
          if let Some(name) = &tool.name {
            parts.push(name.clone());
          }
          if let Some(args) = &tool.args {
            parts.push(args.clone());
          }
          if let Some(intent) = &tool.intent {
            parts.push(intent.clone());
          }
          if tool.failed == Some(true) {
            parts.push("Failed".to_owned());
          }
          div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(SharedString::from(parts.join(" · ")))
        }))
        .children(agent.recent_output.iter().map(|line| {
          div()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(SharedString::from(line.clone()))
        }))
    }))
    .into_any_element()
}

#[cfg(test)]
mod tests {
  use super::{parse, settle};
  use crate::session::transcript::tool::Body;
  use serde_json::json;

  #[test]
  fn progress_updates_matching_agent_without_merging_missing_ids() {
    let result = json!({"content":[{"text":"complete result"}],"details":{"progress":[
      {"id":"one","status":"running","assignment":"work"},
      {"id":"one","status":"completed","assignment":"work","toolCount":2},
      {"status":"failed"},{"status":"cancelled"},{"status":"unrecognized"}
    ]}});
    let Body::Task { agents, output } = parse(None, Some(&result), None).unwrap().body else {
      panic!("task expected")
    };
    assert_eq!(agents.len(), 4);
    assert_eq!(agents[0].status.as_deref(), Some("completed"));
    assert_eq!(agents[0].calls, Some(2));
    assert_eq!(agents[1].status.as_deref(), Some("failed"));
    assert_eq!(agents[2].status.as_deref(), Some("cancelled"));
    assert_eq!(agents[3].status.as_deref(), Some("unrecognized"));
    assert_eq!(output, "complete result");
  }

  #[test]
  fn recorded_delivery_settles_only_matching_operation() {
    let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/task_turn.messages.json")).unwrap();
    let result = &recording["messages"][4];
    let delivery = &recording["messages"][7];
    let Body::Task { mut agents, mut output } = parse(None, Some(result), None).unwrap().body else {
      panic!("task expected")
    };
    assert_eq!(agents[0].status.as_deref(), Some("pending"));
    let unrelated = json!({"details":{"jobs":[{"jobId":"other","agentId":"BraveChipmunk","status":"failed"}]}});
    settle(&mut agents, &mut output, result, &unrelated);
    assert_eq!(agents[0].status.as_deref(), Some("pending"));
    assert_eq!(output, super::result_text(result));
    settle(&mut agents, &mut output, result, delivery);
    assert_eq!(agents[0].status.as_deref(), Some("completed"));
    assert_eq!(agents[0].duration_ms, Some(3706.0));
    assert_eq!(output, delivery["content"].as_str().unwrap());
  }

  #[test]
  fn recorded_direct_wait_settles_only_the_reported_agent() {
    let recording: serde_json::Value = serde_json::from_str(include_str!("../../../../fixtures/real_task_wait.messages.json")).unwrap();
    let messages = recording["messages"].as_array().unwrap();
    let result = messages.iter().find(|message| message["toolName"] == "task").unwrap();
    let delivery = messages.iter().find(|message| message["toolName"] == "wait").unwrap();
    let Body::Task { mut agents, mut output } = parse(None, Some(result), None).unwrap().body else {
      panic!("task expected")
    };
    let mut unrelated = delivery.clone();
    unrelated["details"]["jobs"][0]["agentUrlId"] = json!("unrelated");
    settle(&mut agents, &mut output, result, &unrelated);
    assert_eq!(agents[0].status.as_deref(), Some("pending"));
    assert_eq!(output, super::result_text(result));
    settle(&mut agents, &mut output, result, delivery);
    assert_eq!(agents[0].status.as_deref(), Some("completed"));
    assert_eq!(agents[0].duration_ms, Some(14534.0));
    assert_eq!(output, delivery["details"]["jobs"][0]["resultText"].as_str().unwrap());
  }

  #[test]
  fn delivery_status_remains_reported_and_ambiguous_jobs_do_not_settle() {
    let result = json!({"details":{"async":{"jobId":"job"},"progress":[{"id":"agent","status":"running"}]}});
    let Body::Task { mut agents, mut output } = parse(None, Some(&result), None).unwrap().body else {
      panic!("task expected")
    };
    for status in ["failed", "cancelled", "waiting"] {
      let delivery = json!({"details":{"jobs":[{"jobId":"job","agentId":"agent","status":status}]}});
      settle(&mut agents, &mut output, &result, &delivery);
      assert_eq!(agents[0].status.as_deref(), Some(status));
    }
    let ambiguous = json!({"details":{"jobs":[{"jobId":"job","agentId":"agent","status":"completed"},{"jobId":"job","agentId":"agent","status":"failed"}]}});
    settle(&mut agents, &mut output, &result, &ambiguous);
    assert_eq!(agents[0].status.as_deref(), Some("waiting"));
  }

  #[test]
  fn shared_batch_output_is_not_attributed_to_one_job() {
    let result = json!({
      "content": [{"type":"text","text":"original operation"}],
      "details":{"async":{"jobId":"job"},"progress":[{"id":"agent","status":"running"}]}
    });
    let Body::Task { mut agents, mut output } = parse(None, Some(&result), None).unwrap().body else {
      panic!("task expected")
    };
    let delivery = json!({
      "content":"shared results for both jobs",
      "details":{"jobs":[
        {"jobId":"job","agentId":"agent","status":"completed"},
        {"jobId":"other","agentId":"other-agent","status":"failed"}
      ]}
    });
    settle(&mut agents, &mut output, &result, &delivery);
    assert_eq!(agents[0].status.as_deref(), Some("completed"));
    assert_eq!(output, "original operation");
  }

  #[test]
  fn recorded_background_progress_supersedes_spawn_but_not_completed_response() {
    let events: Vec<serde_json::Value> = serde_json::from_str(include_str!("../../../../fixtures/real_task_progress.events.json")).unwrap();
    let result = events
      .iter()
      .filter_map(|event| event.pointer("/body/frame"))
      .find(|frame| frame["type"] == "tool_execution_end" && frame["toolName"] == "task")
      .unwrap()
      .get("result")
      .unwrap();
    let latest = events
      .iter()
      .rev()
      .filter_map(|event| event.pointer("/body/frame"))
      .find(|frame| frame["type"] == "tool_execution_update" && frame["toolName"] == "task")
      .unwrap()
      .get("partialResult")
      .unwrap();
    let Body::Task { agents, output } = parse(None, Some(result), Some(latest)).unwrap().body else {
      panic!("task expected")
    };
    let agent = agents
      .iter()
      .find(|agent| agent.id.as_deref() == Some("native-task-proof"))
      .unwrap();
    assert_eq!(agent.status.as_deref(), Some("completed"));
    assert_eq!(agent.calls, Some(2));
    assert!(
      agent
        .recent_tools
        .iter()
        .any(|tool| tool.name.as_deref() == Some("read") && tool.args.as_deref() == Some("skill://write-rust"))
    );
    assert_eq!(output, super::result_text(result));

    let earlier = events
      .iter()
      .filter_map(|event| event.pointer("/body/frame"))
      .find(|frame| frame["type"] == "tool_execution_update" && frame["toolName"] == "task")
      .unwrap()
      .get("partialResult")
      .unwrap();
    let Body::Task { agents, .. } = parse(None, Some(latest), Some(earlier)).unwrap().body else {
      panic!("task expected")
    };
    let agent = agents
      .iter()
      .find(|agent| agent.id.as_deref() == Some("native-task-proof"))
      .unwrap();
    assert_eq!(agent.status.as_deref(), Some("completed"));
    assert_eq!(agent.calls, Some(2));
  }
}
