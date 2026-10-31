//! The one owner of domain state: wraps `alvin_conversation` state, issues commands through the client, and runs one
//! event task per live session. Views read it and call its methods; effects arrive back as events.

use std::collections::{BTreeMap, BTreeSet};

use alvin_client::{Client, ConnectionState, Error, ResyncReason};
use alvin_conversation::{Connection, Conversation, SessionsKey};
use alvin_protocol::{
  BashRequest, CheckoutId, ErrorCode, ListSessions, LiveApiClient, LiveSessionId, LiveSessionSummary, MessagesPage, MessagesRequest,
  OpenRequest, OpenTarget, ProjectId, ProjectsApiClient, PromptRequest, SessionCursor, SessionId, SessionsApiClient, SubscribeRequest,
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
}

/// Projects, sessions and live sessions, and the commands that change them.
pub struct Model {
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

  /// Whether a `!` command is running in the session.
  pub fn bash_running(&self, live: LiveSessionId) -> bool {
    self.bash_running.contains(&live)
  }

  /// Loads the projects and the first page of every checkout's sessions.
  pub fn refresh_projects(&mut self, cx: &mut Context<Self>) {
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
      for key in keys {
        if this.update(cx, |model, cx| model.load_sessions(key, None, cx)).is_err() {
          return;
        }
      }
    })
    .detach();
  }

  /// Loads a page of a checkout's sessions, newest first. Without a cursor it is the first page and replaces the list.
  pub fn load_sessions(&mut self, key: SessionsKey, after: Option<SessionCursor>, cx: &mut Context<Self>) {
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
          drop(this.update(cx, |model, cx| {
            model.conversation.load_sessions(key, after.as_ref(), page);
            cx.notify();
          }));
        }
        Err(error) => fail(&this, cx, &error),
      }
    })
    .detach();
  }

  /// Opens a listed session in a live session, or the live session it is already in.
  pub fn open_session(&mut self, session: SessionId, cx: &mut Context<Self>) {
    self.open(OpenTarget::Session { session }, cx);
  }

  /// Opens a new session in a checkout of a project.
  pub fn new_session(&mut self, project: ProjectId, checkout: CheckoutId, cx: &mut Context<Self>) {
    self.open(OpenTarget::New { project, checkout }, cx);
  }

  fn open(&mut self, target: OpenTarget, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(async move |this, cx| {
      let opened = LiveApiClient::open(&client, OpenRequest { target, model: None })
        .await
        .map_err(Error::from);
      match opened {
        Ok(summary) => drop(this.update(cx, |model, cx| model.opened(summary, cx))),
        Err(error) => fail(&this, cx, &error),
      }
    })
    .detach();
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
                  model.conversation.apply(&event);
                  cx.notify();
                })
                .is_err()
              {
                return;
              }
            }
            Some(Err(Error::Resync(reason))) => break Some(reason),
            Some(Err(error)) => {
              fail(&this, cx, &error);
              return;
            }
            None => break None,
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

  /// Sends what the user typed: text starting with `!` runs as a shell command, anything else is a prompt, unchanged.
  pub fn send(&mut self, live: LiveSessionId, text: &str, cx: &mut Context<Self>) {
    let client = self.client.clone();
    if let Some(command) = text.strip_prefix('!') {
      let command = command.to_owned();
      self.bash_running.insert(live);
      cx.notify();
      cx.spawn(async move |this, cx| {
        let result = LiveApiClient::bash(&client, BashRequest { live, command })
          .await
          .map_err(Error::from);
        // The command's output is a message in the transcript; read it back.
        let reloaded = match &result {
          Ok(_) => Some(load(&client, live).await),
          Err(_) => None,
        };
        drop(this.update(cx, |model, cx| {
          model.bash_running.remove(&live);
          match (result, reloaded) {
            (Err(error), _) => model.fail(&error, cx),
            (Ok(_), Some(Ok((_, page)))) => model.conversation.load_page(live, page),
            (Ok(_), Some(Err(error))) => model.fail(&error, cx),
            (Ok(_), None) => {}
          }
          cx.notify();
        }));
      })
      .detach();
    } else {
      let message = text.to_owned();
      cx.spawn(async move |this, cx| {
        if let Err(error) = LiveApiClient::prompt(&client, PromptRequest { live, message })
          .await
          .map_err(Error::from)
        {
          fail(&this, cx, &error);
        }
      })
      .detach();
    }
  }

  /// Aborts what is running: the `!` command when there is one, else the turn.
  pub fn abort(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    let bash = self.bash_running.contains(&live);
    cx.spawn(async move |this, cx| {
      let result = if bash {
        LiveApiClient::abort_bash(&client, live).await
      } else {
        LiveApiClient::abort(&client, live).await
      };
      if let Err(error) = result.map_err(Error::from) {
        fail(&this, cx, &error);
      }
    })
    .detach();
  }

  /// Closes a live session and its omp process.
  pub fn close(&mut self, live: LiveSessionId, cx: &mut Context<Self>) {
    let client = self.client.clone();
    cx.spawn(
      async move |this, cx| match LiveApiClient::close(&client, live).await.map_err(Error::from) {
        Ok(()) => drop(this.update(cx, |model, cx| {
          model.sessions.remove(&live);
          model.bash_running.remove(&live);
          model.conversation.close(live);
          cx.emit(ModelEvent::Closed(live));
          cx.notify();
        })),
        Err(error) => fail(&this, cx, &error),
      },
    )
    .detach();
  }

  fn fail(&mut self, error: &Error, cx: &mut Context<Self>) {
    cx.emit(failure(error));
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
  drop(this.update(cx, |_, cx| cx.emit(event)));
}
