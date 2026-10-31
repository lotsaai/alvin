//! One live omp process and the typed async methods that drive it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use alvin_protocol::{
  AbortAndRestoreQueueResult, BashResult, MessagesPage, ModelCycleResult, ModelInfo, PromoteQueuedMessageResult, PromptAck,
  QueuedMessageQueue, RemoveQueuedMessageResult, SessionState, ThinkingLevel, ThinkingLevelCycleResult,
};
use omp_rpc::{
  AbortAndPromptCommand, AbortAndRestoreQueueCommand, AbortBashCommand, AbortCommand, BashCommand, Client, Command, CycleModelCommand,
  CycleThinkingLevelCommand, FollowUpCommand, GetAvailableModelsCommand, GetAvailableThinkingLevelsCommand, GetMessagesPageCommand,
  GetStateCommand, PromoteQueuedMessageCommand, PromptCommand, RemoveQueuedMessageCommand, SetModelCommand, SetThinkingLevelCommand,
  SteerCommand,
};

use crate::translate::convert;
use crate::{Error, Result};

/// A running `omp --mode rpc-ui` process and its working directory. Dropping the handle tears the
/// process and its children down.
pub struct Session {
  cwd: PathBuf,
  pid: u32,
  client: Option<Arc<Client>>,
  closing: Arc<AtomicBool>,
  untranslated: Arc<AtomicU64>,
}

impl std::fmt::Debug for Session {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Session")
      .field("cwd", &self.cwd)
      .field("pid", &self.pid)
      .finish_non_exhaustive()
  }
}
impl Session {
  pub(crate) fn new(cwd: PathBuf, pid: u32, client: Client, closing: Arc<AtomicBool>, untranslated: Arc<AtomicU64>) -> Self {
    Self {
      cwd,
      pid,
      client: Some(Arc::new(client)),
      closing,
      untranslated,
    }
  }

  /// The working directory the session runs in.
  pub fn cwd(&self) -> &Path {
    &self.cwd
  }

  /// The omp process id. On Unix it is also the process group id.
  pub fn pid(&self) -> u32 {
    self.pid
  }

  /// How many frames the protocol does not expose have arrived so far.
  pub fn untranslated_count(&self) -> u64 {
    self.untranslated.load(Ordering::Relaxed)
  }

  /// Submits a prompt. omp expands `/` commands, skills, templates and `^model` itself. Returns once omp has admitted
  /// the prompt; its outcome arrives as a `PromptResult` event.
  pub async fn prompt(&self, message: String) -> Result<PromptAck> {
    let ack = self
      .call(PromptCommand {
        message,
        images: None,
        streaming_behavior: None,
      })
      .await?;
    convert("prompt acknowledgement", &ack)
  }

  /// Queues a message that interrupts the running turn.
  pub async fn steer(&self, message: String) -> Result<()> {
    self.call(SteerCommand { message, images: None }).await
  }

  /// Queues a message that is sent when the running turn finishes.
  pub async fn follow_up(&self, message: String) -> Result<()> {
    self.call(FollowUpCommand { message, images: None }).await
  }

  /// Aborts the running turn and sends `message` in its place. Its outcome arrives as a `PromptResult` event.
  pub async fn abort_and_prompt(&self, message: String) -> Result<()> {
    self.call(AbortAndPromptCommand { message, images: None }).await
  }

  /// Takes a message out of a queue; omp identifies it by its text.
  pub async fn remove_queued(&self, message: String, queue: QueuedMessageQueue) -> Result<RemoveQueuedMessageResult> {
    let queue = convert("queue name", &queue)?;
    let result = self.call(RemoveQueuedMessageCommand { message, queue }).await?;
    convert("remove result", &result)
  }

  /// Moves a queued follow-up to the steering queue.
  pub async fn promote_queued(&self, message: String) -> Result<PromoteQueuedMessageResult> {
    let result = self.call(PromoteQueuedMessageCommand { message }).await?;
    convert("promote result", &result)
  }

