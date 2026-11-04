//! One live session as a view renders it, and the fold of its events.

use std::collections::BTreeMap;
use std::path::PathBuf;

use alvin_protocol::{
  AgentMessage, AutoRetryStartEvent, AvailableSlashCommand, ConfiguredThinkingLevel, CustomMessage, EventBody, EventId, ExtensionUiRequest,
  HistoryEntries, LiveSessionId, LiveSessionSummary, MessageEndEvent, MessageStartEvent, MessageUpdateEvent, MessagesPage, ModelInfo,
  NotifyType, PromptStatus, RpcAgentEvent, SessionState, ThinkingLevel, TodoPhase, TodoReminderEvent, WidgetPlacement,
};
use serde_json::Value;

use crate::dialogs::Dialogs;
use crate::queue::Queue;
use crate::streaming::Streaming;
use crate::tools::Tools;
use crate::tree::Tree;

/// Where the session stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Run {
  /// No turn is running and nothing is known to be finished.
  #[default]
  Idle,
  /// The agent is working on a turn, or will resume without input (a continuation, a background job).
  Running,
  /// Nothing can wake the session again.
  Settled,
}

/// A compaction in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
  /// Whether the user asked for it (`/compact`), as opposed to omp deciding the context is full.
  pub manual: bool,
}

/// An automatic retry omp is waiting out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
  /// Which attempt is coming.
  pub attempt: i64,
  /// How many omp will make.
  pub max_attempts: i64,
  /// How long omp waits first.
  pub delay_ms: i64,
  /// What failed.
  pub error_message: String,
}

/// Severity of a [`Notice`].
pub type Level = NotifyType;

/// Where a [`Notice`] came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
  /// omp's own notice, with its source when it gave one.
  Notice(Option<String>),
  /// Output of a builtin slash command.
  CommandOutput,
  /// An extension showed it.
  Extension,
  /// An extension failed.
  ExtensionError {
    /// The extension.
    path: String,
    /// The event it was handling.
    event: String,
  },
  /// A compaction or retry that did not succeed.
  Run,
  /// omp could not send an event of this type at all.
  FrameDropped(Option<String>),
  /// A frame arrived that did not decode.
  Undecodable,
  /// Rules reported by a TTSR trigger, including their complete metadata.
  TtsrTriggered(Vec<serde_json::Map<String, Value>>),
  /// An unfinished-task reminder with its reported tasks and attempt.
  TodoReminder(TodoReminderEvent),
}

/// Something the user should be told, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
  /// Reported or source-defined severity, if any.
  pub level: Option<Level>,
  /// Where it came from.
  pub origin: Origin,
  /// What it says.
  pub text: String,
  /// The original payload when a frame could not be decoded.
  pub raw: Option<Value>,
}

/// A widget an extension asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Widget {
  /// The lines to show.
  pub lines: Vec<String>,
  /// Above or below the composer.
  pub placement: Option<WidgetPlacement>,
}

/// One live session: its transcript, the turn in progress and everything around it.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveView {
  /// The live session.
  pub live: LiveSessionId,
  /// The working directory omp runs in.
  pub cwd: PathBuf,
  /// The session file.
  pub file: PathBuf,
  /// Committed messages, oldest first: loaded from `live/messages`, extended by `message_end`.
  pub transcript: Vec<AgentMessage>,
  /// Process-scoped message IDs for committed live events, aligned with the transcript.
  pub message_ids: Vec<Option<String>>,
  /// Changes whenever a historical snapshot replaces the transcript.
  pub snapshot_generation: u64,
  /// How many messages the session has, as `live/messages` last reported.
  pub total_messages: usize,
  /// The assistant message being built.
  pub streaming: Option<Streaming>,
  /// Tool executions of the current turn.
  pub tools: Tools,
  /// The current plan, independent of historical todo tool results.
  pub todo_phases: Vec<TodoPhase>,
  /// The latest unfinished-task reminder, if one has not been cleared.
  pub todo_reminder: Option<TodoReminderEvent>,
  /// IRC arrivals not yet represented by committed history.
  irc_pending: Vec<CustomMessage>,
  state_request: EventId,
  /// Where the session stands.
  pub run: Run,
  /// How the last prompt ended.
  pub last_prompt: Option<PromptStatus>,
  /// A compaction in progress.
  pub compaction: Option<Compaction>,
  /// A retry omp is waiting out.
  pub retry: Option<Retry>,
  /// Messages for the user, in arrival order.
  pub notices: Vec<Notice>,
  /// The session's title.
  pub title: Option<String>,
  /// Whether the omp process ended on its own. Nothing more arrives.
  pub exited: bool,
  /// Set by a resync: the transcript was dropped and must be loaded again.
  pub reload_needed: bool,
  /// Set when omp said the model changed without saying to what: reload `live/state`.
  pub state_stale: bool,
  /// The history tree, once loaded.
  pub tree: Option<Tree>,
  /// Set when something happened that can have added entries: read the entries after [`Tree::cursor`].
  pub tree_stale: bool,
  /// The queued steering and follow-up messages.
  pub queue: Queue,
  /// Dialogs waiting for an answer.
  pub dialogs: Dialogs,
  /// The slash commands omp offers.
  pub commands: Vec<AvailableSlashCommand>,
  /// The model in use.
  pub model: Option<ModelInfo>,
  /// The thinking level in use.
  pub thinking: Option<ThinkingLevel>,
  /// The selector reported by a thinking-change event, when distinct from the effective level.
  pub thinking_selector: Option<ConfiguredThinkingLevel>,
  /// Status lines extensions set, by key.
  pub statuses: BTreeMap<String, String>,
  /// Widgets extensions set, by key.
  pub widgets: BTreeMap<String, Widget>,
  /// Text an extension asked to put in the composer. The view takes it with [`LiveView::take_editor_text`].
  editor_text: Option<String>,
}

