//! The whole UI state: the workspace, every opened live session and the connection.

use std::collections::BTreeMap;

use alvin_protocol::{Event, LiveSessionId, LiveSessionSummary, MessagesPage, ProjectSummary, SessionCursor, SessionPage, SessionState};

use crate::live::LiveView;
use crate::workspace::{SessionsKey, Workspace};

/// The client's connection to the gateway, as the UI last saw it. The UI sets it from the client's connection state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Connection {
  /// Calls and event streams work.
  #[default]
  Connected,
  /// The connection dropped and the client is trying again.
  Reconnecting {
    /// Failed attempts since the drop.
    attempt: u32,
  },
  /// The connection is gone for good.
  Lost,
}

/// Projects, sessions and live sessions, folded from the protocol's responses and events.
///
/// Every operation is a plain method on owned state. None fails: decoding is done by the protocol types before values get
/// here, and the client guarantees events arrive in order without duplicates, so this does not check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conversation {
  workspace: Workspace,
  live: BTreeMap<LiveSessionId, LiveView>,
  connection: Connection,
}

impl Conversation {
  /// The projects and session lists.
  pub fn workspace(&self) -> &Workspace {
    &self.workspace
  }

  /// Replaces the projects, as `projects/list` returned them.
  pub fn set_projects(&mut self, projects: Vec<ProjectSummary>) {
    self.workspace.set_projects(projects);
  }

  /// Takes a page of `sessions/list`; `after` is the cursor the request carried.
  pub fn load_sessions(&mut self, key: SessionsKey, after: Option<&SessionCursor>, page: SessionPage) {
    self.workspace.load_sessions(key, after, page);
  }

  /// The connection to the gateway.
  pub fn connection(&self) -> Connection {
    self.connection
  }

  /// Records the connection state the client reported.
  pub fn set_connection(&mut self, connection: Connection) {
    self.connection = connection;
  }

  /// Starts tracking a live session, as `live/open` reported it. Opening one that is already tracked keeps its state.
  pub fn open(&mut self, summary: &LiveSessionSummary) {
    self.live.entry(summary.live).or_insert_with(|| LiveView::new(summary));
  }

  /// Stops tracking a live session.
  pub fn close(&mut self, live: LiveSessionId) {
    self.live.remove(&live);
  }

  /// A live session being tracked.
  pub fn live(&self, live: LiveSessionId) -> Option<&LiveView> {
    self.live.get(&live)
  }

  /// A live session being tracked, to take one-shot values from it.
  pub fn live_mut(&mut self, live: LiveSessionId) -> Option<&mut LiveView> {
    self.live.get_mut(&live)
  }

  /// Every tracked live session.
  pub fn live_sessions(&self) -> impl Iterator<Item = &LiveView> {
    self.live.values()
  }

  /// The fold: one event changes the live session it belongs to. An event for a session that is not tracked changes
  /// nothing; events only arrive for sessions the UI subscribed to, which it opened first.
  pub fn apply(&mut self, event: &Event) {
    if let Some(view) = self.live.get_mut(&event.live) {
      view.apply(&event.body);
    }
  }

  /// Replaces a session's transcript with a page of `live/messages`.
  pub fn load_page(&mut self, live: LiveSessionId, page: MessagesPage) {
    if let Some(view) = self.live.get_mut(&live) {
      view.load_page(page);
    }
  }

  /// Takes a session's `live/state`.
  pub fn set_state(&mut self, live: LiveSessionId, state: &SessionState) {
    if let Some(view) = self.live.get_mut(&live) {
      view.set_state(state);
    }
  }

