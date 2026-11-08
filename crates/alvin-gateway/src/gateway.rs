//! The gateway: shared state behind every protocol method.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use alvin_omp_host::{Host, StartRequest};
use alvin_protocol::{
  AbortAndRestoreQueueResult, AvailableSlashCommand, BashRequest, BashResult, BranchMessage, BranchResult, CancellationResult,
  CompactRequest, CompactionResult, EntriesRequest, EntryRequest, ExportRequest, ForkRequest, HistoryEntries, LabelRequest, ListSessions,
  LiveSessionId, LiveSessionSummary, MessagesPage, MessagesRequest, ModelCycleResult, ModelInfo, NavigateRequest, NavigateTreeResult,
  OpenRequest, OpenTarget, ProjectSummary, PromoteQueuedMessageResult, PromoteQueuedRequest, PromptRequest, RemoveQueuedMessageResult,
  RemoveQueuedRequest, RenameRequest, RespondRequest, SessionPage, SessionState, SessionStats, SetModelRequest, SetThinkingRequest,
  SubscribeRequest, ThinkingLevel, ThinkingLevelCycleResult,
};
use tokio::sync::broadcast;

use crate::live::{self, EventLog, Live, LiveSessions};
use crate::telemetry::Metrics;
use crate::terminal::Terminals;
use crate::{Error, GatewayConfig, Result};

/// The application boundary. Cheap to clone; every transport serves the same methods from one instance.
#[derive(Debug, Clone)]
pub struct Gateway {
  config: Arc<GatewayConfig>,
  host: Host,
  live: Arc<LiveSessions>,
  pub(crate) terminals: Arc<Terminals>,
  pub(crate) metrics: Metrics,
}

impl Gateway {
  /// Creates a gateway from its configuration. Initialize [`alvin_telemetry::Telemetry`] first to export its metrics.
  pub fn new(config: GatewayConfig) -> Self {
    Self {
      host: Host::new(config.omp.clone()),
      config: Arc::new(config),
      live: Arc::default(),
      terminals: Arc::default(),
      metrics: Metrics::new(),
    }
  }

  /// Closes every live session and waits until their omp processes are gone. Every session is closed even when one
  /// fails; the first failure is returned.
  pub async fn shutdown(&self) -> Result<()> {
    let mut first = None;
    let terminals = {
      let _opening = self.terminals.open_lock.lock().await;
      self.terminals.stop();
      self.terminals.drain()
    };
    for terminal in terminals {
      if let Err(error) = terminal.close().await {
        first.get_or_insert(error);
      }
    }
    for live in self.live.drain() {
      self.metrics.live_sessions(-1);
      if let Err(error) = live::close(live).await {
        tracing::error!(%error, "closing a live session at shutdown");
        first.get_or_insert(error);
      }
    }
    first.map_or(Ok(()), Err)
  }

  /// Every project with sessions. A session file that cannot be read fails the listing.
  pub(crate) async fn projects(&self) -> Result<Vec<ProjectSummary>> {
    let config = Arc::clone(&self.config);
    tokio::task::spawn_blocking(move || {
      let heads = alvin_session_index::scan(&config.sessions_dir)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
      Ok(alvin_session_index::group(heads.into_iter().map(|head| head.cwd))?)
    })
    .await
    .map_err(Error::from)?
  }

  /// One page of a project's sessions. Validates the request here, once: the page is not empty and the project (and
  /// checkout) exist.
  pub(crate) async fn sessions(&self, request: ListSessions) -> Result<SessionPage> {
    let limit = usize::try_from(request.limit)
      .ok()
      .filter(|limit| *limit > 0)
      .ok_or(Error::EmptyPage)?;
    let config = Arc::clone(&self.config);
    tokio::task::spawn_blocking(move || {
      let heads = alvin_session_index::scan(&config.sessions_dir)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
      let selected = alvin_session_index::select(heads, &request.project, request.checkout.as_ref())?;
      if selected.is_empty() {
        return Err(Error::UnknownProject {
          project: request.project,
          checkout: request.checkout,
        });
      }
      Ok(alvin_session_index::list_page(selected, request.after.as_ref(), limit)?)
    })
    .await
    .map_err(Error::from)?
  }

