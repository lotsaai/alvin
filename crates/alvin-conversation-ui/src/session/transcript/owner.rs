use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use alvin_conversation::LiveView;
use alvin_protocol::{AgentMessage, AssistantContent};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::message_scroller::MessageScrollerState;
use gpui_kit::{AppContext as _, Context, Entity, Subscription, Window};
use serde_json::{Value, json};

use super::events::{AssistantBlock, Block, UserBlock, bash_input, python_input, result_input};
use super::tool::{ToolBlock, ToolInput};
use super::{ToolResults, hidden, message_at, message_id, synthetic_directive};

struct Slot {
  source: Rc<AgentMessage>,
  body: Option<Rc<Block>>,
  tools: Rc<HashMap<usize, ToolInput>>,
  streaming: bool,
  _subscription: Option<Subscription>,
}

type BlockSnapshot = Vec<(Option<Rc<Block>>, bool)>;

#[derive(Default)]
pub(in crate::session) struct Transcript {
  generation: Option<u64>,
  slots: Vec<Slot>,
  blocks: Rc<BlockSnapshot>,
  process_open: Rc<RefCell<HashMap<usize, bool>>>,
}

pub(in crate::session) struct Prepared {
  generation: u64,
  changed: bool,
  entries: Vec<Entry>,
}

struct Entry {
  id: gpui_kit::SharedString,
  source: Option<Rc<AgentMessage>>,
  tools: Rc<HashMap<usize, ToolInput>>,
  streaming: bool,
  matched: bool,
}
fn task_delivery(view: &LiveView, result: &Value) -> Option<Value> {
  let job = result.pointer("/details/async/jobId")?.as_str().filter(|job| !job.is_empty())?;
  let count = view
    .transcript
    .iter()
    .filter(|message| match message {
      AgentMessage::ToolResult(result) => {
        result
          .details
          .as_ref()
          .and_then(|details| details.pointer("/async/jobId"))
          .and_then(Value::as_str)
          == Some(job)
      }
      _ => false,
    })
    .count();
  if count != 1 {
    return None;
  }
  let mut deliveries = view.transcript.iter().filter_map(|message| {
    let (details, key) = match message {
      AgentMessage::Custom(message) if message.custom_type.as_deref() == Some("async-result") => (message.details.as_ref()?, "jobId"),
      AgentMessage::ToolResult(result) if result.tool_name.as_deref() == Some("wait") => {
        let details = result.details.as_ref()?;
        if details.get("op").and_then(Value::as_str) != Some("wait") {
          return None;
        }
        (details, "id")
      }
      _ => return None,
    };
    if !details
      .get("jobs")?
      .as_array()?
      .iter()
      .any(|entry| entry.get(key).and_then(Value::as_str) == Some(job))
    {
      return None;
    }
    match serde_json::to_value(message) {
      Ok(value) => Some(value),
      Err(error) => {
        tracing::warn!(%error, "could not serialize job delivery");
        None
      }
    }
  });
  let delivery = deliveries.next()?;
  deliveries.next().is_none().then_some(delivery)
}

impl Prepared {
  pub(in crate::session) fn changed(&self) -> bool {
    self.changed
  }
}