impl LiveView {
  pub(crate) fn new(summary: &LiveSessionSummary) -> Self {
    Self {
      live: summary.live,
      cwd: summary.cwd.clone(),
      file: summary.file.clone(),
      transcript: Vec::new(),
      message_ids: Vec::new(),
      snapshot_generation: 0,
      total_messages: 0,
      streaming: None,
      tools: Tools::default(),
      todo_phases: Vec::new(),
      todo_reminder: None,
      irc_pending: Vec::new(),
      state_request: EventId::new(),
      run: Run::Idle,
      last_prompt: None,
      compaction: None,
      retry: None,
      notices: Vec::new(),
      title: None,
      exited: summary.exited,
      reload_needed: false,
      state_stale: false,
      tree: None,
      tree_stale: false,
      queue: Queue::default(),
      dialogs: Dialogs::default(),
      commands: Vec::new(),
      model: None,
      thinking: None,
      thinking_selector: None,
      statuses: BTreeMap::new(),
      widgets: BTreeMap::new(),
      editor_text: None,
    }
  }

  /// The text an extension asked to put in the composer, once.
  pub fn take_editor_text(&mut self) -> Option<String> {
    self.editor_text.take()
  }

  /// Starts a state request, invalidating earlier requests for this session.
  pub fn begin_state_request(&mut self) -> EventId {
    self.state_request = EventId::new();
    self.state_request
  }

  /// Replaces the tree with the one the session's entries make.
  pub(crate) fn set_entries(&mut self, entries: HistoryEntries) {
    self.tree = Some(Tree::from_entries(entries));
    self.tree_stale = false;
  }

  /// Adds the entries appended since the tree's newest. Without a tree there is nothing to add them to: they are the
  /// whole history only if they start at the beginning, so the caller reads them with no `since` first.
  pub(crate) fn apply_entries(&mut self, entries: HistoryEntries) {
    match &mut self.tree {
      Some(tree) => tree.apply(entries),
      None => self.tree = Some(Tree::from_entries(entries)),
    }
    self.tree_stale = false;
  }

  /// Replaces the transcript with a page of `live/messages` and clears the reload-needed mark.
  pub(crate) fn load_page(&mut self, page: MessagesPage) {
    self.total_messages = usize::try_from(page.total_messages).unwrap_or(0);
    self.irc_pending.retain(|pending| {
      !Self::irc_id(pending).is_some_and(|id| page.messages.iter().any(|message| Self::message_irc_id(message) == Some(id)))
    });
    self.transcript = page.messages;
    self.transcript.extend(self.irc_pending.iter().cloned().map(AgentMessage::Custom));
    self.message_ids = vec![None; self.transcript.len()];
    self.snapshot_generation = self.snapshot_generation.saturating_add(1);
    self.reload_needed = false;
  }