  /// Starts omp on a new or existing session, or returns the live session an existing one is already in. Validates the
  /// target once: the session or project exists and its directory is there.
  pub(crate) async fn open(&self, request: OpenRequest) -> Result<LiveSessionSummary> {
    // Starting omp takes seconds; two opens of one file must not both start a process.
    let _opening = self.live.open_lock.lock().await;
    let (cwd, resume) = self.resolve(&request.target).await?;
    if let Some(file) = &resume
      && let Some(existing) = self.live.by_file(file)
    {
      return Ok(existing.summary());
    }
    let model = request.model.or_else(|| self.config.default_model.clone());
    let start_cwd = if resume.is_none() { dunce::simplified(&cwd) } else { &cwd };
    let start = StartRequest::builder()
      .cwd(start_cwd.to_path_buf())
      .model(model)
      .resume(resume.clone())
      .build();
    let (session, events) = self.timed("open", self.host.start(start)).await?;
    let state = self.timed("get_state", session.state()).await?;
    let file = state
      .session_file
      .map(PathBuf::from)
      .or(resume)
      .ok_or_else(|| Error::NoSessionFile { cwd: cwd.clone() })?;
    let file = live::canonical_file(&file)?;
    let id = LiveSessionId::new();
    let log = EventLog::new(id, self.metrics.clone());
    tokio::spawn(live::pump(Arc::clone(&log), events));
    let live = Arc::new(Live::new(id, cwd, file, session, log));
    let summary = live.summary();
    self.live.insert(live);
    self.metrics.live_sessions(1);
    Ok(summary)
  }

  /// The working directory and, for an existing session, the file to resume.
  pub(crate) async fn resolve(&self, target: &OpenTarget) -> Result<(PathBuf, Option<PathBuf>)> {
    let (cwd, resume) = match target {
      OpenTarget::Session { session } => {
        let config = Arc::clone(&self.config);
        let wanted = session.clone();
        let head = tokio::task::spawn_blocking(move || {
          let heads = alvin_session_index::scan(&config.sessions_dir)?
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
          Ok::<_, Error>(heads.into_iter().find(|head| head.id == wanted))
        })
        .await??
        .ok_or_else(|| Error::UnknownSession { session: session.clone() })?;
        (head.cwd, Some(live::canonical_file(&head.path)?))
      }
      OpenTarget::New { project, checkout } => {
        let projects = self.projects().await?;
        let root = projects
          .iter()
          .find(|candidate| candidate.id == *project)
          .and_then(|candidate| candidate.checkouts.iter().find(|candidate| candidate.id == *checkout))
          .map(|checkout| checkout.root.clone())
          .ok_or_else(|| Error::UnknownProject {
            project: project.clone(),
            checkout: Some(checkout.clone()),
          })?;
        (root, None)
      }
    };
    if !tokio::fs::metadata(&cwd).await.is_ok_and(|metadata| metadata.is_dir()) {
      return Err(Error::MissingDirectory { path: cwd });
    }
    Ok((cwd, resume))
  }

  /// Closes a live session and removes it from the registry.
  pub(crate) async fn close(&self, id: LiveSessionId) -> Result<()> {
    let live = self.live.remove(id)?;
    self.metrics.live_sessions(-1);
    live::close(live).await
  }

  pub(crate) fn list_live(&self) -> Vec<LiveSessionSummary> {
    self.live.list()
  }

  pub(crate) async fn state(&self, id: LiveSessionId) -> Result<SessionState> {
    let live = self.live.get(id)?;
    self.timed("get_state", live.session.state()).await
  }