impl Transcript {
  pub(in crate::session) fn prepare(&self, view: &LiveView) -> Prepared {
    let results = ToolResults::new(view);
    let mut changed = self.generation != Some(view.snapshot_generation)
      || self.slots.len() != view.transcript.len() + usize::from(view.streaming.is_some());
    let current_turn = view
      .transcript
      .iter()
      .rposition(|message| matches!(message, AgentMessage::User(_)))
      .map_or(0, |index| index.saturating_add(1));
    let mut counts = HashMap::<&str, usize>::new();
    for index in current_turn..view.transcript.len() + usize::from(view.streaming.is_some()) {
      if let Some((AgentMessage::Assistant(message), _)) = message_at(view, index) {
        for part in message.content.iter().flatten() {
          if let AssistantContent::ToolCall(call) = part
            && let Some(id) = call.id.as_deref().filter(|id| !id.is_empty())
          {
            *counts.entry(id).or_default() += 1;
          }
        }
      }
    }
    let mut entries = Vec::new();
    for index in 0..view.transcript.len() + usize::from(view.streaming.is_some()) {
      let Some((message, streaming)) = message_at(view, index) else {
        continue;
      };
      let source =
        if self.generation == Some(view.snapshot_generation) && self.slots.get(index).is_some_and(|slot| slot.source.as_ref() == message) {
          None
        } else {
          Some(Rc::new(message.clone()))
        };
      let cached = self.slots.get(index).filter(|_| source.is_none()).filter(|_| {
        if let AgentMessage::Assistant(assistant) = message {
          assistant.content.iter().flatten().enumerate().all(|(position, part)| {
            let AssistantContent::ToolCall(call) = part else { return true };
            if call.name.as_deref() == Some("task") {
              return false;
            }
            if index >= current_turn && call.id.as_deref().and_then(|id| view.tools.get(id)).is_some() {
              return false;
            }
            results
              .calls
              .get(&(index, position))
              .and_then(|result_index| self.slots.get(*result_index).zip(message_at(view, *result_index)))
              .is_some_and(|(slot, (result, _))| slot.source.as_ref() == result)
          })
        } else {
          true
        }
      });
      let mut tools = HashMap::new();
      if cached.is_none()
        && let AgentMessage::Assistant(assistant) = message
      {
        for (position, part) in assistant.content.iter().flatten().enumerate() {
          if let AssistantContent::ToolCall(call) = part {
            let live = call
              .id
              .as_deref()
              .filter(|id| index >= current_turn && counts.get(id) == Some(&1))
              .and_then(|id| view.tools.get(id));
            let committed = results
              .calls
              .get(&(index, position))
              .and_then(|result| message_at(view, *result))
              .and_then(|(result, _)| match result {
                AgentMessage::ToolResult(result) => Some(result),
                _ => None,
              });
            let result = if let Some(result) = committed {
              Some(match serde_json::to_value(result) {
                Ok(value) => value,
                Err(error) => json!({"content":[{"type":"text","text":format!("Tool result decoding failed: {error}")}]}),
              })
            } else {
              live
                .and_then(|tool| {
                  tool
                    .outcome
                    .as_ref()
                    .and_then(|outcome| outcome.result.as_ref())
                    .or(tool.partial.as_ref())
                })
                .cloned()
            };
            tools.insert(
              position,
              ToolInput {
                name: call
                  .name
                  .clone()
                  .or_else(|| live.map(|tool| tool.name.clone()))
                  .unwrap_or_else(|| "tool".into()),
                task_progress: live
                  .filter(|tool| {
                    tool.name == "task" && (committed.is_some() || tool.outcome.as_ref().is_some_and(|outcome| outcome.result.is_some()))
                  })
                  .and_then(|tool| tool.partial.clone()),
                intent: call.intent.clone().or_else(|| live.and_then(|tool| tool.intent.clone())),
                args: live
                  .and_then(|tool| tool.args.clone())
                  .or_else(|| call.arguments.clone().map(Value::Object)),
                delivery: result.as_ref().and_then(|result| task_delivery(view, result)),
                result,
                stream: live.map(|tool| tool.stream.clone()).unwrap_or_default(),
                running: committed.is_none() && live.is_some_and(|tool| tool.outcome.is_none()),
                failed: committed.map_or_else(
                  || live.and_then(|tool| tool.outcome.as_ref()).is_some_and(|outcome| outcome.is_error),
                  |result| result.is_error.unwrap_or(false),
                ),
                exit_code: None,
                timestamp: committed.and_then(|result| result.timestamp),
              },
            );
          }
        }
      }
      let tools = if let Some(slot) = cached {
        slot.tools.clone()
      } else if let Some(slot) = self.slots.get(index).filter(|slot| slot.tools.as_ref() == &tools) {
        slot.tools.clone()
      } else {
        Rc::new(tools)
      };
      let matched = results.results.contains_key(&index) || hidden(message);
      changed |= self.slots.get(index).is_none_or(|slot| {
        source.is_some() || slot.streaming != streaming || slot.body.is_none() != matched || !Rc::ptr_eq(&slot.tools, &tools)
      });
      entries.push(Entry {
        id: message_id(view, index),
        source,
        tools,
        streaming,
        matched,
      });
    }
    Prepared {
      generation: view.snapshot_generation,
      changed,
      entries,
    }
  }