  pub(crate) fn set_state(&mut self, state: &SessionState, request: EventId) {
    if self.model != state.model || self.thinking != state.thinking_level {
      self.thinking_selector = None;
    }
    self.model.clone_from(&state.model);
    self.thinking = state.thinking_level;
    if self.state_request == request {
      self.todo_phases.clone_from(&state.todo_phases);
      self.state_stale = false;
    }
    if self.run != Run::Settled || state.is_streaming {
      self.run = if state.is_streaming { Run::Running } else { Run::Idle };
    }
    if let Some(name) = &state.session_name {
      self.title = Some(name.clone());
    }
  }

  /// Drops what a gap makes untrustworthy and marks the session for reloading. Notices, title, commands and the rest stay.
  pub(crate) fn resync(&mut self) {
    self.transcript.clear();
    self.message_ids.clear();
    self.transcript.extend(self.irc_pending.iter().cloned().map(AgentMessage::Custom));
    self.message_ids.resize(self.transcript.len(), None);
    self.snapshot_generation = self.snapshot_generation.saturating_add(1);
    self.total_messages = 0;
    self.streaming = None;
    self.tools.clear();
    self.reload_needed = true;
    self.tree = None;
    self.tree_stale = false;
    self.thinking_selector = None;
    self.todo_phases.clear();
    self.todo_reminder = None;
    self.state_request = EventId::new();
    self.state_stale = true;
  }

  /// The effect of one event. Every variant of [`EventBody`] has one; frames that change nothing a view shows say so.
  pub(crate) fn apply(&mut self, body: &EventBody) {
    match body {
      EventBody::Agent(event) => self.agent(event),
      EventBody::PromptResult(result) => {
        self.last_prompt = Some(result.status);
        if result.session_settled {
          self.run = Run::Settled;
        }
      }
      EventBody::SessionSettled(_) => self.run = Run::Settled,
      EventBody::ExtensionUiRequest(request) => self.extension_request(request),
      EventBody::ExtensionError(error) => self.notice(
        NotifyType::Error,
        Origin::ExtensionError {
          path: error.extension_path.clone(),
          event: error.event.clone(),
        },
        error.error.clone(),
      ),
      EventBody::AvailableCommandsUpdate(update) => self.commands.clone_from(&update.commands),
      EventBody::CommandOutput(output) => self.notice(NotifyType::Info, Origin::CommandOutput, output.text.clone()),
      EventBody::SessionInfoUpdate(update) => {
        if update.title.is_some() {
          self.title.clone_from(&update.title);
        }
      }
      EventBody::ConfigUpdate(update) => {
        if (update.model.is_some() && self.model != update.model)
          || (update.thinking_level.is_some() && self.thinking != update.thinking_level)
        {
          self.thinking_selector = None;
        }
        if update.model.is_some() {
          self.model.clone_from(&update.model);
        }
        if update.thinking_level.is_some() {
          self.thinking = update.thinking_level;
        }
      }
      EventBody::RpcFrameError(error) => self.notice(
        NotifyType::Warning,
        Origin::FrameDropped(error.original_type.clone()),
        error.error.clone(),
      ),
      EventBody::Undecodable { error, raw } => self.notices.push(Notice {
        level: Some(NotifyType::Warning),
        origin: Origin::Undecodable,
        text: error.clone(),
        raw: Some(raw.clone()),
      }),
      EventBody::ProcessExited => {
        self.exited = true;
        self.streaming = None;
        self.tools.clear();
        // Nobody is left to answer.
        self.dialogs.clear();
        self.compaction = None;
        self.retry = None;
      }
    }
  }