  pub(crate) async fn messages(&self, request: MessagesRequest) -> Result<MessagesPage> {
    let limit = request
      .limit
      .map(|limit| i64::from(limit).clamp(0, i64::MAX))
      .map(|limit| if limit == 0 { Err(Error::EmptyPage) } else { Ok(limit) })
      .transpose()?;
    let live = self.live.get(request.live)?;
    self.timed("get_messages_page", live.session.messages(request.cursor, limit)).await
  }

  pub(crate) async fn prompt(&self, request: PromptRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("prompt", live.session.prompt(request.message)).await.map(drop)
  }

  pub(crate) async fn steer(&self, request: PromptRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("steer", live.session.steer(request.message)).await
  }

  pub(crate) async fn follow_up(&self, request: PromptRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("follow_up", live.session.follow_up(request.message)).await
  }

  pub(crate) async fn abort_and_prompt(&self, request: PromptRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("abort_and_prompt", live.session.abort_and_prompt(request.message)).await
  }

  pub(crate) async fn remove_queued(&self, request: RemoveQueuedRequest) -> Result<RemoveQueuedMessageResult> {
    let live = self.live.get(request.live)?;
    self
      .timed("remove_queued_message", live.session.remove_queued(request.message, request.queue))
      .await
  }

  pub(crate) async fn promote_queued(&self, request: PromoteQueuedRequest) -> Result<PromoteQueuedMessageResult> {
    let live = self.live.get(request.live)?;
    self
      .timed("promote_queued_message", live.session.promote_queued(request.message))
      .await
  }

  pub(crate) async fn restore_queue(&self, id: LiveSessionId) -> Result<AbortAndRestoreQueueResult> {
    let live = self.live.get(id)?;
    self.timed("abort_and_restore_queue", live.session.restore_queue()).await
  }

  pub(crate) async fn respond(&self, request: RespondRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("extension_ui_response", live.session.respond(request.response)).await
  }

  pub(crate) async fn entries(&self, request: EntriesRequest) -> Result<HistoryEntries> {
    let live = self.live.get(request.live)?;
    self.timed("get_entries", live.session.entries(request.since)).await
  }

  pub(crate) async fn rename(&self, request: RenameRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("set_session_name", live.session.rename(request.name)).await
  }

  pub(crate) async fn compact(&self, request: CompactRequest) -> Result<CompactionResult> {
    let live = self.live.get(request.live)?;
    self.timed("compact", live.session.compact(request.custom_instructions)).await
  }

  pub(crate) async fn stats(&self, id: LiveSessionId) -> Result<SessionStats> {
    let live = self.live.get(id)?;
    self.timed("get_session_stats", live.session.stats()).await
  }

  pub(crate) async fn export_html(&self, request: ExportRequest) -> Result<String> {
    let live = self.live.get(request.live)?;
    self.timed("export_html", live.session.export_html(request.output_path)).await
  }

  pub(crate) async fn commands(&self, id: LiveSessionId) -> Result<Vec<AvailableSlashCommand>> {
    let live = self.live.get(id)?;
    self.timed("get_available_commands", live.session.commands()).await
  }

  pub(crate) async fn branch_messages(&self, id: LiveSessionId) -> Result<Vec<BranchMessage>> {
    let live = self.live.get(id)?;
    self.timed("get_branch_messages", live.session.branch_messages()).await
  }

  pub(crate) async fn navigate_tree(&self, request: NavigateRequest) -> Result<NavigateTreeResult> {
    let live = self.live.get(request.live)?;
    self
      .timed(
        "navigate_tree",
        live
          .session
          .navigate_tree(request.entry, request.summarize, request.custom_instructions),
      )
      .await
  }

  pub(crate) async fn abort_branch_summary(&self, id: LiveSessionId) -> Result<()> {
    let live = self.live.get(id)?;
    self.timed("abort_branch_summary", live.session.abort_branch_summary()).await
  }

  pub(crate) async fn branch(&self, request: EntryRequest) -> Result<BranchResult> {
    let live = self.live.get(request.live)?;
    let result = self.timed("branch", live.session.branch(request.entry)).await?;
    if !result.cancelled {
      self.follow_session_file(&live).await?;
    }
    Ok(result)
  }

