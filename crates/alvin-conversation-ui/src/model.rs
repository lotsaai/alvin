//! The one owner of domain state: wraps `alvin_conversation` state, issues commands through the client, and runs one
//! event task per live session. Views read it and call its methods; effects arrive back as events.

use std::collections::{BTreeMap, BTreeSet};

use alvin_client::{Client, ConnectionState, Error, ResyncReason};
use alvin_conversation::{Connection, Conversation, Notice, Run, SessionsKey};
use alvin_protocol::{
  BashRequest, CheckoutId, CompactRequest, DialogApiClient, EntriesRequest, EntryId, EntryRequest, ErrorCode, ErrorData, ExportRequest,
  ExtensionUiResponse, ForkRequest, LabelRequest, ListSessions, LiveApiClient, LiveSessionId, LiveSessionSummary, MessagesPage,
  MessagesRequest, ModelApiClient, ModelInfo, NavigateRequest, OpenRequest, OpenTarget, ProjectId, ProjectsApiClient, PromoteQueuedRequest,
  PromptRequest, QueueApiClient, QueuedMessageQueue, RemoveQueuedRequest, RenameRequest, RespondRequest, SessionCursor, SessionId,
  SessionStats, SessionsApiClient, SetModelRequest, SetThinkingRequest, SubscribeRequest, ThinkingLevel,
};
use gpui_kit::{AsyncApp, Context, EventEmitter, SharedString, Task, WeakEntity};

/// How many sessions one `sessions/list` page asks for.
const SESSIONS_PAGE: u32 = 50;

/// What the model tells its views beyond "something changed".
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
  /// A command failed. The code is the gateway's stable one when the failure came from it.
  Failed {
    /// The gateway's error code, when there is one.
    code: Option<ErrorCode>,
    /// What went wrong, for the user.
    message: SharedString,
  },
  /// A live session was opened and is tracked.
  Opened(LiveSessionId),
  /// A live session was closed.
  Closed(LiveSessionId),
  /// Text for a session's composer: queued messages an abort withdrew (one per line), or what an extension asked to put
  /// there.
  ComposerText {
    /// The live session.
    live: LiveSessionId,
    /// The text.
    text: String,
  },
  /// A session was written as an HTML page.
  Exported {
    /// The live session.
    live: LiveSessionId,
    /// Where omp wrote it.
    path: String,
  },
  /// An extension or omp has something to tell the user.
  Notice {
    /// The live session.
    live: LiveSessionId,
    /// What to tell.
    notice: Notice,
  },
}

/// What a live session can be switched to: omp's models, and the thinking levels the current model accepts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Catalog {
  /// The models omp can use.
  pub(crate) models: Vec<ModelInfo>,
  /// The thinking levels the current model accepts.
  pub(crate) levels: Vec<ThinkingLevel>,
}

/// Projects, sessions and live sessions, and the commands that change them.
pub struct Model {
  catalogs: BTreeMap<LiveSessionId, Catalog>,
  /// Sessions whose history is being read, and those asked to be read again meanwhile. Triggers coalesce into one request.
  tree_reads: BTreeMap<LiveSessionId, bool>,
  /// The user messages each session can branch from, as omp lists them, by entry id.
  branch_candidates: BTreeMap<LiveSessionId, BTreeSet<EntryId>>,
  /// Sessions whose navigation is generating a branch summary, which can be cancelled.
  summarizing: BTreeSet<LiveSessionId>,
  /// The counts omp last gave for each session.
  stats: BTreeMap<LiveSessionId, SessionStats>,
  conversation: Conversation,
  client: Client,
  /// Live sessions with a `!` command running, so Escape aborts that and not the turn.
  bash_running: BTreeSet<LiveSessionId>,
  /// One task per live session: loads it, then folds its events. Dropping a task stops it.
  sessions: BTreeMap<LiveSessionId, Task<()>>,
  _connection: Task<()>,
}

impl EventEmitter<ModelEvent> for Model {}