  fn agent(&mut self, event: &RpcAgentEvent) {
    match event {
      RpcAgentEvent::AgentStart(_) => self.run = Run::Running,
      RpcAgentEvent::AgentEnd(end) => {
        self.tree_stale = true;
        // A non-terminal end means a continuation or background work remains: the run goes on.
        if end.is_terminal != Some(false) && self.run != Run::Settled {
          self.run = Run::Idle;
        }
      }
      RpcAgentEvent::TurnEnd(_) => {
        self.tools.clear();
        self.tree_stale = true;
      }
      RpcAgentEvent::MessageStart(start) => self.message_start(start),
      RpcAgentEvent::MessageUpdate(update) => self.message_update(update),
      RpcAgentEvent::MessageEnd(end) => self.message_end(end),
      RpcAgentEvent::ToolExecutionStart(start) => self.tools.start(start),
      RpcAgentEvent::ToolExecutionUpdate(update) => self.tools.update(update),
      RpcAgentEvent::ToolStreamUpdate(update) => self.tools.stream_update(update),
      RpcAgentEvent::ToolExecutionEnd(end) => {
        self.tools.end(end);
        if end.tool_name == "todo"
          && end.is_error == Some(false)
          && let Some(phases) = end
            .result
            .as_ref()
            .and_then(|result| result.get("details"))
            .and_then(|details| details.get("phases"))
          && let Ok(phases) = serde_json::from_value::<Vec<TodoPhase>>(phases.clone())
        {
          self.todo_phases = phases;
          self.state_request = EventId::new();
        }
      }
      RpcAgentEvent::AutoCompactionStart(_) => self.compaction = Some(Compaction { manual: false }),
      RpcAgentEvent::AutoCompactionEnd(end) => {
        self.compaction = None;
        self.tree_stale = true;
        if let Some(message) = &end.error_message
          && !end.aborted
          && end.skipped != Some(true)
        {
          self.notice(NotifyType::Error, Origin::Run, format!("Compaction failed: {message}"));
        }
      }
      RpcAgentEvent::ManualCompactionStart(_) => self.compaction = Some(Compaction { manual: true }),
      RpcAgentEvent::ManualCompactionEnd(end) => {
        self.compaction = None;
        self.tree_stale = true;
        if let Some(message) = &end.error_message
          && !end.aborted
        {
          self.notice(NotifyType::Warning, Origin::Run, message.clone());
        }
      }
      RpcAgentEvent::AutoRetryStart(start) => self.retry = Some(Retry::from(start)),
      RpcAgentEvent::AutoRetryEnd(end) => {
        self.retry = None;
        if let Some(error) = &end.final_error
          && !end.success
        {
          self.notice(
            NotifyType::Error,
            Origin::Run,
            format!("Gave up after {} attempts: {error}", end.attempt),
          );
        }
      }
      RpcAgentEvent::ModelChanged(_) => self.state_stale = true,
      RpcAgentEvent::ThinkingLevelChanged(changed) => {
        self.thinking_selector = changed.configured;
        if changed.thinking_level.is_some() {
          self.thinking = changed.thinking_level;
        }
      }
      RpcAgentEvent::RetryFallbackApplied(fallback) => {
        let mut text = format!("Fallback: {} → {}\nRole: {}", fallback.from, fallback.to, fallback.role);
        if let Some(reason) = &fallback.reason {
          text.push('\n');
          text.push_str(reason);
        }
        self.notice(NotifyType::Warning, Origin::Run, text);
      }
      RpcAgentEvent::RetryFallbackSucceeded(fallback) => self.notices.push(Notice {
        level: None,
        origin: Origin::Run,
        text: format!("Fallback succeeded on {}\nRole: {}", fallback.model, fallback.role),
        raw: None,
      }),
      RpcAgentEvent::Notice(notice) => self.notice(notice.level, Origin::Notice(notice.source.clone()), notice.message.clone()),
      RpcAgentEvent::QueueUpdate(update) => self.queue.replace(update),
      RpcAgentEvent::TodoReminder(reminder) => {
        self.todo_reminder = Some(reminder.clone());
        self.notices.push(Notice {
          level: None,
          origin: Origin::TodoReminder(reminder.clone()),
          text: format!("Attempt {} of {}", reminder.attempt, reminder.max_attempts),
          raw: None,
        });
      }
      RpcAgentEvent::TodoAutoClear(_) => {
        self.todo_phases.clear();
        self.todo_reminder = None;
        self.state_request = EventId::new();
      }
      RpcAgentEvent::TtsrTriggered(event) => self.notice(
        NotifyType::Warning,
        Origin::TtsrTriggered(event.rules.clone()),
        "Rules triggered".to_owned(),
      ),
      RpcAgentEvent::IrcMessage(event) => self.irc_message(&event.message),
      // These are lifecycle or bookkeeping updates.
      RpcAgentEvent::TurnStart(_)
      | RpcAgentEvent::CacheWarmingStart(_)
      | RpcAgentEvent::CacheWarmingEnd(_)
      | RpcAgentEvent::ConfigWarningsChanged(_)
      | RpcAgentEvent::AdvisorCostChanged(_)
      | RpcAgentEvent::AdvisorYielded(_)
      | RpcAgentEvent::GoalUpdated(_) => {}
    }
  }

  fn message_start(&mut self, start: &MessageStartEvent) {
    self.streaming = Some(Streaming {
      message_id: start.message_id.clone(),
      message: start.message.clone(),
    });
  }