  pub(crate) async fn fork(&self, request: ForkRequest) -> Result<CancellationResult> {
    let live = self.live.get(request.live)?;
    let result = self.timed("fork", live.session.fork(request.entry)).await?;
    if !result.cancelled {
      self.follow_session_file(&live).await?;
    }
    Ok(result)
  }

  pub(crate) async fn label(&self, request: LabelRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("set_label", live.session.set_label(request.entry, request.label)).await
  }

  /// A fork or branch can move omp onto another session file. Reads which one and files the live session under it, so an
  /// open of either file finds the right process.
  async fn follow_session_file(&self, live: &Live) -> Result<()> {
    let state = self.timed("get_state", live.session.state()).await?;
    let file = state
      .session_file
      .map(PathBuf::from)
      .ok_or_else(|| Error::NoSessionFile { cwd: live.cwd.clone() })?;
    let file = live::canonical_file(&file)?;
    if file != live.file() {
      self.live.rekey(live, file);
    }
    Ok(())
  }

  pub(crate) async fn models(&self, id: LiveSessionId) -> Result<Vec<ModelInfo>> {
    let live = self.live.get(id)?;
    self.timed("get_available_models", live.session.models()).await
  }

  pub(crate) async fn set_model(&self, request: SetModelRequest) -> Result<ModelInfo> {
    let live = self.live.get(request.live)?;
    self
      .timed("set_model", live.session.set_model(request.provider, request.model_id))
      .await
  }

  pub(crate) async fn cycle_model(&self, id: LiveSessionId) -> Result<Option<ModelCycleResult>> {
    let live = self.live.get(id)?;
    self.timed("cycle_model", live.session.cycle_model()).await
  }

  pub(crate) async fn thinking_levels(&self, id: LiveSessionId) -> Result<Vec<ThinkingLevel>> {
    let live = self.live.get(id)?;
    self.timed("get_available_thinking_levels", live.session.thinking_levels()).await
  }

  pub(crate) async fn set_thinking(&self, request: SetThinkingRequest) -> Result<()> {
    let live = self.live.get(request.live)?;
    self.timed("set_thinking_level", live.session.set_thinking(request.level)).await
  }

  pub(crate) async fn cycle_thinking(&self, id: LiveSessionId) -> Result<Option<ThinkingLevelCycleResult>> {
    let live = self.live.get(id)?;
    self.timed("cycle_thinking_level", live.session.cycle_thinking()).await
  }

  pub(crate) async fn abort(&self, id: LiveSessionId) -> Result<()> {
    let live = self.live.get(id)?;
    self.timed("abort", live.session.abort()).await
  }

  pub(crate) async fn bash(&self, request: BashRequest) -> Result<BashResult> {
    let live = self.live.get(request.live)?;
    self.timed("bash", live.session.bash(request.command)).await
  }

  pub(crate) async fn abort_bash(&self, id: LiveSessionId) -> Result<()> {
    let live = self.live.get(id)?;
    self.timed("abort_bash", live.session.abort_bash()).await
  }

  /// The retained events newer than the cursor and a receiver for later ones.
  pub(crate) fn subscribe(
    &self,
    request: &SubscribeRequest,
  ) -> Result<(
    Vec<alvin_protocol::Event>,
    broadcast::Receiver<alvin_protocol::Event>,
    Arc<EventLog>,
  )> {
    let live = self.live.get(request.live)?;
    let (replay, receiver) = live.log.subscribe(request.after)?;
    Ok((replay, receiver, Arc::clone(&live.log)))
  }

  /// Runs one command against omp and records `alvin.omp.command.duration`.
  async fn timed<T>(&self, command: &'static str, call: impl Future<Output = alvin_omp_host::Result<T>>) -> Result<T> {
    let started = Instant::now();
    let result = call.await;
    self
      .metrics
      .record_omp_command(command, if result.is_ok() { "ok" } else { "error" }, started.elapsed());
    result.map_err(Error::from)
  }
}
