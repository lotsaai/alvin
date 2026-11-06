//! The one owner of domain state: wraps `alvin_conversation` state, issues commands through the client, and runs one
//! event task per live session. Views read it and call its methods; effects arrive back as events.

use std::collections::{BTreeMap, BTreeSet};

use alvin_client::{Client, ConnectionState, Error, ResyncReason};
use alvin_conversation::{Connection, Conversation, Notice, Run, SessionsKey};
use alvin_protocol::{
  BashRequest, CheckoutId, CompactRequest, DialogApiClient, EntriesRequest, EntryId, EntryRequest, ErrorCode, ErrorData, EventId,
  ExportRequest, ExtensionUiResponse, ForkRequest, LabelRequest, ListSessions, LiveApiClient, LiveSessionId, LiveSessionSummary,
  MessagesPage, MessagesRequest, ModelApiClient, ModelInfo, NavigateRequest, OpenRequest, OpenTarget, ProjectId, ProjectsApiClient,
  PromoteQueuedRequest, PromptRequest, QueueApiClient, QueuedMessageQueue, RemoveQueuedRequest, RenameRequest, RespondRequest,
  SessionCursor, SessionId, SessionPage, SessionStats, SessionsApiClient, SetModelRequest, SetThinkingRequest, SubscribeRequest,
  ThinkingLevel,
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
  /// A live session was opened and is tracked, with the navigation intent that requested it.
  Opened {
    /// The tracked session.
    live: LiveSessionId,
    /// The navigation intent that requested the open.
    intent: EventId,
  },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelChange {
  Workspace,
  Connection,
  Session(LiveSessionId),
  SessionMetadata(LiveSessionId),
}

/// What a live session can be switched to: omp's models, and the thinking levels the current model accepts.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Catalog {
  /// The models omp can use.
  pub(crate) models: Vec<ModelInfo>,
  /// The thinking levels the current model accepts.
  pub(crate) levels: Vec<ThinkingLevel>,
}

#[derive(Default)]
struct PageLoads {
  generations: BTreeMap<SessionsKey, EventId>,
}

struct PageLoad {
  key: SessionsKey,
  after: Option<SessionCursor>,
  expected: Option<Option<SessionCursor>>,
  generation: EventId,
}

impl PageLoads {
  fn start(&mut self, conversation: &Conversation, key: SessionsKey, after: Option<SessionCursor>) -> PageLoad {
    let generation = if after.is_none() {
      let generation = EventId::new();
      self.generations.insert(key.clone(), generation);
      generation
    } else {
      *self.generations.entry(key.clone()).or_default()
    };
    let expected = conversation.workspace().sessions(&key).map(|list| list.next.clone());
    PageLoad {
      key,
      after,
      expected,
      generation,
    }
  }

  fn apply(&self, conversation: &mut Conversation, load: PageLoad, page: SessionPage) -> bool {
    let current = conversation.workspace().sessions(&load.key).map(|list| list.next.clone());
    if self.generations.get(&load.key) != Some(&load.generation)
      || current != load.expected
      || load
        .after
        .as_ref()
        .is_some_and(|after| current.as_ref().and_then(Option::as_ref) != Some(after))
    {
      return false;
    }
    conversation.load_sessions(load.key, load.after.as_ref(), page);
    true
  }
}

#[derive(Default)]
struct Navigation {
  intent: EventId,
}

impl Navigation {
  fn begin(&mut self) -> EventId {
    self.intent = EventId::new();
    self.intent
  }

  fn is_current(&self, intent: EventId) -> bool {
    self.intent == intent
  }
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
  session_ids: BTreeMap<LiveSessionId, SessionId>,
  new_sessions: BTreeSet<LiveSessionId>,
  session_checkouts: BTreeMap<LiveSessionId, (ProjectId, CheckoutId)>,
  page_loads: PageLoads,
  navigation: Navigation,
  client: Client,
  /// Live sessions with a `!` command running, so Escape aborts that and not the turn.
  bash_running: BTreeSet<LiveSessionId>,
  /// One task per live session: loads it, then folds its events. Dropping a task stops it.
  sessions: BTreeMap<LiveSessionId, Task<()>>,
  _connection: Task<()>,
}

impl EventEmitter<ModelEvent> for Model {}
impl EventEmitter<ModelChange> for Model {}