  /// Aborts the running turn and returns the queued messages that were withdrawn.
  pub async fn restore_queue(&self) -> Result<AbortAndRestoreQueueResult> {
    let result = self.call(AbortAndRestoreQueueCommand::default()).await?;
    convert("restored queue", &result)
  }

  /// The models omp can use.
  pub async fn models(&self) -> Result<Vec<ModelInfo>> {
    let models = self.call(GetAvailableModelsCommand::default()).await?;
    convert("model list", &models)
  }

  /// Switches to a model.
  pub async fn set_model(&self, provider: String, model_id: String) -> Result<ModelInfo> {
    let model = self.call(SetModelCommand { provider, model_id }).await?;
    convert("model", &model)
  }

  /// Switches to the next model in omp's cycle, or `None` when there is no other.
  pub async fn cycle_model(&self) -> Result<Option<ModelCycleResult>> {
    let result = self.call(CycleModelCommand::default()).await?;
    convert("model cycle", &result)
  }

  /// The thinking levels the current model accepts.
  pub async fn thinking_levels(&self) -> Result<Vec<ThinkingLevel>> {
    let levels = self.call(GetAvailableThinkingLevelsCommand::default()).await?;
    convert("thinking levels", &levels)
  }

  /// Sets the thinking level.
  pub async fn set_thinking(&self, level: ThinkingLevel) -> Result<()> {
    let level = convert("thinking level", &level)?;
    self.call(SetThinkingLevelCommand { level }).await
  }

  /// Switches to the next thinking level, or `None` when the model has none to cycle through.
  pub async fn cycle_thinking(&self) -> Result<Option<ThinkingLevelCycleResult>> {
    let result = self.call(CycleThinkingLevelCommand::default()).await?;
    convert("thinking cycle", &result)
  }

  /// Aborts the running turn.
  pub async fn abort(&self) -> Result<()> {
    self.call(AbortCommand::default()).await
  }

  /// Runs a shell command in the session and returns its output. Waits until the command ends, up to `omp-rpc`'s
  /// default command deadline.
  pub async fn bash(&self, command: String) -> Result<BashResult> {
    let result = self.call(BashCommand { command }).await?;
    convert("bash result", &result)
  }

  /// omp's state of the session: session file, model, thinking level, streaming and compacting flags.
  pub async fn state(&self) -> Result<SessionState> {
    let state = self.call(GetStateCommand::default()).await?;
    convert("session state", &state)
  }

  /// One page of the session's messages. `cursor` is the previous page's `next_cursor`.
  pub async fn messages(&self, cursor: Option<String>, limit: Option<i64>) -> Result<MessagesPage> {
    let page = self.call(GetMessagesPageCommand { cursor, limit }).await?;
    convert("messages page", &page)
  }

  /// Aborts the running `bash` command.
  pub async fn abort_bash(&self) -> Result<()> {
    self.call(AbortBashCommand::default()).await
  }

  /// Closes the session and confirms that omp and its children are gone. Fails with [`Error::CloseInFlight`] when a
  /// command is still running; the teardown then finishes when it returns.
  pub async fn close(mut self) -> Result<()> {
    self.closing.store(true, Ordering::SeqCst);
    let client = self
      .client
      .take()
      .ok_or_else(|| Error::from_rpc(&self.cwd, "close", omp_rpc::Error::Closed))?;
    let cwd = self.cwd.clone();
    tokio::task::spawn_blocking(move || teardown(cwd, client)).await?
  }

  async fn call<C>(&self, command: C) -> Result<C::Output>
  where
    C: Command + Send + 'static,
    C::Output: Send + 'static,
  {
    let client = self
      .client
      .clone()
      .ok_or_else(|| Error::from_rpc(&self.cwd, C::NAME, omp_rpc::Error::Closed))?;
    let outcome = tokio::task::spawn_blocking(move || client.call(&command)).await?;
    outcome.map_err(|source| Error::from_rpc(&self.cwd, C::NAME, source))
  }
}