  fn message_update(&mut self, update: &MessageUpdateEvent) {
    // omp sends the message so far with every update, so the update replaces the snapshot. An update for a message this
    // view never saw start (it subscribed mid-message) starts the snapshot.
    let message_id = update
      .message_id
      .clone()
      .or_else(|| self.streaming.as_ref().and_then(|streaming| streaming.message_id.clone()));
    self.streaming = Some(Streaming {
      message_id,
      message: update.message.clone(),
    });
  }

  fn irc_id(message: &CustomMessage) -> Option<&str> {
    if message.custom_type.as_deref() != Some("irc:incoming") {
      return None;
    }
    message.details.as_ref()?.get("id")?.as_str().filter(|id| !id.is_empty())
  }

  fn message_irc_id(message: &AgentMessage) -> Option<&str> {
    match message {
      AgentMessage::Custom(message) => Self::irc_id(message),
      _ => None,
    }
  }

  fn irc_message(&mut self, message: &CustomMessage) {
    if Self::irc_id(message).is_some_and(|id| self.transcript.iter().any(|existing| Self::message_irc_id(existing) == Some(id))) {
      return;
    }
    self.irc_pending.push(message.clone());
    self.transcript.push(AgentMessage::Custom(message.clone()));
    self.message_ids.resize(self.transcript.len(), None);
  }

  fn message_end(&mut self, end: &MessageEndEvent) {
    if let Some(id) = Self::message_irc_id(&end.message)
      && let Some(pending) = self.irc_pending.iter().position(|message| Self::irc_id(message) == Some(id))
    {
      self.irc_pending.remove(pending);
      if let Some(index) = self.transcript.iter().position(|message| Self::message_irc_id(message) == Some(id)) {
        if let Some(message) = self.transcript.get_mut(index) {
          *message = end.message.clone();
        }
        self.message_ids.resize(self.transcript.len(), None);
        if let Some(message_id) = self.message_ids.get_mut(index) {
          *message_id = end.message_id.clone();
        }
        self.total_messages = self.total_messages.saturating_add(1);
        self.streaming = None;
        return;
      }
    }
    if Self::message_irc_id(&end.message).is_some_and(|id| self.transcript.iter().any(|message| Self::message_irc_id(message) == Some(id)))
    {
      self.streaming = None;
      return;
    }
    self.message_ids.resize(self.transcript.len(), None);
    self.message_ids.push(
      end
        .message_id
        .clone()
        .or_else(|| self.streaming.as_ref().and_then(|streaming| streaming.message_id.clone())),
    );
    self.transcript.push(end.message.clone());
    self.total_messages = self.total_messages.saturating_add(1);
    self.streaming = None;
  }

  fn extension_request(&mut self, request: &ExtensionUiRequest) {
    match request {
      ExtensionUiRequest::Select(_)
      | ExtensionUiRequest::Confirm(_)
      | ExtensionUiRequest::Input(_)
      | ExtensionUiRequest::Editor(_)
      | ExtensionUiRequest::Ask(_) => self.dialogs.open(request.clone()),
      ExtensionUiRequest::Cancel(cancel) => self.dialogs.close(&cancel.target_id),
      ExtensionUiRequest::Notify(notify) => {
        self.notice(
          notify.notify_type.unwrap_or(NotifyType::Info),
          Origin::Extension,
          notify.message.clone(),
        );
      }
      ExtensionUiRequest::SetStatus(status) => match &status.status_text {
        Some(text) => {
          self.statuses.insert(status.status_key.clone(), text.clone());
        }
        None => {
          self.statuses.remove(&status.status_key);
        }
      },
      ExtensionUiRequest::SetWidget(widget) => match &widget.widget_lines {
        Some(lines) => {
          self.widgets.insert(
            widget.widget_key.clone(),
            Widget {
              lines: lines.clone(),
              placement: widget.widget_placement,
            },
          );
        }
        None => {
          self.widgets.remove(&widget.widget_key);
        }
      },
      ExtensionUiRequest::SetTitle(title) => self.title = Some(title.title.clone()),
      ExtensionUiRequest::SetEditorText(text) => self.editor_text = Some(text.text.clone()),
      ExtensionUiRequest::OpenUrl(open) => self.notice(NotifyType::Info, Origin::Extension, open.url.clone()),
    }
  }

  fn notice(&mut self, level: Level, origin: Origin, text: String) {
    self.notices.push(Notice {
      level: Some(level),
      origin,
      text,
      raw: None,
    });
  }
}