  pub(in crate::session) fn apply<T: 'static>(
    &mut self,
    prepared: Prepared,
    composer: &Entity<EditorState>,
    scroller: &Entity<MessageScrollerState>,
    window: &mut Window,
    cx: &mut Context<T>,
  ) {
    if self.generation != Some(prepared.generation) {
      self.slots.clear();
      self.process_open.borrow_mut().clear();
      self.generation = Some(prepared.generation);
    }
    self.slots.truncate(prepared.entries.len());
    for (index, entry) in prepared.entries.into_iter().enumerate() {
      let source_changed = entry.source.is_some();
      if let Some(slot) = self.slots.get_mut(index) {
        if let Some(source) = entry.source {
          if std::mem::discriminant(slot.source.as_ref()) != std::mem::discriminant(source.as_ref())
            || synthetic_directive(slot.source.as_ref()) != synthetic_directive(source.as_ref())
          {
            *slot = Self::create(entry.id.clone(), source, entry.matched, composer, scroller, cx);
          } else {
            slot.source = source;
          }
        }
        if entry.matched {
          slot.body = None;
          slot._subscription = None;
        } else if slot.body.is_none() {
          *slot = Self::create(entry.id, slot.source.clone(), false, composer, scroller, cx);
        }
        slot.streaming = entry.streaming;
        slot.tools = entry.tools.clone();
        Self::sync_slot(slot, &entry.tools, entry.streaming, source_changed, window, cx);
      } else if let Some(source) = entry.source {
        let mut slot = Self::create(entry.id, source, entry.matched, composer, scroller, cx);
        slot.streaming = entry.streaming;
        slot.tools = entry.tools.clone();
        Self::sync_slot(&mut slot, &entry.tools, entry.streaming, false, window, cx);
        self.slots.push(slot);
      }
    }
    let changed = self.blocks.len() != self.slots.len()
      || self.blocks.iter().zip(&self.slots).any(|((body, streaming), slot)| {
        *streaming != slot.streaming
          || match (body.as_ref(), slot.body.as_ref()) {
            (Some(old), Some(new)) => !Rc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
          }
      });
    if changed {
      self.blocks = Rc::new(self.slots.iter().map(|slot| (slot.body.clone(), slot.streaming)).collect());
    }
  }

  fn create<T: 'static>(
    id: gpui_kit::SharedString,
    source: Rc<AgentMessage>,
    matched: bool,
    composer: &Entity<EditorState>,
    scroller: &Entity<MessageScrollerState>,
    cx: &mut Context<T>,
  ) -> Slot {
    let mut subscription = None;
    let body = if matched {
      None
    } else {
      Some(Rc::new(match source.as_ref() {
        message if synthetic_directive(message) => {
          let entity = cx.new(|cx| super::events::SystemActivity::new_synthetic(id.clone(), message, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Custom { id, entity }
        }
        message @ (AgentMessage::BranchSummary(_) | AgentMessage::CompactionSummary(_)) => {
          let entity = cx.new(|cx| super::events::SystemActivity::new_synthetic(id, message, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Boundary(entity)
        }
        message @ (AgentMessage::Developer(_) | AgentMessage::FileMention(_)) => {
          let entity = cx.new(|cx| super::events::SystemActivity::new_synthetic(id.clone(), message, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Custom { id, entity }
        }
        AgentMessage::User(_) => {
          let entity = cx.new(|cx| UserBlock::new(id, source.clone(), composer.clone(), cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::User(entity)
        }
        AgentMessage::Assistant(_) => {
          let entity = cx.new(|cx| AssistantBlock::new(id, source.clone(), composer.clone(), cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Assistant(entity)
        }
        AgentMessage::Custom(message) => {
          let entity = cx.new(|cx| super::events::SystemActivity::new(id.clone(), message, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Custom { id, entity }
        }
        AgentMessage::HookMessage(message) => {
          let entity = cx.new(|cx| super::events::SystemActivity::new_hook(id.clone(), message, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Custom { id, entity }
        }
        AgentMessage::BashExecution(message) => {
          let input = bash_input(message);
          let entity = cx.new(|cx| ToolBlock::new(id, input, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Tool(entity)
        }
        AgentMessage::PythonExecution(message) => {
          let input = python_input(message);
          let entity = cx.new(|cx| ToolBlock::new(id, input, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Tool(entity)
        }
        AgentMessage::ToolResult(message) => {
          let input = result_input(message);
          let entity = cx.new(|cx| ToolBlock::new(id, input, cx));
          subscription = Some(watch(&entity, scroller, cx));
          Block::Tool(entity)
        }
      }))
    };
    Slot {
      source,
      body,
      tools: Rc::new(HashMap::new()),
      streaming: false,
      _subscription: subscription,
    }
  }

  fn sync_slot<T: 'static>(
    slot: &mut Slot,
    tools: &HashMap<usize, ToolInput>,
    streaming: bool,
    source_changed: bool,
    window: &mut Window,
    cx: &mut Context<T>,
  ) {
    match (slot.body.as_deref(), slot.source.as_ref()) {
      (Some(Block::User(entity)), AgentMessage::User(_)) => entity.update(cx, |block, cx| block.sync(slot.source.clone(), cx)),
      (Some(Block::Assistant(entity)), AgentMessage::Assistant(_)) => {
        entity.update(cx, |block, cx| block.sync(slot.source.clone(), tools, streaming, window, cx))
      }
      (Some(Block::Custom { entity, .. }), AgentMessage::Custom(message)) if source_changed => {
        entity.update(cx, |block, cx| block.sync(message, cx))
      }
      (Some(Block::Custom { entity, .. }), AgentMessage::HookMessage(message)) if source_changed => {
        entity.update(cx, |block, cx| block.sync_hook(message, cx))
      }
      (Some(Block::Custom { entity, .. } | Block::Boundary(entity)), message)
        if source_changed
          && (synthetic_directive(message)
            || matches!(
              message,
              AgentMessage::Developer(_)
                | AgentMessage::FileMention(_)
                | AgentMessage::BranchSummary(_)
                | AgentMessage::CompactionSummary(_)
            )) =>
      {
        entity.update(cx, |block, cx| block.sync_synthetic(message, cx))
      }
      (Some(Block::Tool(entity)), AgentMessage::BashExecution(message)) if source_changed => {
        entity.update(cx, |block, cx| block.sync(&bash_input(message), cx))
      }
      (Some(Block::Tool(entity)), AgentMessage::PythonExecution(message)) if source_changed => {
        entity.update(cx, |block, cx| block.sync(&python_input(message), cx))
      }
      (Some(Block::Tool(entity)), AgentMessage::ToolResult(message)) if source_changed => {
        entity.update(cx, |block, cx| block.sync(&result_input(message), cx))
      }
      _ => {}
    }
  }

  pub(in crate::session) fn generation(&self) -> u64 {
    self.generation.unwrap_or_default()
  }

  pub(super) fn blocks(&self) -> Rc<BlockSnapshot> {
    self.blocks.clone()
  }

  pub(super) fn process_open(&self) -> Rc<RefCell<HashMap<usize, bool>>> {
    self.process_open.clone()
  }
}

fn watch<T: 'static, O: 'static>(entity: &Entity<T>, scroller: &Entity<MessageScrollerState>, cx: &mut Context<O>) -> Subscription {
  let scroller = scroller.clone();
  cx.observe(entity, move |_, _, cx| scroller.update(cx, |scroller, cx| scroller.remeasure(cx)))
}

#[cfg(test)]
mod tests {
  use alvin_conversation::Conversation;
  use serde_json::json;

  use super::Transcript;
  use crate::test_support::live_session;

  #[test]
  fn recorded_async_delivery_is_paired_only_with_unique_job() {
    for (recording, kind_key, kind) in [
      (
        include_str!("../../../fixtures/task_turn.messages.json"),
        "customType",
        "async-result",
      ),
      (include_str!("../../../fixtures/real_task_wait.messages.json"), "toolName", "wait"),
    ] {
      let session = live_session("/work/api");
      let mut conversation = Conversation::default();
      conversation.open(&session);
      let mut view = conversation.live(session.live).unwrap().clone();
      let recording: serde_json::Value = serde_json::from_str(recording).unwrap();
      view.transcript = serde_json::from_value(recording["messages"].clone()).unwrap();
      let result = recording["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message.pointer("/details/async/jobId").is_some())
        .unwrap();
      let delivery = super::task_delivery(&view, result).unwrap();
      assert_eq!(delivery[kind_key], kind);
      let duplicate: alvin_protocol::AgentMessage = serde_json::from_value(result.clone()).unwrap();
      view.transcript.push(duplicate);
      assert!(super::task_delivery(&view, result).is_none());
    }
  }

  #[test]
  fn committed_results_stay_at_call_positions_and_orphans_remain() {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(json!([
      {"role":"toolResult","toolCallId":"orphan","content":[{"type":"text","text":"orphan"}]},
      {"role":"assistant","content":[
        {"type":"toolCall","id":"a","name":"read","arguments":{"path":"a"}},
        {"type":"toolCall","id":"b","name":"read","arguments":{"path":"b"}}
      ]},
      {"role":"toolResult","toolCallId":"b","content":[{"type":"text","text":"B"}]},
      {"role":"toolResult","toolCallId":"a","content":[{"type":"text","text":"A"}]}
    ]))
    .unwrap();
    let prepared = Transcript::default().prepare(&view);
    assert!(!prepared.entries[0].matched);
    assert!(prepared.entries[2].matched);
    assert!(prepared.entries[3].matched);
    assert_eq!(prepared.entries[1].tools[&0].result.as_ref().unwrap()["content"][0]["text"], "A");
    assert_eq!(prepared.entries[1].tools[&1].result.as_ref().unwrap()["content"][0]["text"], "B");
  }

  #[test]
  fn committed_result_retains_live_stream_updates() {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    conversation.apply(&crate::test_support::event(
      session.live,
      json!({
        "kind":"agent",
        "frame":{"type":"tool_stream_update","toolCallId":"call","toolName":"bash","update":{"unknown":"original"}}
      }),
    ));
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(json!([
      {"role":"assistant","content":[{"type":"toolCall","id":"call","name":"bash","arguments":{"command":"echo"}}]},
      {"role":"toolResult","toolCallId":"call","toolName":"bash","content":[{"type":"text","text":"done"}]}
    ]))
    .unwrap();
    let prepared = Transcript::default().prepare(&view);
    let input = &prepared.entries[0].tools[&0];
    assert_eq!(input.result.as_ref().unwrap()["content"][0]["text"], "done");
    assert_eq!(input.stream, vec![json!({"unknown":"original"})]);
  }

  #[test]
  fn duplicate_call_ids_do_not_receive_ambiguous_live_progress() {
    let session = live_session("/work/api");
    let mut conversation = Conversation::default();
    conversation.open(&session);
    conversation.apply(&crate::test_support::event(
      session.live,
      json!({
        "kind":"agent",
        "frame":{"type":"tool_execution_start","toolCallId":"same","toolName":"read","args":{"path":"live"}}
      }),
    ));
    let mut view = conversation.live(session.live).unwrap().clone();
    view.transcript = serde_json::from_value(json!([
      {"role":"assistant","content":[
        {"type":"toolCall","id":"same","name":"read","arguments":{"path":"first"}}
      ]}
    ]))
    .unwrap();
    let one = Transcript::default().prepare(&view);
    assert!(one.entries[0].tools[&0].running);
    view.transcript = serde_json::from_value(json!([
      {"role":"assistant","content":[
        {"type":"toolCall","id":"same","name":"read","arguments":{"path":"first"}},
        {"type":"toolCall","id":"same","name":"read","arguments":{"path":"second"}}
      ]}
    ]))
    .unwrap();
    let duplicate = Transcript::default().prepare(&view);
    assert!(!duplicate.entries[0].tools[&0].running);
    assert!(!duplicate.entries[0].tools[&1].running);
    assert_eq!(duplicate.entries[0].tools[&0].args.as_ref().unwrap()["path"], "first");
    assert_eq!(duplicate.entries[0].tools[&1].args.as_ref().unwrap()["path"], "second");
  }
}
