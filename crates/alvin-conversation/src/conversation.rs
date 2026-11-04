//! The whole UI state: the workspace, every opened live session and the connection.

use std::collections::BTreeMap;

use alvin_protocol::{
  EntryId, Event, EventId, HistoryEntries, LiveSessionId, LiveSessionSummary, MessagesPage, ProjectSummary, SessionCursor, SessionPage,
  SessionState,
};

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

  /// Takes a session's `live/state` using the token captured before the request.
  pub fn set_state(&mut self, live: LiveSessionId, state: &SessionState, request: EventId) {
    if let Some(view) = self.live.get_mut(&live) {
      view.set_state(state, request);
    }
  }

  /// Replaces a session's tree with the one these entries (all of them) make: on open, after a resync, and when omp does not
  /// know the `since` of an earlier read.
  pub fn set_entries(&mut self, live: LiveSessionId, entries: HistoryEntries) {
    if let Some(view) = self.live.get_mut(&live) {
      view.set_entries(entries);
    }
  }

  /// Adds the entries omp appended after [`Conversation::tree_cursor`].
  pub fn apply_entries(&mut self, live: LiveSessionId, entries: HistoryEntries) {
    if let Some(view) = self.live.get_mut(&live) {
      view.apply_entries(entries);
    }
  }

  /// Forgets a session's tree, so the next read of entries (with no `since`) builds it anew. A fork or branch moves the
  /// session onto another file, whose entries are not an extension of the old tree's.
  pub fn forget_tree(&mut self, live: LiveSessionId) {
    if let Some(view) = self.live.get_mut(&live) {
      view.tree = None;
      view.tree_stale = true;
    }
  }

  /// The `since` of the next read of entries: the newest entry the tree has. `None` when there is no tree yet.
  pub fn tree_cursor(&self, live: LiveSessionId) -> Option<&EntryId> {
    self.live.get(&live)?.tree.as_ref()?.cursor()
  }

  /// Closes the dialog a request named, after the user answered or dismissed it.
  pub fn dialog_answered(&mut self, live: LiveSessionId, request: &str) {
    if let Some(view) = self.live.get_mut(&live) {
      view.dialogs.close(request);
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
  use alvin_protocol::{AgentMessage, EventBody, ExtensionUiRequest, HistoryEntries, MessagesPage, PromptStatus, RpcAgentEvent};

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

  fn queued() -> Recorded {
    Recorded {
      events: serde_json::from_str(include_str!("../fixtures/queue_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/queue_turn.messages.json")).unwrap(),
    }
  }

  fn asked() -> Recorded {
    Recorded {
      events: serde_json::from_str(include_str!("../fixtures/ask_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/ask_turn.messages.json")).unwrap(),
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

  #[test]
  fn the_queue_follows_queue_updates_through_a_recorded_turn() {
    let recorded = queued();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));

    let mut waiting = Vec::new();
    for event in &recorded.events {
      conversation.apply(event);
      if matches!(&event.body, EventBody::Agent(RpcAgentEvent::QueueUpdate(_))) {
        waiting.push(
          conversation
            .live(live)
            .unwrap()
            .queue
            .follow_up
            .iter()
            .map(|message| message.text.clone())
            .collect::<Vec<_>>(),
        );
      }
    }

    // The follow-up was queued while the first turn ran, then sent when it ended.
    assert_eq!(
      waiting,
      [vec!["Now reply with the single word done.".to_owned()], Vec::<String>::new()]
    );
    let view = conversation.live(live).unwrap();
    assert!(view.queue.steering.is_empty());
    assert_eq!(view.queue.live_steered, 0);
    assert_eq!(comparable(&view.transcript), comparable(&recorded.messages.messages));
    assert_eq!(view.run, Run::Settled);
  }

  #[test]
  fn an_ask_opens_a_dialog_that_stays_until_it_is_answered() {
    let recorded = asked();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));

    let mut opened = None;
    for event in &recorded.events {
      conversation.apply(event);
      if let EventBody::ExtensionUiRequest(ExtensionUiRequest::Ask(ask)) = &event.body {
        let pending = conversation.live(live).unwrap().dialogs.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(crate::request_id(&pending[0]), ask.id);
        opened = Some(ask.id.clone());
      }
    }

    // The widget requests around the dialog show nothing; the dialog is still waiting.
    let id = opened.expect("the run asked a question");
    assert_eq!(conversation.live(live).unwrap().dialogs.pending().len(), 1);
    conversation.dialog_answered(live, &id);
    assert!(conversation.live(live).unwrap().dialogs.pending().is_empty());
    assert_eq!(
      comparable(&conversation.live(live).unwrap().transcript),
      comparable(&recorded.messages.messages)
    );
  }

  #[test]
  fn an_extension_cancel_closes_the_dialog_it_names() {
    let recorded = asked();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));
    let ask = recorded
      .events
      .iter()
      .find(|event| matches!(&event.body, EventBody::ExtensionUiRequest(ExtensionUiRequest::Ask(_))))
      .expect("the run asked a question");
    conversation.apply(ask);
    let EventBody::ExtensionUiRequest(ExtensionUiRequest::Ask(request)) = &ask.body else {
      unreachable!()
    };

    let mut cancel = ask.clone();
    cancel.body = EventBody::ExtensionUiRequest(ExtensionUiRequest::Cancel(alvin_protocol::CancelUiRequest {
      id: "cancel".to_owned(),
      target_id: request.id.clone(),
    }));
    conversation.apply(&cancel);

    assert!(conversation.live(live).unwrap().dialogs.pending().is_empty());
  }

  #[test]
  fn history_grows_after_each_turn_and_a_resync_drops_the_tree() {
    let recorded: Recorded = Recorded {
      events: serde_json::from_str(include_str!("../fixtures/tree_turn.events.json")).unwrap(),
      messages: serde_json::from_str(include_str!("../fixtures/tree_turn.messages.json")).unwrap(),
    };
    let before: HistoryEntries = serde_json::from_str(include_str!("../fixtures/tree_turn.entries_before.json")).unwrap();
    let delta: HistoryEntries = serde_json::from_str(include_str!("../fixtures/tree_turn.entries_delta.json")).unwrap();
    let live = summary(&recorded).live;
    let mut conversation = Conversation::default();
    conversation.open(&summary(&recorded));
    assert_eq!(conversation.tree_cursor(live), None);

    conversation.set_entries(live, before.clone());
    assert!(!conversation.live(live).unwrap().tree_stale);
    assert_eq!(conversation.tree_cursor(live), before.entries.last().map(|entry| &entry.id));

    // The turn that follows ends, which says history may have grown; reading the entries since the cursor settles that.
    for event in &recorded.events {
      conversation.apply(event);
    }
    assert!(conversation.live(live).unwrap().tree_stale);
    conversation.apply_entries(live, delta.clone());
    assert!(!conversation.live(live).unwrap().tree_stale);
    assert_eq!(conversation.tree_cursor(live), delta.entries.last().map(|entry| &entry.id));

    conversation.resync(live);
    assert!(conversation.live(live).unwrap().tree.is_none());
  }
}