impl Drop for Session {
  fn drop(&mut self) {
    self.closing.store(true, Ordering::SeqCst);
    let Some(client) = self.client.take() else {
      return;
    };
    let cwd = self.cwd.clone();
    let job = move || {
      if let Err(error) = teardown(cwd, client) {
        tracing::warn!(%error, "tearing down a dropped omp session");
      }
    };
    // Tearing down waits for omp to exit, so it never runs on a runtime worker.
    match tokio::runtime::Handle::try_current() {
      Ok(runtime) => drop(runtime.spawn_blocking(job)),
      Err(error) => {
        tracing::trace!(%error, "dropped outside a runtime; tearing down inline");
        job();
      }
    }
  }
}

/// How long teardown waits for commands in flight to return after aborting them.
const IN_FLIGHT_GRACE: Duration = Duration::from_secs(5);

/// Stops running work, then closes the client once nothing else holds it: SIGTERM then SIGKILL to omp's process group
/// on Unix.
///
/// omp runs every command in a process group of its own, so signalling omp's group cannot reach a running command's
/// processes. omp kills its own children when told to abort, so abort first. Processes a command left running in the
/// background after it returned are out of reach here.
fn teardown(cwd: PathBuf, client: Arc<Client>) -> Result<()> {
  abort_running_work(&cwd, &client);
  let deadline = Instant::now() + IN_FLIGHT_GRACE;
  while Arc::strong_count(&client) > 1 && Instant::now() < deadline {
    std::thread::sleep(Duration::from_millis(10));
  }
  match Arc::try_unwrap(client) {
    Ok(client) => client.close().map_err(|source| Error::Teardown {
      cwd,
      source: Box::new(source),
    }),
    Err(client) => {
      drop(client);
      Err(Error::CloseInFlight { cwd })
    }
  }
}

fn abort_running_work(cwd: &Path, client: &Client) {
  log_abort(cwd, AbortBashCommand::NAME, client.call(&AbortBashCommand::default()));
  log_abort(cwd, AbortCommand::NAME, client.call(&AbortCommand::default()));
}

fn log_abort(cwd: &Path, command: &'static str, outcome: std::result::Result<(), omp_rpc::Error>) {
  match outcome {
    // The process already ended: nothing left to abort.
    Ok(()) | Err(omp_rpc::Error::Closed) => {}
    Err(error) => tracing::warn!(cwd = %cwd.display(), command, %error, "aborting before teardown failed"),
  }
}

#[cfg(test)]
mod tests {
  use std::time::Duration;

  use alvin_protocol::{AssistantMessageEvent, EventBody, PromptStatus, RpcAgentEvent};

  use crate::Error;
  use crate::test_support::{Fixture, PATIENCE, alive, gone, next, pid_in, prompt_status, sleeper, streamed_text, until_prompt_result};

  #[tokio::test]
  async fn a_prompt_streams_text_deltas_in_order_and_ends_with_its_result() {
    let fixture = Fixture::new();
    let (session, mut events) = fixture.start().await;

    let ack = session.prompt("Reply with the single word ok.".to_owned()).await.unwrap();
    let seen = until_prompt_result(&mut events).await;

    assert_eq!(ack.agent_invoked, None);
    let position = |wanted: fn(&EventBody) -> bool| seen.iter().position(wanted).unwrap();
    let start = position(|event| matches!(event, EventBody::Agent(RpcAgentEvent::AgentStart(_))));
    let delta = position(
      |event| matches!(event, EventBody::Agent(RpcAgentEvent::MessageUpdate(update)) if matches!(update.assistant_message_event, AssistantMessageEvent::TextDelta(_))),
    );
    let end = position(|event| matches!(event, EventBody::Agent(RpcAgentEvent::AgentEnd(_))));
    assert!(start < delta && delta < end, "start {start}, delta {delta}, end {end}");
    assert_eq!(seen.last().map(|event| matches!(event, EventBody::PromptResult(_))), Some(true));
    assert_eq!(prompt_status(&seen), Some(PromptStatus::Completed));
    let (streamed, ended) = streamed_text(&seen);
    assert!(!streamed.is_empty());
    assert_eq!(Some(streamed), ended);
    session.close().await.unwrap();
  }