  /// The event stream cannot continue without a gap: drops the transcript, the message and tools in progress, and marks
  /// the session reload-needed. The session stays tracked and listed.
  pub fn resync(&mut self, live: LiveSessionId) {
    if let Some(view) = self.live.get_mut(&live) {
      view.resync();
    }
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{AgentMessage, EventBody, MessagesPage, PromptStatus, RpcAgentEvent};

  use super::*;
  use crate::live::{Origin, Run};

  /// What one fixture holds: the events of a real omp run and `live/messages` read after it.
  struct Recorded {
    events: Vec<Event>,
    messages: MessagesPage,
  }

  fn plain() -> Recorded {
    Recorded {
      events: serde_json::from_str(include_str!("../fixtures/plain_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/plain_turn.messages.json")).unwrap(),
    }
  }

  fn tool() -> Recorded {
    Recorded {
      events: serde_json::from_str(include_str!("../fixtures/tool_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/tool_turn.messages.json")).unwrap(),
    }
  }

  fn aborted() -> Recorded {
    Recorded {
      events: serde_json::from_str(include_str!("../fixtures/aborted_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/aborted_turn.messages.json")).unwrap(),
    }
  }

  /// omp adds bookkeeping to an assistant message when it persists it, after `message_end` has gone out: an error id and the
  /// context snapshot. A view never shows them; everything else of the two transcripts must match.
  fn comparable(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
      .iter()
      .cloned()
      .map(|message| match message {
        AgentMessage::Assistant(mut assistant) => {
          assistant.error_id = None;
          assistant.extra.remove("contextSnapshot");
          AgentMessage::Assistant(assistant)
        }
        other => other,
      })
      .collect()
  }

  fn summary(recorded: &Recorded) -> LiveSessionSummary {
    LiveSessionSummary {
      live: recorded.events[0].live,
      cwd: "/repo".into(),
      file: "/sessions/one.jsonl".into(),
      exited: false,
    }
  }

  fn folded(recorded: &Recorded) -> Conversation {
    let mut conversation = Conversation::default();
    conversation.open(&summary(recorded));
    for event in &recorded.events {
      conversation.apply(event);
    }
    conversation
  }

  #[test]
  fn folding_a_plain_turn_gives_the_transcript_live_messages_returned() {
    let recorded = plain();
    let conversation = folded(&recorded);
    let view = conversation.live(summary(&recorded).live).unwrap();

    assert_eq!(comparable(&view.transcript), comparable(&recorded.messages.messages));
    assert_eq!(view.total_messages, recorded.messages.messages.len());
    assert_eq!(view.streaming, None);
    assert_eq!(view.run, Run::Settled);
    assert_eq!(view.last_prompt, Some(PromptStatus::Completed));
    assert!(!view.exited && !view.reload_needed);
  }

  #[test]
  fn folding_a_turn_with_tool_calls_gives_the_transcript_and_tracks_the_execution() {
    let recorded = tool();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));

    let mut seen_running = false;
    let mut seen_finished = false;
    for event in &recorded.events {
      conversation.apply(event);
      let tools = &conversation.live(live).unwrap().tools;
      match &event.body {
        EventBody::Agent(RpcAgentEvent::ToolExecutionStart(start)) => {
          let execution = tools.get(&start.tool_call_id).unwrap();
          assert_eq!(execution.name, start.tool_name);
          assert!(execution.outcome.is_none());
          seen_running = true;
        }
        EventBody::Agent(RpcAgentEvent::ToolExecutionEnd(end)) => {
          let execution = tools.get(&end.tool_call_id).unwrap();
          assert!(execution.outcome.as_ref().is_some_and(|outcome| !outcome.is_error));
          seen_finished = true;
        }
        _ => {}
      }
    }