impl From<&AutoRetryStartEvent> for Retry {
  fn from(start: &AutoRetryStartEvent) -> Self {
    Self {
      attempt: start.attempt,
      max_attempts: start.max_attempts,
      delay_ms: start.delay_ms,
      error_message: start.error_message.clone(),
    }
  }
}
#[cfg(test)]
mod tests {
  use alvin_protocol::{
    AgentMessage, CustomMessage, Event, EventBody, IrcMessageEvent, LiveSessionId, LiveSessionSummary, MessageEndEvent, MessagesPage,
    NotifyType, RpcAgentEvent, SessionState,
  };
  use serde_json::{Value, json};

  use super::LiveView;

  fn events(source: &str) -> Vec<Event> {
    serde_json::from_str(source).unwrap()
  }

  fn view(events: &[Event]) -> LiveView {
    LiveView::new(&LiveSessionSummary {
      live: events[0].live,
      cwd: "/repo".into(),
      file: "/session".into(),
      exited: false,
    })
  }

  fn state(source: &str) -> SessionState {
    serde_json::from_str(source).unwrap()
  }

  fn install(view: &mut LiveView, state: &SessionState) {
    let request = view.begin_state_request();
    view.set_state(state, request);
  }

  fn irc(id: Value, display: bool) -> CustomMessage {
    serde_json::from_value(json!({
      "role": "custom", "customType": "irc:incoming", "content": "envelope", "display": display,
      "details": {"id": id, "from": "peer", "message": "body", "future": {"key": true}},
      "attribution": "agent", "unknown": 7
    }))
    .unwrap()
  }

  fn page(messages: Vec<AgentMessage>) -> MessagesPage {
    MessagesPage {
      total_messages: i64::try_from(messages.len()).unwrap(),
      messages,
      next_cursor: None,
    }
  }

  #[test]
  fn undecodable_notice_preserves_original_payload_and_error() {
    let mut view = LiveView::new(&LiveSessionSummary {
      live: LiveSessionId::new(),
      cwd: "/repo".into(),
      file: "/session".into(),
      exited: false,
    });
    let raw = json!({"type": "message_end", "message": {"unexpected": ["α", 7]}});
    view.apply(&EventBody::Undecodable {
      raw: raw.clone(),
      error: "missing field role".into(),
    });
    let notice = view.notices.last().unwrap();
    assert_eq!(notice.raw.as_ref(), Some(&raw));
    assert_eq!(notice.text, "missing field role");
    assert_eq!(notice.level, Some(NotifyType::Warning));
    assert_eq!(notice.origin, super::Origin::Undecodable);
  }

  #[test]
  fn rule_triggers_preserve_every_violation_and_rule_field() {
    let mut view = LiveView::new(&LiveSessionSummary {
      live: LiveSessionId::new(),
      cwd: "/repo".into(),
      file: "/session".into(),
      exited: false,
    });
    let rules = json!([{"name":"safety","path":"/rules/safety.md","content":"complete rule","future":{"x":7}}]);
    let event = RpcAgentEvent::from_value(json!({"type":"ttsr_triggered","rules":rules})).unwrap();
    view.apply(&EventBody::Agent(event.clone()));
    view.apply(&EventBody::Agent(event));
    assert_eq!(view.notices.len(), 2);
    for notice in &view.notices {
      assert_eq!(notice.level, Some(NotifyType::Warning));
      let super::Origin::TtsrTriggered(retained) = &notice.origin else {
        panic!("expected rule trigger");
      };
      assert_eq!(json!(retained), rules);
      assert!(notice.raw.is_none());
    }
  }

  #[test]
  fn recorded_rule_trigger_retains_source_and_full_content() {
    let events = events(include_str!("../fixtures/real_ttsr_trigger.events.json"));
    let mut view = view(&events);
    for event in &events {
      view.apply(&event.body);
    }
    let notice = &view.notices[0];
    assert_eq!(notice.level, Some(NotifyType::Warning));
    let super::Origin::TtsrTriggered(rules) = &notice.origin else {
      panic!("expected rule trigger");
    };
    assert_eq!(rules[0]["name"], "capture-rule");
    assert_eq!(rules[0]["content"], "This temporary capture rule records a thinking-stream match.");
    assert_eq!(rules[0]["_source"]["level"], "project");
    assert_eq!(rules[0]["scope"], json!(["thinking"]));
  }

