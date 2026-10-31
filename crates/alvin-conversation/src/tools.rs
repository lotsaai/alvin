//! Tool executions in progress and just finished, keyed by omp's tool call id.
//!
//! Arguments and results stay `serde_json::Value`, as omp's schema defines them; a view renders them per tool name.

use alvin_protocol::{ToolExecutionEndEvent, ToolExecutionStartEvent, ToolExecutionUpdateEvent, ToolStreamUpdateEvent};
use serde_json::Value;

/// How a tool execution ended.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
  /// The result omp reported.
  pub result: Option<Value>,
  /// Whether the tool failed.
  pub is_error: bool,
}

/// One tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolExecution {
  /// omp's id for the call.
  pub id: String,
  /// The tool's name.
  pub name: String,
  /// What the model said it was doing, when it said.
  pub intent: Option<String>,
  /// The arguments, as the latest event carried them.
  pub args: Option<Value>,
  /// The result so far.
  pub partial: Option<Value>,
  /// Streamed output, in arrival order.
  pub stream: Vec<Value>,
  /// Set once the execution ended.
  pub outcome: Option<ToolOutcome>,
}

/// The tool executions of the current turn, in the order they started.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tools {
  executions: Vec<ToolExecution>,
}

impl Tools {
  /// The executions, oldest first.
  pub fn executions(&self) -> &[ToolExecution] {
    &self.executions
  }

  /// The execution of a tool call.
  pub fn get(&self, id: &str) -> Option<&ToolExecution> {
    self.executions.iter().find(|execution| execution.id == id)
  }

  pub(crate) fn start(&mut self, event: &ToolExecutionStartEvent) {
    self.executions.retain(|execution| execution.id != event.tool_call_id);
    self.executions.push(ToolExecution {
      id: event.tool_call_id.clone(),
      name: event.tool_name.clone(),
      intent: event.intent.clone(),
      args: event.args.clone(),
      partial: None,
      stream: Vec::new(),
      outcome: None,
    });
  }

  pub(crate) fn update(&mut self, event: &ToolExecutionUpdateEvent) {
    if let Some(execution) = self.entry(&event.tool_call_id, &event.tool_name) {
      if event.args.is_some() {
        execution.args.clone_from(&event.args);
      }
      execution.partial.clone_from(&event.partial_result);
    }
  }

  pub(crate) fn stream_update(&mut self, event: &ToolStreamUpdateEvent) {
    if let Some(execution) = self.entry(&event.tool_call_id, &event.tool_name)
      && let Some(update) = &event.update
    {
      execution.stream.push(update.clone());
    }
  }

  pub(crate) fn end(&mut self, event: &ToolExecutionEndEvent) {
    if let Some(execution) = self.entry(&event.tool_call_id, &event.tool_name) {
      execution.outcome = Some(ToolOutcome {
        result: event.result.clone(),
        is_error: event.is_error.unwrap_or(false),
      });
    }
  }

  pub(crate) fn clear(&mut self) {
    self.executions.clear();
  }

  /// The execution of `id`, started by an earlier event. An update for a call this view never saw start (the view opened
  /// mid-turn) starts it from the update.
  fn entry(&mut self, id: &str, name: &str) -> Option<&mut ToolExecution> {
    if !self.executions.iter().any(|execution| execution.id == id) {
      self.executions.push(ToolExecution {
        id: id.to_owned(),
        name: name.to_owned(),
        intent: None,
        args: None,
        partial: None,
        stream: Vec::new(),
        outcome: None,
      });
    }
    self.executions.iter_mut().find(|execution| execution.id == id)
  }
}