    let view = conversation.live(live).unwrap();
    assert!(seen_running && seen_finished);
    assert_eq!(comparable(&view.transcript), comparable(&recorded.messages.messages));
    assert!(matches!(
      view.transcript.as_slice(),
      [
        AgentMessage::User(_),
        AgentMessage::Assistant(_),
        AgentMessage::ToolResult(_),
        AgentMessage::Assistant(_)
      ]
    ));
    // The turn ended, so its executions are cleared; the transcript holds the tool result.
    assert!(view.tools.executions().is_empty());
  }

  #[test]
  fn an_assistant_message_in_progress_is_streaming_until_it_ends() {
    let recorded = plain();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));

    let mut snapshots = Vec::new();
    for event in &recorded.events {
      conversation.apply(event);
      if matches!(&event.body, EventBody::Agent(RpcAgentEvent::MessageUpdate(_))) {
        snapshots.push(conversation.live(live).unwrap().streaming.clone());
      }
    }

    assert!(!snapshots.is_empty());
    assert!(
      snapshots
        .iter()
        .all(|snapshot| snapshot.as_ref().is_some_and(|streaming| streaming.message_id.is_some()))
    );
    assert_eq!(conversation.live(live).unwrap().streaming, None);
  }

  #[test]
  fn folding_an_aborted_turn_gives_the_transcript_and_the_aborted_status() {
    let recorded = aborted();
    let conversation = folded(&recorded);
    let view = conversation.live(summary(&recorded).live).unwrap();

    assert_eq!(comparable(&view.transcript), comparable(&recorded.messages.messages));
    assert_eq!(view.last_prompt, Some(PromptStatus::Aborted));
    assert_eq!(view.run, Run::Settled);
    assert_eq!(view.streaming, None);
  }

  #[test]
  fn a_process_exit_mid_turn_ends_the_streaming_message_and_marks_the_session() {
    let recorded = plain();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));
    // Up to the first streaming update, then the process dies.
    let cut = recorded
      .events
      .iter()
      .position(|event| matches!(&event.body, EventBody::Agent(RpcAgentEvent::MessageUpdate(_))))
      .unwrap();
    for event in &recorded.events[..=cut] {
      conversation.apply(event);
    }
    assert!(conversation.live(live).unwrap().streaming.is_some());

    let exit = Event {
      body: EventBody::ProcessExited,
      ..recorded.events[cut].clone()
    };
    conversation.apply(&exit);

    let view = conversation.live(live).unwrap();
    assert!(view.exited);
    assert_eq!(view.streaming, None);
    assert!(view.tools.executions().is_empty());
  }

  #[test]
  fn a_resync_drops_the_live_parts_and_keeps_the_session_listed() {
    let recorded = plain();
    let live = summary(&recorded).live;
    let mut conversation = folded(&recorded);
    conversation.set_projects(vec![project_of(&summary(&recorded))]);

    conversation.resync(live);

    let view = conversation.live(live).unwrap();
    assert!(view.transcript.is_empty());
    assert_eq!(view.total_messages, 0);
    assert_eq!(view.streaming, None);
    assert!(view.tools.executions().is_empty());
    assert!(view.reload_needed);
    // Not the session's identity, nor what the user was told.
    assert_eq!(view.file, summary(&recorded).file);
    assert_eq!(conversation.workspace().projects().len(), 1);
    assert!(conversation.live_sessions().any(|view| view.live == live));

    conversation.load_page(live, recorded.messages.clone());
    let view = conversation.live(live).unwrap();
    assert_eq!(comparable(&view.transcript), comparable(&recorded.messages.messages));
    assert!(!view.reload_needed);
  }

  fn project_of(summary: &LiveSessionSummary) -> ProjectSummary {
    ProjectSummary {
      id: "p".to_owned().into(),
      root: summary.cwd.clone(),
      checkouts: Vec::new(),
    }
  }

  #[test]
  fn recorded_extension_requests_and_commands_show_up_as_widgets_and_commands() {
    let recorded = plain();
    let conversation = folded(&recorded);
    let view = conversation.live(summary(&recorded).live).unwrap();

    // The recorded run set and cleared a widget and announced the slash commands.
    assert!(view.widgets.is_empty());
    assert!(view.thinking.is_some());
    assert!(
      !view.commands.is_empty()
        || recorded
          .events
          .iter()
          .all(|event| !matches!(event.body, EventBody::AvailableCommandsUpdate(_)))
    );
  }

  #[test]
  fn events_for_a_session_that_is_not_tracked_change_nothing() {
    let recorded = plain();
    let mut conversation = Conversation::default();
    for event in &recorded.events {
      conversation.apply(event);
    }
    assert_eq!(conversation, Conversation::default());
  }

  #[test]
  fn opening_a_tracked_session_again_keeps_its_state() {
    let recorded = plain();
    let mut conversation = folded(&recorded);
    conversation.open(&summary(&recorded));
    assert_eq!(
      comparable(&conversation.live(summary(&recorded).live).unwrap().transcript),
      comparable(&recorded.messages.messages)
    );
  }

  #[test]
  fn notices_keep_arrival_order_and_origin() {
    let recorded = plain();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));
    let notice = Event {
      body: EventBody::CommandOutput(alvin_protocol::CommandOutputEvent { text: "one".to_owned() }),
      ..recorded.events[0].clone()
    };
    let frame_error = Event {
      body: EventBody::RpcFrameError(alvin_protocol::RpcFrameErrorEvent {
        error: "too big".to_owned(),
        original_type: Some("message_update".to_owned()),
      }),
      ..recorded.events[0].clone()
    };
    conversation.apply(&notice);
    conversation.apply(&frame_error);

    let origins: Vec<_> = conversation
      .live(live)
      .unwrap()
      .notices
      .iter()
      .map(|notice| notice.origin.clone())
      .collect();
    assert_eq!(
      origins,
      [Origin::CommandOutput, Origin::FrameDropped(Some("message_update".to_owned()))]
    );
  }
}