  #[tokio::test]
  async fn abort_stops_a_running_turn() {
    let fixture = Fixture::new();
    let (session, mut events) = fixture.start().await;

    session
      .prompt("Write the numbers from 1 to 400, one per line, nothing else.".to_owned())
      .await
      .unwrap();
    loop {
      if matches!(next(&mut events).await, EventBody::Agent(RpcAgentEvent::MessageUpdate(_))) {
        break;
      }
    }
    session.abort().await.unwrap();
    let seen = until_prompt_result(&mut events).await;

    assert_eq!(prompt_status(&seen), Some(PromptStatus::Aborted));
    session.close().await.unwrap();
  }

  #[tokio::test]
  async fn bash_returns_the_commands_output() {
    let fixture = Fixture::new();
    let (session, _events) = fixture.start().await;

    let result = session.bash("echo alvin-bash-ok".to_owned()).await.unwrap();

    assert!(result.output.contains("alvin-bash-ok"), "{result:?}");
    assert!(!result.cancelled);
    session.close().await.unwrap();
  }

  /// Starts `sleeper` as a running foreground command and returns its process id. The call's future is dropped, as a
  /// cancelled request would be, so its blocking task keeps waiting while the test closes the session.
  async fn running_child(session: &super::Session, fixture: &Fixture) -> u32 {
    let file = fixture.cwd.path().join("child.pid");
    let cancelled = tokio::time::timeout(Duration::from_millis(500), session.bash(sleeper(&file))).await;
    assert!(cancelled.is_err());
    pid_in(&file).await
  }

  #[tokio::test]
  async fn close_leaves_neither_omp_nor_a_running_command() {
    let fixture = Fixture::new();
    let (session, _events) = fixture.start().await;
    let omp = session.pid();
    let child = running_child(&session, &fixture).await;
    assert!(alive(omp) && alive(child));

    session.close().await.unwrap();

    assert!(!alive(omp), "omp {omp} survived close");
    assert!(!alive(child), "child {child} survived close");
  }

  #[tokio::test]
  async fn dropping_the_handle_leaves_neither_omp_nor_a_running_command() {
    let fixture = Fixture::new();
    let (session, _events) = fixture.start().await;
    let omp = session.pid();
    let child = running_child(&session, &fixture).await;
    assert!(alive(omp) && alive(child));

    drop(session);

    gone(omp).await;
    gone(child).await;
  }

  #[tokio::test]
  async fn a_killed_omp_yields_a_process_exited_event_and_a_typed_error() {
    let fixture = Fixture::new();
    let (session, mut events) = fixture.start().await;

    let status = std::process::Command::new("kill")
      .args(["-9", &session.pid().to_string()])
      .status()
      .unwrap();
    assert!(status.success());

    loop {
      match next(&mut events).await {
        EventBody::ProcessExited => break,
        other => assert!(!matches!(other, EventBody::PromptResult(_)), "{other:?}"),
      }
    }
    let error = session.abort().await.unwrap_err();
    assert!(matches!(error, Error::ProcessExited { .. }), "{error:?}");
  }

  #[tokio::test]
  async fn a_slow_consumer_loses_no_events() {
    let fixture = Fixture::with_capacity(1);
    let (session, mut events) = fixture.start().await;

    session
      .prompt("Write the numbers from 1 to 60, one per line, nothing else.".to_owned())
      .await
      .unwrap();
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
      let event = tokio::time::timeout_at(deadline, events.recv()).await.unwrap().unwrap();
      let done = matches!(event, EventBody::PromptResult(_));
      seen.push(event);
      if done {
        break;
      }
      tokio::time::sleep(Duration::from_millis(15)).await;
    }

    let (streamed, ended) = streamed_text(&seen);
    assert!(streamed.contains("60"), "{streamed:?}");
    assert_eq!(Some(streamed), ended);
    session.close().await.unwrap();
  }
}