  #[test]
  fn fallback_warning_preserves_source_fields() {
    let mut view = LiveView::new(&LiveSessionSummary {
      live: LiveSessionId::new(),
      cwd: "/repo".into(),
      file: "/session".into(),
      exited: false,
    });
    let event = RpcAgentEvent::from_value(json!({
      "type":"retry_fallback_applied", "from":"primary", "to":"backup", "role":"main", "reason":"rate limited"
    }))
    .unwrap();
    view.apply(&EventBody::Agent(event));
    let notice = &view.notices[0];
    assert_eq!(notice.level, Some(NotifyType::Warning));
    assert_eq!(notice.origin, super::Origin::Run);
    assert_eq!(notice.text, "Fallback: primary → backup\nRole: main\nrate limited");
    let success = RpcAgentEvent::from_value(json!({
      "type":"retry_fallback_succeeded", "model":"backup", "role":"main"
    }))
    .unwrap();
    view.apply(&EventBody::Agent(success));
    let status = &view.notices[1];
    assert_eq!(status.level, None);
    assert_eq!(status.origin, super::Origin::Run);
    assert_eq!(status.text, "Fallback succeeded on backup\nRole: main");
  }

  #[test]
  fn recorded_todo_transitions_and_snapshots_replace_current_plan() {
    let events = events(include_str!("../fixtures/real_todo_transitions.events.json"));
    let before = state(include_str!("../fixtures/real_todo_transitions.state_before.json"));
    let blocked = state(include_str!("../fixtures/real_todo_transitions.state_blocked.json"));
    let completed = state(include_str!("../fixtures/real_todo_transitions.state_completed.json"));
    let mut view = view(&events);
    install(&mut view, &before);
    assert!(view.todo_phases.is_empty());
    for event in &events {
      view.apply(&event.body);
    }
    assert_eq!(view.todo_phases, completed.todo_phases);
    assert_eq!(view.todo_phases[0].tasks[2].blocker.as_deref(), Some("Waiting for an example"));
    install(&mut view, &blocked);
    assert_eq!(view.todo_phases, blocked.todo_phases);
    install(&mut view, &before);
    assert!(view.todo_phases.is_empty());
    let history: MessagesPage = serde_json::from_str(include_str!("../fixtures/real_todo_transitions.messages.json")).unwrap();
    view.load_page(history);
    assert!(view.todo_phases.is_empty());
  }

  #[test]
  fn failed_missing_and_malformed_tool_results_do_not_replace_plan() {
    let events = events(include_str!("../fixtures/real_todo_transitions.events.json"));
    let blocked = state(include_str!("../fixtures/real_todo_transitions.state_blocked.json"));
    let mut view = view(&events);
    install(&mut view, &blocked);
    for (is_error, result) in [
      (true, json!({"details":{"phases":[]}})),
      (false, json!({"details":{}})),
      (false, json!({"details":{"phases":"bad"}})),
    ] {
      let event = RpcAgentEvent::from_value(json!({
        "type":"tool_execution_end", "toolCallId":"constructed", "toolName":"todo", "isError":is_error, "result":result
      }))
      .unwrap();
      view.apply(&EventBody::Agent(event));
      assert_eq!(view.todo_phases, blocked.todo_phases);
    }
    let repeated = events.iter().find(|event| matches!(&event.body, EventBody::Agent(RpcAgentEvent::MessageEnd(end)) if matches!(&end.message, AgentMessage::ToolResult(result) if result.tool_name.as_deref() == Some("todo")))).unwrap();
    view.apply(&repeated.body);
    assert_eq!(view.todo_phases, blocked.todo_phases);
    let clear = RpcAgentEvent::from_value(json!({
      "type":"tool_execution_end", "toolCallId":"constructed", "toolName":"todo", "isError":false, "result":{"details":{"phases":[]}}
    }))
    .unwrap();
    view.apply(&EventBody::Agent(clear));
    assert!(view.todo_phases.is_empty());
  }