impl Model {
  /// A model over a connected client.
  pub fn new(client: Client, cx: &mut Context<Self>) -> Self {
    let mut states = client.watch_connection();
    let connection = cx.spawn(async move |this, cx| {
      loop {
        let state = *states.borrow_and_update();
        let alive = this
          .update(cx, |model, cx| {
            model.conversation.set_connection(match state {
              ConnectionState::Connected => Connection::Connected,
              ConnectionState::Reconnecting { attempt } => Connection::Reconnecting { attempt },
              ConnectionState::Lost => Connection::Lost,
            });
            cx.notify();
          })
          .is_ok();
        if !alive || states.changed().await.is_err() {
          return;
        }
      }
    });
    Self {
      catalogs: BTreeMap::new(),
      tree_reads: BTreeMap::new(),
      branch_candidates: BTreeMap::new(),
      summarizing: BTreeSet::new(),
      stats: BTreeMap::new(),
      conversation: Conversation::default(),
      client,
      bash_running: BTreeSet::new(),
      sessions: BTreeMap::new(),
      _connection: connection,
    }
  }

  /// The state views render.
  pub fn conversation(&self) -> &Conversation {
    &self.conversation
  }

  /// The models and thinking levels of a session, once loaded.
  pub(crate) fn catalog(&self, live: LiveSessionId) -> Option<&Catalog> {
    self.catalogs.get(&live)
  }

  /// Whether an entry is a user message omp can branch from.
  pub fn can_branch_from(&self, live: LiveSessionId, entry: &EntryId) -> bool {
    self
      .branch_candidates
      .get(&live)
      .is_some_and(|candidates| candidates.contains(entry))
  }

  /// The counts of a session, once read.
  pub fn stats(&self, live: LiveSessionId) -> Option<&SessionStats> {
    self.stats.get(&live)
  }

  /// Whether a navigation is generating a branch summary.
  pub fn summarizing(&self, live: LiveSessionId) -> bool {
    self.summarizing.contains(&live)
  }

  /// Whether a `!` command is running in the session.
  pub fn bash_running(&self, live: LiveSessionId) -> bool {
    self.bash_running.contains(&live)
  }