impl Model {
  fn session_event_changed(event: &alvin_protocol::Event, cx: &mut Context<Self>) {
    cx.emit(ModelChange::Session(event.live));
    if session_event_changes_metadata(event) {
      cx.emit(ModelChange::SessionMetadata(event.live));
    }
  }

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
            cx.emit(ModelChange::Connection);
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
      session_ids: BTreeMap::new(),
      new_sessions: BTreeSet::new(),
      session_checkouts: BTreeMap::new(),
      page_loads: PageLoads::default(),
      navigation: Navigation::default(),
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

  /// Loads projects and refreshes the first page of already loaded checkouts.
  pub fn refresh_projects(&mut self, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let projects = ProjectsApiClient::list(&client).await.map_err(Error::from);
      let keys = match projects {
        Ok(projects) => {
          match this.update(cx, |model, cx| {
            let mut keys = Vec::new();
            for project in &projects {
              for checkout in &project.checkouts {
                let key = SessionsKey {
                  project: project.id.clone(),
                  checkout: Some(checkout.id.clone()),
                };
                if model.conversation.workspace().sessions(&key).is_some() {
                  keys.push(key);
                }
              }
            }
            model.conversation.set_projects(projects);
            cx.emit(ModelChange::Workspace);
            cx.notify();
            keys
          }) {
            Ok(keys) => keys,
            Err(_) => return,
          }
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
    let load = self.page_loads.start(&self.conversation, key, after);
    cx.spawn(async move |this, cx| {
      let request = ListSessions {
        project: load.key.project.clone(),
        checkout: load.key.checkout.clone(),
        after: load.after.clone(),
        limit: SESSIONS_PAGE,
      };
      match SessionsApiClient::list(&client, request).await.map_err(Error::from) {
        Ok(page) => {
          if this
            .update(cx, |model, cx| {
              if model.page_loads.apply(&mut model.conversation, load, page) {
                cx.emit(ModelChange::Workspace);
                cx.notify();
              }
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
  /// Loads every remaining session page for the currently known checkouts.
  pub(crate) fn load_all_sessions(&mut self, cx: &mut Context<Self>) -> Task<bool> {
    let client = self.client.clone();
    let keys: Vec<_> = self
      .conversation
      .workspace()
      .projects()
      .iter()
      .flat_map(|project| {
        project.checkouts.iter().map(|checkout| SessionsKey {
          project: project.id.clone(),
          checkout: Some(checkout.id.clone()),
        })
      })
      .collect();
    cx.spawn(async move |this, cx| {
      for key in keys {
        loop {
          let load = match this.update(cx, |model, _| {
            let after = match model.conversation.workspace().sessions(&key) {
              Some(list) => list.next.clone().map(Some),
              None => Some(None),
            }?;
            Some(model.page_loads.start(&model.conversation, key.clone(), after))
          }) {
            Ok(Some(after)) => after,
            Ok(None) => break,
            Err(_) => return false,
          };
          let request = ListSessions {
            project: load.key.project.clone(),
            checkout: load.key.checkout.clone(),
            after: load.after.clone(),
            limit: SESSIONS_PAGE,
          };
          let page = match SessionsApiClient::list(&client, request).await.map_err(Error::from) {
            Ok(page) => page,
            Err(error) => {
              fail(&this, cx, &error);
              return false;
            }
          };
          if this
            .update(cx, |model, cx| {
              if model.page_loads.apply(&mut model.conversation, load, page) {
                cx.emit(ModelChange::Workspace);
                cx.notify();
              }
            })
            .is_err()
          {
            return false;
          }
        }
      }
      true
    })
  }

  /// Opens a listed session in a live session, or the live session it is already in.
  pub fn open_session(&mut self, session: SessionId, project: ProjectId, checkout: CheckoutId, cx: &mut Context<Self>) -> Task<()> {
    self.open(OpenTarget::Session { session }, (project, checkout), cx)
  }

  /// Opens a new session in a checkout of a project.
  pub fn new_session(&mut self, project: ProjectId, checkout: CheckoutId, cx: &mut Context<Self>) -> Task<()> {
    self.open(
      OpenTarget::New {
        project: project.clone(),
        checkout: checkout.clone(),
      },
      (project, checkout),
      cx,
    )
  }

  fn open(&mut self, target: OpenTarget, checkout: (ProjectId, CheckoutId), cx: &mut Context<Self>) -> Task<()> {
    let session = match &target {
      OpenTarget::Session { session } => Some(session.clone()),
      OpenTarget::New { .. } => None,
    };
    let intent = self.navigation.begin();
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let opened = LiveApiClient::open(&client, OpenRequest { target, model: None })
        .await
        .map_err(Error::from);
      match opened {
        Ok(summary) => {
          if this
            .update(cx, |model, cx| model.opened(summary, session, checkout, intent, cx))
            .is_err()
          {
            tracing::debug!("model released before the command finished");
          }
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
  }

  fn opened(
    &mut self,
    summary: LiveSessionSummary,
    session: Option<SessionId>,
    checkout: (ProjectId, CheckoutId),
    intent: EventId,
    cx: &mut Context<Self>,
  ) {
    let live = summary.live;
    self.session_checkouts.insert(live, checkout);
    if session.is_none() {
      self.new_sessions.insert(live);
    }
    if let Some(session) = session {
      self.session_ids.insert(live, session);
    }
    self.conversation.open(&summary);
    if !self.sessions.contains_key(&live) {
      let task = self.attach(live, cx);
      self.sessions.insert(live, task);
    }
    cx.emit(ModelEvent::Opened { live, intent });
    cx.emit(ModelChange::SessionMetadata(live));
    cx.notify();
  }

  pub(crate) fn navigation_is_current(&self, intent: EventId) -> bool {
    self.navigation.is_current(intent)
  }

  pub(crate) fn session_id(&self, live: LiveSessionId) -> Option<&SessionId> {
    self.session_ids.get(&live)
  }

  pub(crate) fn is_new_session(&self, live: LiveSessionId) -> bool {
    self.new_sessions.contains(&live)
  }

  pub(crate) fn session_checkout(&self, live: LiveSessionId) -> Option<&(ProjectId, CheckoutId)> {
    self.session_checkouts.get(&live)
  }

  fn set_state(&mut self, live: LiveSessionId, state: &alvin_protocol::SessionState, request: EventId) {
    if self.conversation.live(live).is_some() {
      self.session_ids.insert(live, SessionId::from(state.session_id.clone()));
      self.conversation.set_state(live, state, request);
    }
  }

  pub(crate) fn select_tab(&mut self) {
    self.navigation.begin();
  }

  /// Loads a live session's state and transcript, then folds its events until it is closed. A stream that cannot
  /// continue without a gap (`Resync`) drops the transcript and starts over.
  fn attach(&self, live: LiveSessionId, cx: &mut Context<Self>) -> Task<()> {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      loop {
        let request = match this.update(cx, |model, _| {
          model.conversation.live_mut(live).map(|view| view.begin_state_request())
        }) {
          Ok(Some(request)) => request,
          _ => return,
        };
        match load(&client, live).await {
          Ok((state, page)) => {
            let loaded = this.update(cx, |model, cx| {
              model.set_state(live, &state, request);
              model.conversation.load_page(live, page);
              model.load_catalog(live, cx);
              model.refresh_tree(live, cx);
              model.refresh_stats(live, cx).detach();
              cx.emit(ModelChange::Session(live));
              cx.emit(ModelChange::SessionMetadata(live));
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
                  let model_changed = matches!(&event.body, alvin_protocol::EventBody::ConfigUpdate(update)
                  if update.model.as_ref().is_some_and(|next| {
                    model.conversation.live(live).and_then(|view| view.model.as_ref()) != Some(next)
                  }));
                  let seen = model.conversation.live(live).map_or(0, |view| view.notices.len());
                  model.conversation.apply(&event);
                  if model_changed {
                    model.refresh_thinking_levels(live, cx);
                  }
                  model.announce(live, seen, cx);
                  if model.conversation.live(live).is_some_and(|view| view.tree_stale) {
                    model.refresh_tree(live, cx);
                  }
                  // omp said the model changed without saying to what.
                  if model.conversation.live(live).is_some_and(|view| view.state_stale) {
                    model.refresh_state(live, cx);
                  }
                  if matches!(&event.body, alvin_protocol::EventBody::SessionSettled(_)) {
                    cx.notify();
                    model.refresh_stats(live, cx).detach();
                  }
                  Self::session_event_changed(&event, cx);
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
                cx.emit(ModelChange::Session(live));
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
      cx.emit(ModelChange::Session(live));
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
            if result.is_ok() {
              model.new_sessions.remove(&live);
            }
            model.bash_running.remove(&live);
            match (result, reloaded) {
              (Err(error), _) => model.fail(&error, cx),
              (Ok(_), Some(Ok((_, page)))) => model.conversation.load_page(live, page),
              (Ok(_), Some(Err(error))) => model.fail(&error, cx),
              (Ok(_), None) => {}
            }
            cx.emit(ModelChange::Session(live));
            cx.emit(ModelChange::SessionMetadata(live));
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
      match result.map_err(Error::from) {
        Ok(_) => {
          if this
            .update(cx, |model, cx| {
              if model.new_sessions.remove(&live) {
                cx.emit(ModelChange::Session(live));
                cx.emit(ModelChange::SessionMetadata(live));
              }
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

  /// Switches only this session's model, leaving omp's configured roles unchanged.
  pub(crate) fn switch_model(&mut self, live: LiveSessionId, model: &ModelInfo, cx: &mut Context<Self>) -> Task<()> {
    let message = format!("/switch {}/{}", model.provider, model.id);
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      if let Err(error) = LiveApiClient::prompt(&client, PromptRequest { live, message })
        .await
        .map_err(Error::from)
      {
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
          cx.emit(ModelChange::Session(live));
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
              cx.emit(ModelChange::Session(live));
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
    let Some(request) = self.conversation.live_mut(live).map(|view| view.begin_state_request()) else {
      return;
    };
    let client = self.client.clone();
    cx.spawn(async move |this, cx| match load(&client, live).await {
      Ok((state, page)) => {
        if this
          .update(cx, |model, cx| {
            model.set_state(live, &state, request);
            model.conversation.load_page(live, page);
            if new_file {
              model.conversation.forget_tree(live);
            }
            model.refresh_tree(live, cx);
            model.refresh_stats(live, cx).detach();
            cx.emit(ModelChange::Session(live));
            cx.emit(ModelChange::SessionMetadata(live));
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
      cx.emit(ModelChange::Session(live));
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
          cx.emit(ModelChange::Session(live));
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
              cx.emit(ModelChange::Session(live));
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
              cx.emit(ModelChange::Session(live));
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
    let Some(request) = self.conversation.live_mut(live).map(|view| view.begin_state_request()) else {
      return;
    };
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::state(&client, live).await.map_err(Error::from) {
        Ok(state) => {
          if this
            .update(cx, |model, cx| {
              let model_changed = model.conversation.live(live).and_then(|view| view.model.as_ref()) != state.model.as_ref();
              model.set_state(live, &state, request);
              if model_changed {
                model.refresh_thinking_levels(live, cx);
              }
              cx.emit(ModelChange::Session(live));
              cx.emit(ModelChange::SessionMetadata(live));
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
              cx.emit(ModelChange::Session(live));
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

  fn refresh_thinking_levels(&self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match ModelApiClient::thinking_levels(&client, live).await.map_err(Error::from) {
        Ok(levels) => {
          if this
            .update(cx, |model, cx| {
              if let Some(catalog) = model.catalogs.get_mut(&live) {
                catalog.levels = levels;
                cx.emit(ModelChange::Session(live));
                cx.notify();
              }
            })
            .is_err()
          {
            tracing::debug!("model released before thinking levels loaded");
          }
        }
        Err(error) => fail(&this, cx, &error),
      },
    )
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
              model.session_ids.remove(&live);
              model.new_sessions.remove(&live);
              model.session_checkouts.remove(&live);
              model.conversation.close(live);
              cx.emit(ModelEvent::Closed(live));
              cx.emit(ModelChange::SessionMetadata(live));
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
    cx.emit(ModelChange::Workspace);
    cx.notify();
  }

  /// Tracks a live session and announces it, as `live/open` would have. No omp starts.
  pub(crate) fn seed_open(&mut self, summary: &LiveSessionSummary, cx: &mut Context<Self>) {
    self.conversation.open(summary);
    let intent = self.navigation.begin();
    cx.emit(ModelEvent::Opened {
      live: summary.live,
      intent,
    });
    cx.emit(ModelChange::SessionMetadata(summary.live));
    cx.notify();
  }

  /// Announces that a live session closed, as `live/close` would have.
  pub(crate) fn seed_close(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    cx.emit(ModelEvent::Closed(live));
    cx.emit(ModelChange::SessionMetadata(live));
    cx.notify();
  }

  /// Folds a live-session event into the state, as the event stream would have.
  pub(crate) fn seed_event(&mut self, event: &alvin_protocol::Event, cx: &mut Context<Self>) {
    self.conversation.apply(event);
    Self::session_event_changed(event, cx);
  }

  /// Puts a page of messages in a live session's transcript, as `live/messages` would have.
  pub(crate) fn seed_messages(&mut self, live: LiveSessionId, page: alvin_protocol::MessagesPage, cx: &mut Context<Self>) {
    self.conversation.load_page(live, page);
    cx.emit(ModelChange::Session(live));
  }

  /// Records the connection state the client would have reported.
  pub(crate) fn seed_connection(&mut self, connection: Connection, cx: &mut Context<Self>) {
    self.conversation.set_connection(connection);
    cx.emit(ModelChange::Connection);
    cx.notify();
  }
}

fn session_event_changes_metadata(event: &alvin_protocol::Event) -> bool {
  matches!(
    &event.body,
    alvin_protocol::EventBody::SessionInfoUpdate(_)
      | alvin_protocol::EventBody::PromptResult(_)
      | alvin_protocol::EventBody::SessionSettled(_)
      | alvin_protocol::EventBody::ProcessExited
      | alvin_protocol::EventBody::Agent(alvin_protocol::RpcAgentEvent::AgentStart(_) | alvin_protocol::RpcAgentEvent::AgentEnd(_))
  )
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

#[cfg(test)]
mod tests {
  use alvin_conversation::{Conversation, SessionsKey};
  use alvin_protocol::{ProjectId, SessionCursor, SessionPage};

  use super::{Navigation, PageLoads};
  use crate::test_support::session_summary;

  fn page(id: &str, next: Option<SessionCursor>) -> SessionPage {
    SessionPage {
      sessions: vec![session_summary(id, id)],
      next,
    }
  }

  #[test]
  fn refresh_rejects_older_refresh_and_pages_even_when_cursor_is_unchanged() {
    let key = SessionsKey {
      project: ProjectId::from("project".to_owned()),
      checkout: None,
    };
    let other = SessionsKey {
      project: ProjectId::from("other".to_owned()),
      checkout: None,
    };
    let mut conversation = Conversation::default();
    let mut loads = PageLoads::default();
    let cursor = SessionCursor {
      modified: session_summary("cursor", "cursor").modified,
      id: alvin_protocol::SessionId::from("cursor".to_owned()),
    };
    let first = loads.start(&conversation, key.clone(), None);
    assert!(loads.apply(&mut conversation, first, page("initial", Some(cursor.clone()))));
    let old_page = loads.start(&conversation, key.clone(), Some(cursor.clone()));
    let old_refresh = loads.start(&conversation, key.clone(), None);
    let unrelated = loads.start(&conversation, other.clone(), None);
    let latest = loads.start(&conversation, key.clone(), None);
    assert!(loads.apply(&mut conversation, latest, page("latest", Some(cursor.clone()))));
    assert!(!loads.apply(&mut conversation, old_refresh, page("stale", Some(cursor.clone()))));
    assert!(!loads.apply(&mut conversation, old_page, page("stale-page", None)));
    assert!(loads.apply(&mut conversation, unrelated, page("other", None)));
    assert_eq!(conversation.workspace().sessions(&key).unwrap().sessions[0].id.as_str(), "latest");
    assert_eq!(conversation.workspace().sessions(&other).unwrap().sessions[0].id.as_str(), "other");

    let wrong_cursor = SessionCursor {
      id: alvin_protocol::SessionId::from("wrong".to_owned()),
      ..cursor.clone()
    };
    let wrong = loads.start(&conversation, key.clone(), Some(wrong_cursor));
    assert!(!loads.apply(&mut conversation, wrong, page("wrong", None)));
    let next = loads.start(&conversation, key.clone(), Some(cursor.clone()));
    let duplicate = loads.start(&conversation, key.clone(), Some(cursor));
    assert!(loads.apply(&mut conversation, next, page("next", None)));
    assert!(!loads.apply(&mut conversation, duplicate, page("duplicate", None)));
    assert_eq!(conversation.workspace().sessions(&key).unwrap().sessions.len(), 2);
  }

  #[test]
  fn latest_navigation_intent_wins_even_if_older_open_completes_last() {
    let mut navigation = Navigation::default();
    let first_open = navigation.begin();
    let second_open = navigation.begin();
    assert!(navigation.is_current(second_open));
    assert!(!navigation.is_current(first_open));
    let pending_open = navigation.begin();
    let tab_switch = navigation.begin();
    assert!(navigation.is_current(tab_switch));
    assert!(!navigation.is_current(pending_open));
  }
}