  #[test]
  fn reminders_do_not_replace_plan_and_clear_invalidates_snapshot() {
    let events = events(include_str!("../fixtures/real_todo_reminder_repeated.events.json"));
    let snapshot = state(include_str!("../fixtures/real_todo_reminder_repeated.state.json"));
    let mut view = view(&events);
    install(&mut view, &snapshot);
    for event in &events {
      if matches!(&event.body, EventBody::Agent(RpcAgentEvent::TodoReminder(_))) {
        view.apply(&event.body);
      }
    }
    assert_eq!(view.todo_phases, snapshot.todo_phases);
    assert_eq!(view.todo_reminder.as_ref().unwrap().attempt, 1);
    assert_eq!(view.todo_reminder.as_ref().unwrap().max_attempts, 3);
    let request = view.begin_state_request();
    let clear = RpcAgentEvent::from_value(json!({"type":"todo_auto_clear"})).unwrap();
    view.apply(&EventBody::Agent(clear));
    view.set_state(&snapshot, request);
    assert!(view.todo_phases.is_empty());
    assert!(view.todo_reminder.is_none());
    let old = view.begin_state_request();
    view.resync();
    assert!(view.state_stale);
    view.set_state(&snapshot, old);
    assert!(view.todo_phases.is_empty());
    assert!(view.state_stale);
    install(&mut view, &snapshot);
    assert_eq!(view.todo_phases, snapshot.todo_phases);
  }

  #[test]
  fn newer_todo_event_and_newer_request_win_over_stale_snapshots() {
    let events = events(include_str!("../fixtures/real_todo_transitions.events.json"));
    let before = state(include_str!("../fixtures/real_todo_transitions.state_before.json"));
    let blocked = state(include_str!("../fixtures/real_todo_transitions.state_blocked.json"));
    let mut view = view(&events);
    let old = view.begin_state_request();
    let new = view.begin_state_request();
    view.set_state(&blocked, new);
    view.set_state(&before, old);
    assert_eq!(view.todo_phases, blocked.todo_phases);
    let old = view.begin_state_request();
    let event = events
      .iter()
      .find(|event| matches!(&event.body, EventBody::Agent(RpcAgentEvent::ToolExecutionEnd(end)) if end.tool_name == "todo"))
      .unwrap();
    view.apply(&event.body);
    let after_event = view.todo_phases.clone();
    view.set_state(&before, old);
    assert_eq!(view.todo_phases, after_event);
  }

  #[test]
  fn irc_live_commit_and_history_use_same_payload_and_preserve_unmatched() {
    let events = events(include_str!("../fixtures/real_todo_transitions.events.json"));
    let mut view = view(&events);
    let first = irc(json!("one"), false);
    let second = irc(json!("two"), true);
    for message in [&first, &first, &second] {
      view.apply(&EventBody::Agent(RpcAgentEvent::IrcMessage(IrcMessageEvent {
        message: message.clone(),
      })));
    }
    assert_eq!(view.transcript.len(), 2);
    assert_eq!(view.transcript[0], AgentMessage::Custom(first.clone()));
    assert_eq!(first.display, Some(false));
    let committed = AgentMessage::Custom(first.clone());
    view.message_end(&MessageEndEvent {
      message: committed.clone(),
      message_id: Some("wire-id".into()),
    });
    assert_eq!(view.transcript, vec![committed.clone(), AgentMessage::Custom(second.clone())]);
    assert_eq!(view.message_ids[0].as_deref(), Some("wire-id"));
    view.resync();
    assert_eq!(view.transcript, vec![AgentMessage::Custom(second.clone())]);
    view.load_page(page(vec![committed.clone()]));
    assert_eq!(view.transcript, vec![committed, AgentMessage::Custom(second.clone())]);
    view.load_page(page(vec![AgentMessage::Custom(first), AgentMessage::Custom(second.clone())]));
    assert_eq!(view.transcript.len(), 2);
    view.irc_message(&second);
    view.message_end(&MessageEndEvent {
      message: AgentMessage::Custom(second),
      message_id: None,
    });
    assert_eq!(view.transcript.len(), 2);
  }

  #[test]
  fn irc_missing_or_invalid_ids_remain_distinct() {
    let events = events(include_str!("../fixtures/real_todo_transitions.events.json"));
    let mut view = view(&events);
    for id in [Value::Null, json!(""), json!(7)] {
      let message = irc(id, true);
      view.irc_message(&message);
      view.message_end(&MessageEndEvent {
        message: AgentMessage::Custom(message),
        message_id: None,
      });
    }
    let mut missing = irc(Value::Null, true);
    assert!(missing.details.as_mut().unwrap().as_object_mut().unwrap().remove("id").is_some());
    view.irc_message(&missing);
    view.irc_message(&missing);
    assert_eq!(view.transcript.len(), 8);
    view.resync();
    assert_eq!(view.transcript.len(), 5);
    view.load_page(page(Vec::new()));
    assert_eq!(view.transcript.len(), 5);
  }
}