  /// Loads the projects and the first page of every checkout's sessions.
  pub fn refresh_projects(&mut self, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let projects = ProjectsApiClient::list(&client).await.map_err(Error::from);
      let keys = match projects {
        Ok(projects) => {
          let keys: Vec<SessionsKey> = projects
            .iter()
            .flat_map(|project| {
              project.checkouts.iter().map(|checkout| SessionsKey {
                project: project.id.clone(),
                checkout: Some(checkout.id.clone()),
              })
            })
            .collect();
          if this
            .update(cx, |model, cx| {
              model.conversation.set_projects(projects);
              cx.notify();
            })
            .is_err()
          {
            return;
          }
          keys
        }
        Err(error) => {
          fail(&this, cx, &error);
          return;
        }
      };
      let mut loads = Vec::new();
      for key in keys {
        match this.update(cx, |model, cx| model.load_sessions(key, None, cx)) {
          Ok(load) => loads.push(load),
          Err(_) => return,
        }
      }
      for load in loads {
        load.await;
      }
    })
  }

  /// Loads a page of a checkout's sessions, newest first. Without a cursor it is the first page and replaces the list.
  pub fn load_sessions(&mut self, key: SessionsKey, after: Option<SessionCursor>, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let request = ListSessions {
        project: key.project.clone(),
        checkout: key.checkout.clone(),
        after: after.clone(),
        limit: SESSIONS_PAGE,
      };
      match SessionsApiClient::list(&client, request).await.map_err(Error::from) {
        Ok(page) => {
          if this
            .update(cx, |model, cx| {
              model.conversation.load_sessions(key, after.as_ref(), page);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
  }

  /// Opens a listed session in a live session, or the live session it is already in.
  pub fn open_session(&mut self, session: SessionId, cx: &mut Context<Self>) -> Task<()> {
    self.open(OpenTarget::Session { session }, cx)
  }

  /// Opens a new session in a checkout of a project.
  pub fn new_session(&mut self, project: ProjectId, checkout: CheckoutId, cx: &mut Context<Self>) -> Task<()> {
    self.open(OpenTarget::New { project, checkout }, cx)
  }

  fn open(&mut self, target: OpenTarget, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let opened = LiveApiClient::open(&client, OpenRequest { target, model: None })
        .await
        .map_err(Error::from);
      match opened {
        Ok(summary) => {
          if this.update(cx, |model, cx| model.opened(summary, cx)).is_err() {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
  }

  fn opened(&mut self, summary: LiveSessionSummary, cx: &mut Context<Self>) {
    let live = summary.live;
    self.conversation.open(&summary);
    if !self.sessions.contains_key(&live) {
      let task = self.attach(live, cx);
      self.sessions.insert(live, task);
    }
    cx.emit(ModelEvent::Opened(live));
    cx.notify();
  }

  /// Loads a live session's state and transcript, then folds its events until it is closed. A stream that cannot
  /// continue without a gap (`Resync`) drops the transcript and starts over.
  fn attach(&self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      loop {
        match load(&client, live).await {
          Ok((state, page)) => {
            let loaded = this.update(cx, |model, cx| {
              model.conversation.set_state(live, &state);
              model.conversation.load_page(live, page);
              model.load_catalog(live, cx);
              model.refresh_tree(live, cx);
              cx.notify();
            });
            if loaded.is_err() {
              return;
            }
          }
          Err(error) => {
            fail(&this, cx, &error);
            return;
          }
        }
        let mut stream = match client.events(SubscribeRequest { live, after: None }).await {
          Ok(stream) => stream,
          Err(error) => {
            fail(&this, cx, &error);
            return;
          }
        };
        let resync = loop {
          match stream.next().await {
            Some(Ok(event)) => {
              if this
                .update(cx, |model, cx| {
                  let seen = model.conversation.live(live).map_or(0, |view| view.notices.len());
                  model.conversation.apply(&event);
                  model.announce(live, seen, cx);
                  if model.conversation.live(live).is_some_and(|view| view.tree_stale) {
                    model.refresh_tree(live, cx);
                  }
                  // omp said the model changed without saying to what.
                  if model.conversation.live(live).is_some_and(|view| view.state_stale) {
                    model.refresh_state(live, cx);
                  }
                  cx.notify();
                })
                .is_err()
              {
                return;
              }
            }
            Some(Err(Error::Resync(reason))) => {
              break Some(reason);
            }
            Some(Err(error)) => {
              fail(&this, cx, &error);
              return;
            }
            None => {
              break None;
            }
          }
        };
        match resync {
          Some(ResyncReason::StaleCursor) => {
            if this
              .update(cx, |model, cx| {
                model.conversation.resync(live);
                cx.notify();
              })
              .is_err()
            {
              return;
            }
          }
          // The live session is gone or the client is lost: nothing to reload.
          Some(ResyncReason::UnknownLiveSession) | None => return,
        }
      }
    })
  }

  /// Sends what the user typed. A leading `!` runs the rest as a shell command; anything else goes to omp unchanged. While a
  /// turn runs, a message is queued instead: it steers the turn, or with `follow_up` waits for it to end.
  pub fn send(&mut self, live: LiveSessionId, text: &str, follow_up: bool, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    if let Some(command) = text.strip_prefix('!') {
      let command = command.to_owned();
      self.bash_running.insert(live);
      cx.notify();
      return cx.spawn(async move |this, cx| {
        let result = LiveApiClient::bash(&client, BashRequest { live, command })
          .await
          .map_err(Error::from);
        // The command's output is a message in the transcript; read it back.
        let reloaded = match &result {
          Ok(_) => Some(load(&client, live).await),
          Err(_) => None,
        };
        if this
          .update(cx, |model, cx| {
            model.bash_running.remove(&live);
            match (result, reloaded) {
              (Err(error), _) => model.fail(&error, cx),
              (Ok(_), Some(Ok((_, page)))) => model.conversation.load_page(live, page),
              (Ok(_), Some(Err(error))) => model.fail(&error, cx),
              (Ok(_), None) => {}
            }
            cx.notify();
          })
          .is_err()
        {
          tracing::debug!("model released before the command finished");
        }
      });
    }
    let running = self.conversation.live(live).is_some_and(|view| view.run == Run::Running);
    let request = PromptRequest {
      live,
      message: text.to_owned(),
    };
    cx.spawn(async move |this, cx| {
      let result = match (running, follow_up) {
        (false, _) => LiveApiClient::prompt(&client, request).await,
        (true, false) => LiveApiClient::steer(&client, request).await,
        (true, true) => LiveApiClient::follow_up(&client, request).await,
      };
      if let Err(error) = result.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
  }

  /// Aborts the running turn and sends `text` in its place. Messages queued behind the turn are not touched.
  pub fn interrupt_and_send(&mut self, live: LiveSessionId, text: String, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let request = PromptRequest { live, message: text };
      if let Err(error) = LiveApiClient::abort_and_prompt(&client, request).await.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
  }

  /// Aborts what is running: the `!` command when there is one, else the turn. Queued messages the abort withdraws come back
  /// as [`ModelEvent::ComposerText`].
  pub fn abort(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    let bash = self.bash_running.contains(&live);
    let queued = self
      .conversation
      .live(live)
      .is_some_and(|view| !view.queue.steering.is_empty() || !view.queue.follow_up.is_empty());
    cx.spawn(async move |this, cx| {
      if queued && !bash {
        match QueueApiClient::restore(&client, live).await.map_err(Error::from) {
          Ok(restored) => {
            let text = restored
              .steering
              .iter()
              .chain(&restored.follow_up)
              .map(|message| message.text.as_str())
              .collect::<Vec<_>>()
              .join("\n");
            if this.update(cx, |_, cx| cx.emit(ModelEvent::ComposerText { live, text })).is_err() {
              tracing::debug!("model released before the command finished");
              return;
            };
          }
          Err(error) => fail(&this, cx, &error),
        }
        return;
      }
      let result = if bash {
        LiveApiClient::abort_bash(&client, live).await
      } else {
        LiveApiClient::abort(&client, live).await
      };
      if let Err(error) = result.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
  }

  /// Takes a queued message out of its queue.
  pub fn remove_queued(&mut self, live: LiveSessionId, message: String, queue: QueuedMessageQueue, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let request = RemoveQueuedRequest { live, message, queue };
      if let Err(error) = QueueApiClient::remove(&client, request).await.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
  }

  /// Moves a queued follow-up to the steering queue.
  pub fn promote_queued(&mut self, live: LiveSessionId, message: String, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      if let Err(error) = QueueApiClient::promote(&client, PromoteQueuedRequest { live, message })
        .await
        .map_err(Error::from)
      {
        fail(&this, cx, &error);
      }
    })
  }

  /// Reads the entries omp appended since the tree's newest, or all of them when there is no tree yet or omp does not know
  /// the cursor. One read runs per session; triggers that arrive meanwhile ask for one more when it ends.
  fn refresh_tree(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    if let Some(again) = self.tree_reads.get_mut(&live) {
      *again = true;
      return;
    }
    self.tree_reads.insert(live, false);
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      loop {
        let since = match this.update(cx, |model, _| model.conversation.tree_cursor(live).cloned()) {
          Ok(since) => since,
          Err(_) => return,
        };
        let read = LiveApiClient::entries(
          &client,
          EntriesRequest {
            live,
            since: since.clone(),
          },
        )
        .await
        .map_err(Error::from);
        let read = match read {
          // omp does not know the cursor (the session moved to another file, say): read everything.
          Err(Error::Gateway {
            data: Some(ErrorData::OmpCommandFailed { code: Some(code) }),
            ..
          }) if code == "unknown_since" => LiveApiClient::entries(&client, EntriesRequest { live, since: None })
            .await
            .map_err(Error::from)
            .map(|entries| (entries, true)),
          other => other.map(|entries| (entries, since.is_none())),
        };
        let again = this.update(cx, |model, cx| {
          match read {
            Ok((entries, true)) => model.conversation.set_entries(live, entries),
            Ok((entries, false)) => model.conversation.apply_entries(live, entries),
            Err(error) => {
              model.tree_reads.remove(&live);
              model.fail(&error, cx);
              return false;
            }
          }
          model.load_branch_candidates(live, cx);
          cx.notify();
          let again = model.tree_reads.insert(live, false).unwrap_or(false);
          if !again {
            model.tree_reads.remove(&live);
          }
          again
        });
        match again {
          Ok(true) => {}
          _ => return,
        }
      }
    })
    .detach();
  }

  fn load_branch_candidates(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::branch_messages(&client, live).await.map_err(Error::from) {
        Ok(messages) => {
          if this
            .update(cx, |model, cx| {
              let ids = messages.into_iter().map(|message| EntryId::from(message.entry_id)).collect();
              model.branch_candidates.insert(live, ids);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      },
    )
    .detach();
  }

  /// Reads the session's state and transcript again, and its history: the active branch changed under the views. With
  /// `new_file` the session moved onto another file, so the tree starts over.
  fn reload(&mut self, live: LiveSessionId, new_file: bool, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| match load(&client, live).await {
      Ok((state, page)) => {
        if this
          .update(cx, |model, cx| {
            model.conversation.set_state(live, &state);
            model.conversation.load_page(live, page);
            if new_file {
              model.conversation.forget_tree(live);
            }
            model.refresh_tree(live, cx);
            cx.notify();
          })
          .is_err()
        {
          tracing::debug!("model released before the command finished");
        }
      }
      Err(error) => fail(&this, cx, &error),
    })
    .detach();
  }

  /// Moves the active branch to an entry. A user message rewinds past itself and its text goes to the composer.
  pub fn navigate_tree(
    &mut self,
    live: LiveSessionId,
    entry: EntryId,
    summarize: bool,
    instructions: String,
    cx: &mut Context<Self>,
  ) -> Task<()> {
    let client = self.client.clone();
    if summarize {
      self.summarizing.insert(live);
      cx.notify();
    }
    cx.spawn(async move |this, cx| {
      let request = NavigateRequest {
        live,
        entry,
        summarize: summarize.then_some(true),
        custom_instructions: (!instructions.trim().is_empty()).then_some(instructions),
      };
      let result = LiveApiClient::navigate_tree(&client, request).await.map_err(Error::from);
      if this
        .update(cx, |model, cx| {
          model.summarizing.remove(&live);
          match result {
            Ok(result) if !result.cancelled => {
              if let Some(text) = result.editor_text {
                cx.emit(ModelEvent::ComposerText { live, text });
              }
              model.reload(live, false, cx);
            }
            Ok(_) => {}
            Err(error) => model.fail(&error, cx),
          }
          cx.notify();
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Stops the branch summary a navigation is generating.
  pub fn abort_branch_summary(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      if let Err(error) = LiveApiClient::abort_branch_summary(&client, live).await.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
  }

  /// Branches at a user message; its text goes to the composer.
  pub fn branch(&mut self, live: LiveSessionId, entry: EntryId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::branch(&client, EntryRequest { live, entry })
        .await
        .map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(result) if !result.cancelled => {
            cx.emit(ModelEvent::ComposerText { live, text: result.text });
            model.reload(live, true, cx);
          }
          Ok(_) => {}
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Forks the session into a new file, through an entry.
  pub fn fork(&mut self, live: LiveSessionId, entry: Option<EntryId>, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::fork(&client, ForkRequest { live, entry }).await.map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(result) if !result.cancelled => model.reload(live, true, cx),
          Ok(_) => {}
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Sets an entry's label, or clears it.
  pub fn label(&mut self, live: LiveSessionId, entry: EntryId, label: Option<String>, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::label(&client, LabelRequest { live, entry, label })
        .await
        .map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(()) => model.refresh_tree(live, cx),
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Names the session. The tab and the sidebar follow once omp has the name.
  pub fn rename(&mut self, live: LiveSessionId, name: String, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::rename(&client, RenameRequest { live, name })
        .await
        .map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(()) => {
            model.refresh_state(live, cx);
            model.refresh_projects(cx).detach();
          }
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Compacts the conversation. Progress arrives as events; the transcript is read again once it is done.
  pub fn compact(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::compact(
        &client,
        CompactRequest {
          live,
          custom_instructions: None,
        },
      )
      .await
      .map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(_) => model.reload(live, false, cx),
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Reads the counts of the session.
  pub fn refresh_stats(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::stats(&client, live).await.map_err(Error::from) {
        Ok(stats) => {
          if this
            .update(cx, |model, cx| {
              model.stats.insert(live, stats);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      },
    )
  }

  /// Writes the session as an HTML page, where omp chooses.
  pub fn export_html(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let result = LiveApiClient::export_html(&client, ExportRequest { live, output_path: None })
        .await
        .map_err(Error::from);
      if this
        .update(cx, |model, cx| match result {
          Ok(path) => cx.emit(ModelEvent::Exported { live, path }),
          Err(error) => model.fail(&error, cx),
        })
        .is_err()
      {
        tracing::debug!("model released before the command finished");
      }
    })
  }

  /// Tells the views what arrived for the user since `seen` notices: the new notices, and text an extension put in the
  /// composer.
  fn announce(&mut self, live: LiveSessionId, seen: usize, cx: &mut Context<Self>) {
    let Some(view) = self.conversation.live_mut(live) else { return };
    let notices: Vec<Notice> = view.notices.iter().skip(seen).cloned().collect();
    let text = view.take_editor_text();
    for notice in notices {
      cx.emit(ModelEvent::Notice { live, notice });
    }
    if let Some(text) = text {
      cx.emit(ModelEvent::ComposerText { live, text });
    }
  }

  /// Answers or dismisses a dialog. The dialog closes once omp has the answer.
  pub fn respond(&mut self, live: LiveSessionId, response: ExtensionUiResponse, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    let id = match &response {
      ExtensionUiResponse::ValueUiResponse(response) => response.id.clone(),
      ExtensionUiResponse::ConfirmUiResponse(response) => response.id.clone(),
      ExtensionUiResponse::CancelUiResponse(response) => response.id.clone(),
      ExtensionUiResponse::AnswersUiResponse(response) => response.id.clone(),
    };
    cx.spawn(async move |this, cx| {
      match DialogApiClient::respond(&client, RespondRequest { live, response })
        .await
        .map_err(Error::from)
      {
        Ok(()) => {
          if this
            .update(cx, |model, cx| {
              model.conversation.dialog_answered(live, &id);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
  }

  /// Reads `live/state` again, for what changed in omp without an event that says what.
  fn refresh_state(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::state(&client, live).await.map_err(Error::from) {
        Ok(state) => {
          if this
            .update(cx, |model, cx| {
              model.conversation.set_state(live, &state);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      },
    )
    .detach();
  }

  /// Loads the models and thinking levels a picker offers. The levels depend on the current model.
  fn load_catalog(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let loaded = async {
        let models = ModelApiClient::models(&client, live).await?;
        let levels = ModelApiClient::thinking_levels(&client, live).await?;
        Ok::<_, Error>(Catalog { models, levels })
      }
      .await;
      match loaded {
        Ok(catalog) => {
          if this
            .update(cx, |model, cx| {
              model.catalogs.insert(live, catalog);
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
    .detach();
  }

  /// Switches the session to a model, then shows what omp now reports.
  pub fn set_model(&mut self, live: LiveSessionId, provider: String, model_id: String, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    self.change_model(cx, live, async move {
      ModelApiClient::set_model(&client, SetModelRequest { live, provider, model_id })
        .await
        .map(drop)
    })
  }

  /// Switches to the next model in omp's cycle.
  pub fn cycle_model(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    self.change_model(cx, live, async move { ModelApiClient::cycle_model(&client, live).await.map(drop) })
  }

  /// Sets the thinking level, then shows what omp now reports.
  pub fn set_thinking(&mut self, live: LiveSessionId, level: ThinkingLevel, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    self.change_thinking(cx, live, async move {
      ModelApiClient::set_thinking(&client, SetThinkingRequest { live, level }).await
    })
  }

  /// Switches to the next thinking level.
  pub fn cycle_thinking(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    self.change_thinking(
      cx,
      live,
      async move { ModelApiClient::cycle_thinking(&client, live).await.map(drop) },
    )
  }

  /// A model change also changes the thinking levels on offer.
  fn change_model<E: Into<Error> + 'static>(
    &mut self,
    cx: &mut Context<Self>,
    live: LiveSessionId,
    call: impl Future<Output = Result<(), E>> + 'static,
  ) -> Task<()> {
    cx.spawn(async move |this, cx| match call.await.map_err(Into::into) {
      Ok(()) => {
        if this
          .update(cx, |model, cx| {
            model.refresh_state(live, cx);
            model.load_catalog(live, cx);
          })
          .is_err()
        {
          tracing::debug!("model released before the command finished");
        }
      }
      Err(error) => fail(&this, cx, &error),
    })
  }

  fn change_thinking<E: Into<Error> + 'static>(
    &mut self,
    cx: &mut Context<Self>,
    live: LiveSessionId,
    call: impl Future<Output = Result<(), E>> + 'static,
  ) -> Task<()> {
    cx.spawn(async move |this, cx| match call.await.map_err(Into::into) {
      Ok(()) => {
        if this.update(cx, |model, cx| model.refresh_state(live, cx)).is_err() {
          tracing::debug!("model released before the command finished");
        }
      }
      Err(error) => fail(&this, cx, &error),
    })
  }

  /// Closes a live session and its omp process.
  pub fn close(&mut self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::close(&client, live).await.map_err(Error::from) {
        Ok(()) => {
          if this
            .update(cx, |model, cx| {
              model.sessions.remove(&live);
              model.bash_running.remove(&live);
              model.branch_candidates.remove(&live);
              model.summarizing.remove(&live);
              model.stats.remove(&live);
              model.catalogs.remove(&live);
              model.tree_reads.remove(&live);
              model.conversation.close(live);
              cx.emit(ModelEvent::Closed(live));
              cx.notify();
            })
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      },
    )
  }

  fn fail(&mut self, error: &Error, cx: &mut Context<Self>) {
    cx.emit(failure(error));
  }
}

/// Test-only access. It stays inside the module, so the model's public API does not widen for the tests.
#[cfg(test)]
impl Model {
  /// Puts projects and their first session pages in the state, as `projects/list` and `sessions/list` would have.
  pub(crate) fn seed_workspace(
    &mut self,
    projects: Vec<alvin_protocol::ProjectSummary>,
    pages: Vec<(SessionsKey, alvin_protocol::SessionPage)>,
    cx: &mut Context<Self>,
  ) {
    self.conversation.set_projects(projects);
    for (key, page) in pages {
      self.conversation.load_sessions(key, None, page);
    }
    cx.notify();
  }

  /// Tracks a live session and announces it, as `live/open` would have. No omp starts.
  pub(crate) fn seed_open(&mut self, summary: &LiveSessionSummary, cx: &mut Context<Self>) {
    self.conversation.open(summary);
    cx.emit(ModelEvent::Opened(summary.live));
    cx.notify();
  }

  /// Announces that a live session closed, as `live/close` would have.
  pub(crate) fn seed_close(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    cx.emit(ModelEvent::Closed(live));
    cx.notify();
  }

  /// Folds a live-session event into the state, as the event stream would have.
  pub(crate) fn seed_event(&mut self, event: &alvin_protocol::Event, cx: &mut Context<Self>) {
    self.conversation.apply(event);
    cx.notify();
  }

  /// Puts a page of messages in a live session's transcript, as `live/messages` would have.
  pub(crate) fn seed_messages(&mut self, live: LiveSessionId, page: alvin_protocol::MessagesPage, cx: &mut Context<Self>) {
    self.conversation.load_page(live, page);
    cx.notify();
  }

  /// Records the connection state the client would have reported.
  pub(crate) fn seed_connection(&mut self, connection: Connection, cx: &mut Context<Self>) {
    self.conversation.set_connection(connection);
    cx.notify();
  }
}

/// The state and every message of a live session. omp-rpc drains `get_messages_page` the same way: pages in order until
/// there is no cursor.
async fn load(client: &Client, live: LiveSessionId) -> Result<(alvin_protocol::SessionState, MessagesPage), Error> {
  let state = LiveApiClient::state(client, live).await?;
  let mut messages = Vec::new();
  let mut cursor = None;
  let total = loop {
    let page = LiveApiClient::messages(client, MessagesRequest { live, cursor, limit: None }).await?;
    messages.extend(page.messages);
    match page.next_cursor {
      Some(next) => cursor = Some(next),
      None => break page.total_messages,
    }
  };
  Ok((
    state,
    MessagesPage {
      messages,
      total_messages: total,
      next_cursor: None,
    },
  ))
}

fn failure(error: &Error) -> ModelEvent {
  match error {
    Error::Gateway { code, message, .. } => ModelEvent::Failed {
      code: Some(*code),
      message: message.clone().into(),
    },
    other => ModelEvent::Failed {
      code: None,
      message: other.to_string().into(),
    },
  }
}

fn fail(this: &WeakEntity<Model>, cx: &mut AsyncApp, error: &Error) {
  let event = failure(error);
  if this.update(cx, |_, cx| cx.emit(event)).is_err() {
    tracing::debug!("model released before the command finished");
  }
}
