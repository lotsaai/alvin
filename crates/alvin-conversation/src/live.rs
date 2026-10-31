//! One live session as a view renders it, and the fold of its events.

use std::collections::BTreeMap;
use std::path::PathBuf;

use alvin_protocol::{
  AgentMessage, AutoRetryStartEvent, AvailableSlashCommand, EventBody, ExtensionUiRequest, LiveSessionId, LiveSessionSummary,
  MessageEndEvent, MessageStartEvent, MessageUpdateEvent, MessagesPage, ModelInfo, NotifyType, PromptStatus, RpcAgentEvent, SessionState,
  ThinkingLevel, WidgetPlacement,
};

use crate::dialogs::Dialogs;
use crate::queue::Queue;
use crate::streaming::Streaming;
use crate::tools::Tools;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

/// Something the user should be told, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
  /// How serious it is.
  pub level: Level,
  /// Where it came from.
  pub origin: Origin,
  /// What it says.
  pub text: String,
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
  /// How many messages the session has, as `live/messages` last reported.
  pub total_messages: usize,
  /// The assistant message being built.
  pub streaming: Option<Streaming>,
  /// Tool executions of the current turn.
  pub tools: Tools,
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
      total_messages: 0,
      streaming: None,
      tools: Tools::default(),
      run: Run::Idle,
      last_prompt: None,
      compaction: None,
      retry: None,
      notices: Vec::new(),
      title: None,
      exited: summary.exited,
      reload_needed: false,
      state_stale: false,
      queue: Queue::default(),
      dialogs: Dialogs::default(),
      commands: Vec::new(),
      model: None,
      thinking: None,
      statuses: BTreeMap::new(),
      widgets: BTreeMap::new(),
      editor_text: None,
    }
  }

  /// The text an extension asked to put in the composer, once.
  pub fn take_editor_text(&mut self) -> Option<String> {
    self.editor_text.take()
  }

  /// Replaces the transcript with a page of `live/messages` and clears the reload-needed mark.
  pub(crate) fn load_page(&mut self, page: MessagesPage) {
    self.total_messages = usize::try_from(page.total_messages).unwrap_or(0);
    self.transcript = page.messages;
    self.reload_needed = false;
  }

  pub(crate) fn set_state(&mut self, state: &SessionState) {
    self.model.clone_from(&state.model);
    self.thinking = state.thinking_level;
    self.state_stale = false;
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
    self.total_messages = 0;
    self.streaming = None;
    self.tools.clear();
    self.reload_needed = true;
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
      EventBody::Undecodable { error, .. } => self.notice(NotifyType::Warning, Origin::Undecodable, error.clone()),
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
        // A non-terminal end means a continuation or background work remains: the run goes on.
        if end.is_terminal != Some(false) && self.run != Run::Settled {
          self.run = Run::Idle;
        }
      }
      RpcAgentEvent::TurnEnd(_) => self.tools.clear(),
      RpcAgentEvent::MessageStart(start) => self.message_start(start),
      RpcAgentEvent::MessageUpdate(update) => self.message_update(update),
      RpcAgentEvent::MessageEnd(end) => self.message_end(end),
      RpcAgentEvent::ToolExecutionStart(start) => self.tools.start(start),
      RpcAgentEvent::ToolExecutionUpdate(update) => self.tools.update(update),
      RpcAgentEvent::ToolStreamUpdate(update) => self.tools.stream_update(update),
      RpcAgentEvent::ToolExecutionEnd(end) => self.tools.end(end),
      RpcAgentEvent::AutoCompactionStart(_) => self.compaction = Some(Compaction { manual: false }),
      RpcAgentEvent::AutoCompactionEnd(end) => {
        self.compaction = None;
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
        if changed.thinking_level.is_some() {
          self.thinking = changed.thinking_level;
        }
      }
      RpcAgentEvent::Notice(notice) => self.notice(notice.level, Origin::Notice(notice.source.clone()), notice.message.clone()),
      RpcAgentEvent::QueueUpdate(update) => self.queue.replace(update),
      // These change nothing a view shows: turn start (the run state follows `agent_start`), prompt-cache refreshes, retry
      // fallbacks, warnings about configuration, advisor cost, rule injection, todo and goal bookkeeping, IRC asides.
      RpcAgentEvent::TurnStart(_)
      | RpcAgentEvent::CacheWarmingStart(_)
      | RpcAgentEvent::CacheWarmingEnd(_)
      | RpcAgentEvent::RetryFallbackApplied(_)
      | RpcAgentEvent::RetryFallbackSucceeded(_)
      | RpcAgentEvent::ConfigWarningsChanged(_)
      | RpcAgentEvent::AdvisorCostChanged(_)
      | RpcAgentEvent::AdvisorYielded(_)
      | RpcAgentEvent::TtsrTriggered(_)
      | RpcAgentEvent::TodoReminder(_)
      | RpcAgentEvent::TodoAutoClear(_)
      | RpcAgentEvent::IrcMessage(_)
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
    let message_id = self.streaming.as_ref().and_then(|streaming| streaming.message_id.clone());
    self.streaming = Some(Streaming {
      message_id,
      message: update.message.clone(),
    });
  }

  fn message_end(&mut self, end: &MessageEndEvent) {
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
    self.notices.push(Notice { level, origin, text });
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
