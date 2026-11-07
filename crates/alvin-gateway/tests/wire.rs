//! Black-box tests of the gateway's wire contract, exercised through `alvin-client` over both transports.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use test_context::{AsyncTestContext, test_context};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::sync::watch;

use alvin_client::{ChannelPair, Client, ConnectionState, Endpoint, Error, EventStream, ResyncReason};
use alvin_gateway::{Gateway, GatewayConfig, WsServer};
use alvin_omp_host::HostConfig;
use alvin_protocol::{
  AnswersUiResponse, AskAnswer, BashRequest, CheckoutId, DialogApiClient, EntriesRequest, EntryId, EntryRequest, ErrorCode, ErrorData,
  Event, EventBody, EventId, ExportRequest, ExtensionUiRequest, ExtensionUiResponse, ForkRequest, LabelRequest, ListSessions,
  LiveApiClient, LiveSessionId, LiveSessionSummary, MessagesRequest, ModelApiClient, NavigateRequest, OpenRequest, OpenTarget, ProjectId,
  ProjectSummary, ProjectsApiClient, PromoteQueuedRequest, PromptRequest, QueueApiClient, QueuedMessageQueue, RemoveQueuedRequest,
  RenameRequest, RespondRequest, RpcAgentEvent, SessionCursor, SessionId, SessionPage, SessionStatus, SessionsApiClient, SetModelRequest,
  SetThinkingRequest, SubscribeRequest, ThinkingLevel,
};
use alvin_protocol::{
  TerminalAccepted, TerminalApiClient, TerminalEventBody, TerminalInput, TerminalResize, TerminalSize, TerminalSubscription,
};

/// Session files written by real omp (the first two records of one run).
fn fixture_sessions() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sessions")
}

/// The records of one real omp run: title slot, header, user message, assistant message.
const SESSION_RECORDS: &str = include_str!("../fixtures/session-records.jsonl");
/// The `cwd` those records were written with.
const RECORDED_CWD: &str = "/tmp/alvin-probe.hnBA/proj";

/// A real git repository and the session files omp wrote under it, in a directory of the test's own. The directory is
/// removed on drop; teardown closes every gateway the test started, so no omp process outlives a failed test.
struct Project {
  root: PathBuf,
  sessions: PathBuf,
  /// An isolated agent directory for the auto-resume configuration test.
  home: PathBuf,
  gateways: Vec<Arc<Gateway>>,
}

impl AsyncTestContext for Project {
  async fn setup() -> Self {
    let base = std::env::temp_dir().join(format!("alvin-gateway-{}", EventId::new().as_uuid()));
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    let git = Command::new("git").args(["init", "--quiet"]).current_dir(&root).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));

    let sessions = base.join("sessions");
    std::fs::create_dir_all(sessions.join("-repo")).unwrap();
    let home = base.join("home");
    std::fs::create_dir(&home).unwrap();
    Self {
      root,
      sessions,
      home,
      gateways: Vec::new(),
    }
  }

  async fn teardown(self) {
    let mut errors = Vec::new();
    for gateway in &self.gateways {
      if let Err(error) = gateway.shutdown().await {
        errors.push(error);
      }
    }
    assert!(errors.is_empty(), "closing test gateways: {errors:?}");
  }
}

impl Project {
  /// Writes `count` sessions, session `n` modified `n` seconds after the first.
  fn sessions(&self, count: usize) {
    let dir = self.sessions.join("-repo");
    for n in 0..count {
      let records = SESSION_RECORDS
        .replace(RECORDED_CWD, self.root.to_str().unwrap())
        .replace("01a18ff1-ab74-73b8-a3a2-79905565c49b", &format!("session-{n}"));
      let file = dir.join(format!("2026-01-01T00-00-0{n}-000Z_session-{n}.jsonl"));
      std::fs::write(&file, records).unwrap();
      let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + n as u64);
      std::fs::File::options().write(true).open(&file).unwrap().set_modified(at).unwrap();
    }
  }

  fn gateway(&mut self) -> Arc<Gateway> {
    let omp = HostConfig::builder()
      .omp(PathBuf::from("omp"))
      .extra_args(vec!["--session-dir".into(), self.sessions.join("-repo").into_os_string()])
      .build();
    let gateway = Arc::new(Gateway::new(
      GatewayConfig::builder().sessions_dir(self.sessions.clone()).omp(omp).build(),
    ));
    self.gateways.push(Arc::clone(&gateway));
    gateway
  }

  /// A gateway using real omp configuration and the fixture's session directory.
  fn live_gateway(&mut self) -> Arc<Gateway> {
    self.gateway_with(&["--no-tools"], true)
  }

  /// A live gateway whose omp has the `ask` tool, which opens a dialog and waits for its answer.
  fn ask_gateway(&mut self) -> Arc<Gateway> {
    self.gateway_with(&["--tools", "ask"], true)
  }

  fn gateway_with(&mut self, tools: &[&str], session_dir: bool) -> Arc<Gateway> {
    let env = if session_dir {
      Vec::new()
    } else {
      vec![
        (OsString::from("HOME"), self.home.clone().into_os_string()),
        (OsString::from("PI_CODING_AGENT_DIR"), self.home.join("agent").into_os_string()),
        (OsString::from("OMP_PROFILE"), OsString::new()),
        (OsString::from("PI_PROFILE"), OsString::new()),
        (OsString::from("XDG_DATA_HOME"), self.home.join("data").into_os_string()),
      ]
    };
    let mut args: Vec<OsString> = tools
      .iter()
      .chain(&["--no-skills", "--no-rules", "--no-extensions", "--no-context-files"])
      .map(OsString::from)
      .collect();
    if session_dir {
      args.push(OsString::from("--session-dir"));
      args.push(self.sessions.join("-repo").into_os_string());
    }
    let omp = HostConfig::builder().omp(PathBuf::from("omp")).extra_args(args).env(env).build();
    let gateway = Arc::new(Gateway::new(
      GatewayConfig::builder()
        .sessions_dir(self.sessions.clone())
        .omp(omp)
        .default_model(Some(MODEL.to_owned()))
        .build(),
    ));
    self.gateways.push(Arc::clone(&gateway));
    gateway
  }
}

impl Drop for Project {
  fn drop(&mut self) {
    if let Some(base) = self.root.parent()
      && let Err(error) = std::fs::remove_dir_all(base)
    {
      if std::thread::panicking() {
        eprintln!("could not remove {}: {error}", base.display());
      } else {
        panic!("could not remove {}: {error}", base.display());
      }
    }
  }
}

/// The model every live test runs against.
const MODEL: &str = "openrouter/inclusionai/ling-3.0-flash";

fn gateway(sessions_dir: impl Into<PathBuf>) -> Gateway {
  let omp = HostConfig::builder().omp(PathBuf::from("omp")).build();
  Gateway::new(GatewayConfig::builder().sessions_dir(sessions_dir.into()).omp(omp).build())
}

async fn channel_client(gateway: &Gateway) -> Client {
  Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(16).unwrap())))
    .await
    .unwrap()
}

/// Binds a listener on a port the OS picks and serves the gateway on it.
async fn serve(gateway: &Gateway) -> WsServer {
  let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
    .await
    .unwrap();
  gateway.serve_ws(listener).await.unwrap()
}

async fn ws_client(gateway: &Gateway) -> (Client, WsServer) {
  let server = serve(gateway).await;
  let endpoint = format!("ws://{}", server.local_addr()).parse::<Endpoint>().unwrap();
  (Client::connect(endpoint).await.unwrap(), server)
}

/// Both transports, as one client each over the same gateway.
async fn both_transports(gateway: &Gateway) -> ([Client; 2], WsServer) {
  let channel = channel_client(gateway).await;
  let (ws, server) = ws_client(gateway).await;
  ([channel, ws], server)
}

fn request(project: ProjectId, after: Option<SessionCursor>, limit: u32) -> ListSessions {
  ListSessions {
    project,
    checkout: None,
    after,
    limit,
  }
}

async fn only_project(client: &Client) -> (ProjectId, CheckoutId) {
  let projects = ProjectsApiClient::list(client).await.unwrap();
  let [ProjectSummary { id, checkouts, .. }] = projects.as_slice() else {
    panic!("expected one project: {projects:?}");
  };
  (id.clone(), checkouts[0].id.clone())
}

#[test_context(Project)]
#[tokio::test]
async fn projects_list_returns_the_same_projects_over_channel_and_websocket(project: &mut Project) {
  project.sessions(2);
  let gateway = project.gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let over_channel = ProjectsApiClient::list(&channel).await.unwrap();
  let over_ws = ProjectsApiClient::list(&ws).await.unwrap();
  drop((channel, ws));
  server.shutdown().await.unwrap();

  assert_eq!(over_channel, over_ws);
  assert!(
    matches!(over_ws.as_slice(), [ProjectSummary { root, checkouts, .. }] if *root == project.root && checkouts.len() == 1),
    "{over_ws:?}"
  );
}

#[tokio::test]
async fn sessions_of_a_deleted_directory_are_not_listed() {
  // The recorded cwd was a temporary directory that no longer exists, so the session belongs to no project.
  let gateway = gateway(fixture_sessions());
  let (client, server) = ws_client(&gateway).await;

  let projects = ProjectsApiClient::list(&client).await.unwrap();
  drop(client);
  server.shutdown().await.unwrap();

  assert_eq!(projects, vec![]);
}

#[tokio::test]
async fn a_gateway_error_reaches_the_client_typed_over_both_transports() {
  let missing = std::env::temp_dir().join(format!("alvin-missing-{}", EventId::new().as_uuid()));
  let gateway = gateway(&missing);
  let (clients, server) = both_transports(&gateway).await;

  let mut errors = Vec::new();
  for client in &clients {
    errors.push(Error::from(ProjectsApiClient::list(client).await.unwrap_err()));
  }
  drop(clients);
  server.shutdown().await.unwrap();

  for error in errors {
    assert!(
      matches!(
        &error,
        Error::Gateway { code: ErrorCode::SessionIndexFailed, data: Some(ErrorData::SessionIndex { path }), .. }
          if *path == missing
      ),
      "{error:?}"
    );
  }
}

#[test_context(Project)]
#[tokio::test]
async fn sessions_list_pages_newest_first_and_matches_over_both_transports(project: &mut Project) {
  project.sessions(3);
  let gateway = project.gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let (id, _checkout) = only_project(&channel).await;

  let mut pages: Vec<[SessionPage; 2]> = Vec::new();
  let mut after = None;
  loop {
    let over_channel = SessionsApiClient::list(&channel, request(id.clone(), after.clone(), 2))
      .await
      .unwrap();
    let over_ws = SessionsApiClient::list(&ws, request(id.clone(), after.clone(), 2)).await.unwrap();
    assert_eq!(over_channel, over_ws);
    after = over_ws.next.clone();
    pages.push([over_channel, over_ws]);
    if after.is_none() {
      break;
    }
  }
  drop((channel, ws));
  server.shutdown().await.unwrap();

  let ids: Vec<Vec<&str>> = pages
    .iter()
    .map(|[page, _]| page.sessions.iter().map(|session| session.id.as_str()).collect())
    .collect();
  assert_eq!(ids, [vec!["session-2", "session-1"], vec!["session-0"]]);
  let first = &pages[0][0].sessions[0];
  assert_eq!(first.status, SessionStatus::Complete);
  assert_eq!(first.title.as_deref(), Some("Reply with the single word ok."));
}

#[test_context(Project)]
#[tokio::test]
async fn an_unknown_project_or_checkout_is_a_typed_error_over_both_transports(project: &mut Project) {
  project.sessions(1);
  let gateway = project.gateway();
  let (clients, server) = both_transports(&gateway).await;
  let (id, _checkout) = only_project(&clients[0]).await;
  let unknown_project = ProjectId::from("nope".to_owned());
  let unknown_checkout = Some(CheckoutId::from("nope".to_owned()));

  let mut errors = Vec::new();
  for client in &clients {
    let wrong_project = SessionsApiClient::list(client, request(unknown_project.clone(), None, 10))
      .await
      .unwrap_err();
    let wrong_checkout = ListSessions {
      checkout: unknown_checkout.clone(),
      ..request(id.clone(), None, 10)
    };
    let wrong_checkout = SessionsApiClient::list(client, wrong_checkout).await.unwrap_err();
    errors.push((Error::from(wrong_project), Error::from(wrong_checkout)));
  }
  drop(clients);
  server.shutdown().await.unwrap();

  for (wrong_project, wrong_checkout) in errors {
    assert!(
      matches!(
        &wrong_project,
        Error::Gateway { code: ErrorCode::UnknownProject, data: Some(ErrorData::UnknownProject { project, checkout: None }), .. }
          if *project == unknown_project
      ),
      "{wrong_project:?}"
    );
    assert!(
      matches!(
        &wrong_checkout,
        Error::Gateway { code: ErrorCode::UnknownProject, data: Some(ErrorData::UnknownProject { checkout, .. }), .. }
          if *checkout == unknown_checkout
      ),
      "{wrong_checkout:?}"
    );
  }
}

#[test_context(Project)]
#[tokio::test]
async fn a_page_of_no_sessions_is_an_invalid_request(project: &mut Project) {
  project.sessions(1);
  let gateway = project.gateway();
  let client = channel_client(&gateway).await;
  let (id, _checkout) = only_project(&client).await;

  let error = Error::from(SessionsApiClient::list(&client, request(id, None, 0)).await.unwrap_err());

  assert!(
    matches!(
      error,
      Error::Gateway {
        code: ErrorCode::InvalidRequest,
        data: None,
        ..
      }
    ),
    "{error:?}"
  );
}

async fn open_new(client: &Client) -> LiveSessionSummary {
  let (project, checkout) = only_project(client).await;
  LiveApiClient::open(
    client,
    OpenRequest {
      target: OpenTarget::New { project, checkout },
      model: None,
    },
  )
  .await
  .unwrap()
}

async fn open_session(client: &Client, session: &str) -> LiveSessionSummary {
  LiveApiClient::open(
    client,
    OpenRequest {
      target: OpenTarget::Session {
        session: SessionId::from(session.to_owned()),
      },
      model: None,
    },
  )
  .await
  .unwrap()
}

async fn subscribe(client: &Client, live: LiveSessionId, after: Option<EventId>) -> Result<EventStream, Error> {
  client.events(SubscribeRequest { live, after }).await
}

async fn next_event(stream: &mut EventStream) -> Event {
  stream.next().await.expect("the event stream ended").unwrap()
}

/// omp's own definition: a session event or a prompt result that says nothing can wake the session again.
fn settled(event: &Event) -> bool {
  match &event.body {
    EventBody::SessionSettled(_) => true,
    EventBody::PromptResult(result) => result.session_settled,
    _ => false,
  }
}

async fn until_settled(stream: &mut EventStream, seen: &mut Vec<Event>) {
  loop {
    let event = next_event(stream).await;
    let done = settled(&event);
    seen.push(event);
    if done {
      return;
    }
  }
}

fn strictly_increasing(events: &[Event]) -> bool {
  events.windows(2).all(|pair| pair[0].id < pair[1].id)
}

/// The omp process id, as omp's own shell reports it.
async fn omp_pid(client: &Client, live: LiveSessionId) -> u32 {
  let result = LiveApiClient::bash(
    client,
    BashRequest {
      live,
      command: "echo pid=$$".to_owned(),
    },
  )
  .await
  .unwrap();
  result.output.trim().strip_prefix("pid=").unwrap().parse().unwrap()
}

fn alive(pid: u32) -> bool {
  Command::new("kill")
    .args(["-0", &pid.to_string()])
    .output()
    .unwrap()
    .status
    .success()
}

async fn prompt(client: &Client, live: LiveSessionId, message: &str) {
  LiveApiClient::prompt(
    client,
    PromptRequest {
      live,
      message: message.to_owned(),
    },
  )
  .await
  .unwrap();
}

async fn full_turn(client: &Client) {
  let opened = open_new(client).await;
  let mut stream = subscribe(client, opened.live, None).await.unwrap();
  prompt(client, opened.live, "Reply with the single word ok.").await;

  let mut seen = Vec::new();
  until_settled(&mut stream, &mut seen).await;

  assert!(strictly_increasing(&seen));
  assert!(seen.iter().all(|event| event.live == opened.live));
  let started = seen.iter().position(|event| matches!(&event.body, EventBody::Agent(_))).unwrap();
  let result = seen
    .iter()
    .position(|event| matches!(&event.body, EventBody::PromptResult(_)))
    .unwrap();
  assert!(started < result);
  let page = LiveApiClient::messages(
    client,
    MessagesRequest {
      live: opened.live,
      cursor: None,
      limit: None,
    },
  )
  .await
  .unwrap();
  assert!(page.total_messages >= 2, "{page:?}");
  assert!(
    format!("{:?}", page.messages).contains("Reply with the single word ok."),
    "{page:?}"
  );
  LiveApiClient::close(client, opened.live).await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_turn_streams_and_settles_over_the_channel(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  full_turn(&channel_client(&gateway).await).await;
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_turn_streams_and_settles_over_websocket(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let (client, server) = ws_client(&gateway).await;
  full_turn(&client).await;
  drop(client);
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn new_sessions_are_fresh_when_omp_auto_resume_is_enabled(project: &mut Project) {
  let agent_dir = project.home.join("agent");
  project.sessions = agent_dir.join("sessions");
  std::fs::create_dir_all(project.sessions.join("-repo")).unwrap();
  std::fs::write(agent_dir.join("config.yml"), "autoResume: true\n").unwrap();
  project.sessions(1);

  let gateway = project.gateway_with(&["--no-tools"], false);
  let ([channel, ws], server) = both_transports(&gateway).await;
  let previous = open_new(&channel).await;
  LiveApiClient::rename(
    &channel,
    RenameRequest {
      live: previous.live,
      name: "Previous session".to_owned(),
    },
  )
  .await
  .unwrap();
  let output = LiveApiClient::bash(
    &channel,
    BashRequest {
      live: previous.live,
      command: "printf previous-session-marker".to_owned(),
    },
  )
  .await
  .unwrap();
  assert_eq!(output.output, "previous-session-marker");
  let old_state = LiveApiClient::state(&channel, previous.live).await.unwrap();
  assert_eq!(old_state.session_name.as_deref(), Some("Previous session"));
  assert!(old_state.message_count > 0);
  LiveApiClient::close(&channel, previous.live).await.unwrap();

  let fresh = open_new(&ws).await;
  let fresh_state = LiveApiClient::state(&ws, fresh.live).await.unwrap();
  assert_ne!(fresh.file, previous.file);
  assert_ne!(fresh_state.session_id, old_state.session_id);
  assert_eq!(fresh_state.session_name, None);
  assert_eq!(fresh_state.message_count, 0);

  let resumed = open_session(&channel, &old_state.session_id).await;
  let resumed_state = LiveApiClient::state(&channel, resumed.live).await.unwrap();
  assert_eq!(resumed.file, previous.file);
  assert_eq!(resumed_state.session_id, old_state.session_id);
  assert_eq!(resumed_state.session_name.as_deref(), Some("Previous session"));
  assert_eq!(resumed_state.message_count, old_state.message_count);

  drop((channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn opening_one_session_file_twice_is_one_live_session(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let client = channel_client(&gateway).await;

  // A session real omp wrote: one turn, then close. Resuming it is what a listed session is.
  let created = open_new(&client).await;
  let mut stream = subscribe(&client, created.live, None).await.unwrap();
  prompt(&client, created.live, "Reply with the single word ok.").await;
  until_settled(&mut stream, &mut Vec::new()).await;
  drop(stream);
  LiveApiClient::close(&client, created.live).await.unwrap();
  let (project_id, _checkout) = only_project(&client).await;
  let page = SessionsApiClient::list(&client, request(project_id, None, 10)).await.unwrap();
  let listed = page
    .sessions
    .iter()
    .find(|session| session.path == created.file)
    .expect("the turn wrote a session file");

  // Concurrent opens of one file must not start two processes.
  let (first, second) = tokio::join!(open_session(&client, listed.id.as_str()), open_session(&client, listed.id.as_str()));
  let third = open_session(&client, listed.id.as_str()).await;

  assert_eq!(first, second);
  assert_eq!(first, third);
  assert_eq!(first.file, created.file);
  assert_eq!(LiveApiClient::list(&client).await.unwrap(), vec![first.clone()]);
  let state = LiveApiClient::state(&client, first.live).await.unwrap();
  assert_eq!(state.session_file.map(PathBuf::from), Some(first.file.clone()));
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_resumed_subscription_replays_later_events_once_and_old_cursors_are_stale(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;

  let mut first = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(
    &channel,
    opened.live,
    "Write the numbers from 1 to 300, one per line, nothing else.",
  )
  .await;
  let mut early = Vec::new();
  for _ in 0..4 {
    early.push(next_event(&mut first).await);
  }
  drop(first);

  let mut second = subscribe(&ws, opened.live, Some(early[1].id)).await.unwrap();
  let mut rest = vec![next_event(&mut second).await, next_event(&mut second).await];
  assert_eq!([rest[0].id, rest[1].id], [early[2].id, early[3].id]);
  until_settled(&mut second, &mut rest).await;
  assert!(strictly_increasing(&rest));

  // The log cleared when the session settled, so a cursor from before that is gone.
  let stale = subscribe(&ws, opened.live, Some(early[0].id)).await.unwrap_err();
  assert!(
    matches!(
      stale,
      Error::Gateway {
        code: ErrorCode::StaleCursor,
        ..
      }
    ),
    "{stale:?}"
  );
  // Without a cursor, a client starts from whatever is retained: nothing, right after a settle.
  drop(subscribe(&ws, opened.live, None).await.unwrap());

  drop((channel, ws, second));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_killed_omp_is_reported_and_its_session_stays_until_closed(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let client = channel_client(&gateway).await;
  let opened = open_new(&client).await;
  let mut stream = subscribe(&client, opened.live, None).await.unwrap();
  let pid = omp_pid(&client, opened.live).await;

  assert!(Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap().success());
  loop {
    if matches!(next_event(&mut stream).await.body, EventBody::ProcessExited) {
      break;
    }
  }

  let error = LiveApiClient::prompt(
    &client,
    PromptRequest {
      live: opened.live,
      message: "hello".to_owned(),
    },
  )
  .await
  .unwrap_err();
  let error = Error::from(error);
  assert!(
    matches!(
      error,
      Error::Gateway {
        code: ErrorCode::OmpProcessExited,
        ..
      }
    ),
    "{error:?}"
  );
  let listed = LiveApiClient::list(&client).await.unwrap();
  assert_eq!(
    listed.iter().map(|live| (live.live, live.exited)).collect::<Vec<_>>(),
    [(opened.live, true)]
  );

  LiveApiClient::close(&client, opened.live).await.unwrap();
  assert_eq!(LiveApiClient::list(&client).await.unwrap(), vec![]);
  let error = Error::from(LiveApiClient::close(&client, opened.live).await.unwrap_err());
  assert!(
    matches!(
      error,
      Error::Gateway {
        code: ErrorCode::UnknownLiveSession,
        ..
      }
    ),
    "{error:?}"
  );
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn shutting_the_gateway_down_leaves_no_omp_process(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let client = channel_client(&gateway).await;
  let opened = open_new(&client).await;
  let pid = omp_pid(&client, opened.live).await;
  assert!(alive(pid));

  gateway.shutdown().await.unwrap();

  assert!(!alive(pid), "omp {pid} survived the gateway");
  assert_eq!(LiveApiClient::list(&client).await.unwrap(), vec![]);
}

#[tokio::test]
async fn live_requests_are_validated_in_the_gateway() {
  let gateway = gateway(fixture_sessions());
  let client = channel_client(&gateway).await;
  let unknown = LiveSessionId::new();

  let no_session = Error::from(
    LiveApiClient::open(
      &client,
      OpenRequest {
        target: OpenTarget::Session {
          session: SessionId::from("no-such-session".to_owned()),
        },
        model: None,
      },
    )
    .await
    .unwrap_err(),
  );
  let no_live = Error::from(LiveApiClient::abort(&client, unknown).await.unwrap_err());
  let empty_page = Error::from(
    LiveApiClient::messages(
      &client,
      MessagesRequest {
        live: unknown,
        cursor: None,
        limit: Some(0),
      },
    )
    .await
    .unwrap_err(),
  );
  let no_subscription = subscribe(&client, unknown, None).await.unwrap_err();

  assert!(
    matches!(
      no_session,
      Error::Gateway {
        code: ErrorCode::UnknownSession,
        ..
      }
    ),
    "{no_session:?}"
  );
  assert!(
    matches!(
      no_live,
      Error::Gateway {
        code: ErrorCode::UnknownLiveSession,
        ..
      }
    ),
    "{no_live:?}"
  );
  // The live session is looked up first, so an unknown one wins over the page size.
  assert!(
    matches!(
      empty_page,
      Error::Gateway {
        code: ErrorCode::UnknownLiveSession,
        ..
      } | Error::Gateway {
        code: ErrorCode::InvalidRequest,
        ..
      }
    ),
    "{empty_page:?}"
  );
  assert!(
    matches!(
      no_subscription,
      Error::Gateway {
        code: ErrorCode::UnknownLiveSession,
        ..
      }
    ),
    "{no_subscription:?}"
  );
}

/// Waits until the client's connection state satisfies `wanted`.
async fn wait_for_state(state: &mut watch::Receiver<ConnectionState>, wanted: impl Fn(ConnectionState) -> bool) {
  state.wait_for(|state| wanted(*state)).await.unwrap();
}

fn reconnecting(state: ConnectionState) -> bool {
  matches!(state, ConnectionState::Reconnecting { .. })
}

fn connected(state: ConnectionState) -> bool {
  state == ConnectionState::Connected
}

async fn ws_client_at(addr: std::net::SocketAddr) -> Client {
  Client::connect(format!("ws://{addr}").parse::<Endpoint>().unwrap()).await.unwrap()
}

/// A TCP forwarder in front of a gateway listener. It binds once and keeps its listener for as long as it lives, so the address
/// clients dial is never released and never bound again: the gateway behind it can stop, and the next one starts on a port of its
/// own. Rebinding a released port would race every other test binding port 0 at the same time.
struct Front {
  addr: std::net::SocketAddr,
  upstream: watch::Sender<Option<std::net::SocketAddr>>,
  epoch: watch::Sender<u64>,
  /// Total bytes carried from clients to the gateway, so a test can wait until a request has left the client.
  sent: watch::Sender<u64>,
  _accept: tokio::task::JoinSet<()>,
}

impl Front {
  /// Binds a port and forwards to `upstream`. Without one, connections are closed as soon as they are accepted.
  async fn start(upstream: Option<std::net::SocketAddr>) -> Self {
    let listener = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
      .await
      .unwrap();
    let addr = listener.local_addr().unwrap();
    let (upstream, upstream_rx) = watch::channel(upstream);
    let (epoch, epoch_rx) = watch::channel(0_u64);
    let (sent, _) = watch::channel(0_u64);
    let counter = sent.clone();
    let mut accept = tokio::task::JoinSet::new();
    accept.spawn(async move {
      let mut connections = tokio::task::JoinSet::new();
      while let Ok((client, _)) = listener.accept().await {
        let Some(target) = *upstream_rx.borrow() else { continue };
        let seen = *epoch_rx.borrow();
        let mut epoch_rx = epoch_rx.clone();
        let sent = counter.clone();
        connections.spawn(async move {
          let Ok(gateway) = tokio::net::TcpStream::connect(target).await else {
            return;
          };
          let (mut from_client, mut to_client) = client.into_split();
          let (mut from_gateway, mut to_gateway) = gateway.into_split();
          let upstream = async {
            let mut buffer = [0_u8; 8192];
            loop {
              match from_client.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                  if to_gateway.write_all(&buffer[..read]).await.is_err() {
                    break;
                  }
                  sent.send_modify(|total| *total += read as u64);
                }
              }
            }
          };
          let downstream = tokio::io::copy(&mut from_gateway, &mut to_client);
          tokio::select! {
            _ = async { tokio::join!(upstream, downstream) } => {}
            _ = epoch_rx.wait_for(|epoch| *epoch != seen) => {}
          }
        });
      }
    });
    Self {
      addr,
      upstream,
      epoch,
      sent,
      _accept: accept,
    }
  }

  /// A receiver of the running total of bytes carried to the gateway.
  fn sent(&self) -> watch::Receiver<u64> {
    self.sent.subscribe()
  }

  /// The address clients dial.
  fn addr(&self) -> std::net::SocketAddr {
    self.addr
  }

  /// Forwards new connections to `upstream`, or closes them as accepted when there is none.
  fn set_upstream(&self, upstream: Option<std::net::SocketAddr>) {
    self.upstream.send_replace(upstream);
  }

  /// Closes every connection the front carries, abruptly, like a network failure: unlike a listener shutdown, which lets requests
  /// in flight finish.
  fn sever(&self) {
    self.epoch.send_modify(|epoch| *epoch += 1);
  }
}

#[test_context(Project)]
#[tokio::test]
async fn a_stream_continues_across_a_listener_restart_without_gap_or_duplicate(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let server = serve(&gateway).await;
  let front = Front::start(Some(server.local_addr())).await;
  let ws = ws_client_at(front.addr()).await;
  let channel = channel_client(&gateway).await;
  let opened = open_new(&channel).await;
  let mut states = ws.watch_connection();
  // Two subscribers from the start: one over the connection that will drop, one over the channel as the reference.
  let mut stream = subscribe(&ws, opened.live, None).await.unwrap();
  let mut reference = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(
    &channel,
    opened.live,
    "Write the numbers from 1 to 300, one per line, nothing else.",
  )
  .await;
  let mut got = Vec::new();
  for _ in 0..4 {
    got.push(next_event(&mut stream).await);
  }

  server.shutdown().await.unwrap();
  wait_for_state(&mut states, reconnecting).await;
  // While reconnecting, a call fails at once and is not queued.
  let error = Error::from(LiveApiClient::list(&ws).await.unwrap_err());
  assert!(matches!(error, Error::Disconnected(_)), "{error:?}");
  let server = serve(&gateway).await;
  front.set_upstream(Some(server.local_addr()));
  wait_for_state(&mut states, connected).await;
  until_settled(&mut stream, &mut got).await;

  let mut want = Vec::new();
  until_settled(&mut reference, &mut want).await;
  assert_eq!(got, want);
  assert!(strictly_increasing(&got));
  drop((stream, reference, ws, channel, front));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_restarted_gateway_ends_the_stream_with_one_resync(project: &mut Project) {
  project.sessions(1);
  let first = project.live_gateway();
  let server = serve(&first).await;
  let front = Front::start(Some(server.local_addr())).await;
  let ws = ws_client_at(front.addr()).await;
  let opened = open_new(&ws).await;
  let mut states = ws.watch_connection();
  let mut stream = subscribe(&ws, opened.live, None).await.unwrap();

  server.shutdown().await.unwrap();
  first.shutdown().await.unwrap();
  wait_for_state(&mut states, reconnecting).await;
  // A new gateway has an empty registry: the live session is gone.
  let second = project.live_gateway();
  let server = serve(&second).await;
  front.set_upstream(Some(server.local_addr()));
  wait_for_state(&mut states, connected).await;

  // Events published before the drop may still be buffered; the resync follows them, once, and ends the stream.
  let resyncs = async {
    let mut resyncs = Vec::new();
    while let Some(item) = stream.next().await {
      if let Err(Error::Resync(reason)) = item {
        resyncs.push(reason);
      }
    }
    resyncs
  }
  .await;
  assert_eq!(resyncs, [ResyncReason::UnknownLiveSession]);
  drop((stream, ws, front));
  server.shutdown().await.unwrap();
  second.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn a_call_in_flight_during_a_drop_fails_with_a_disconnect(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let server = serve(&gateway).await;
  let front = Front::start(Some(server.local_addr())).await;
  let ws = ws_client_at(front.addr()).await;
  let opened = open_new(&ws).await;
  let mut states = ws.watch_connection();
  let mut sent = front.sent();
  let before = *sent.borrow_and_update();
  // Blocks until omp closes, so the call is still in flight when the connection is cut.
  let call = {
    let ws = ws.clone();
    tokio::spawn(async move {
      LiveApiClient::bash(
        &ws,
        BashRequest {
          live: opened.live,
          command: "tail -f /dev/null".to_owned(),
        },
      )
      .await
    })
  };
  sent.wait_for(|total| *total > before).await.unwrap();

  front.set_upstream(None);
  front.sever();
  let error = Error::from(call.await.unwrap().unwrap_err());
  assert!(matches!(error, Error::Disconnected(_)), "{error:?}");
  wait_for_state(&mut states, reconnecting).await;
  front.set_upstream(Some(server.local_addr()));
  wait_for_state(&mut states, connected).await;

  drop((ws, front));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

/// Reads events until the queue holds exactly these steering and follow-up messages, and returns that update.
async fn until_queue(stream: &mut EventStream, steering: &[&str], follow_up: &[&str]) -> alvin_protocol::QueueUpdateEvent {
  loop {
    if let EventBody::Agent(RpcAgentEvent::QueueUpdate(update)) = next_event(stream).await.body
      && update.steering == steering
      && update.follow_up == follow_up
    {
      return update;
    }
  }
}

fn message(live: LiveSessionId, text: &str) -> PromptRequest {
  PromptRequest {
    live,
    message: text.to_owned(),
  }
}

#[test_context(Project)]
#[tokio::test]
async fn the_queue_is_edited_while_a_turn_runs_over_both_transports(project: &mut Project) {
  project.sessions(1);
  let gateway = project.ask_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(
    &channel,
    opened.live,
    "Use the ask tool exactly once to ask me which colour I prefer, with the options red and blue.",
  )
  .await;
  // An unanswered dialog keeps the turn running until restore aborts it.
  until_ask(&mut stream).await;

  LiveApiClient::follow_up(&channel, message(opened.live, "queued one"))
    .await
    .unwrap();
  until_queue(&mut stream, &[], &["queued one"]).await;

  LiveApiClient::follow_up(&ws, message(opened.live, "queued two")).await.unwrap();
  until_queue(&mut stream, &[], &["queued one", "queued two"]).await;
  let removed = QueueApiClient::remove(
    &channel,
    RemoveQueuedRequest {
      live: opened.live,
      message: "queued two".to_owned(),
      queue: QueuedMessageQueue::FollowUp,
    },
  )
  .await
  .unwrap();
  assert!(removed.removed);
  until_queue(&mut stream, &[], &["queued one"]).await;

  LiveApiClient::follow_up(&channel, message(opened.live, "queued three"))
    .await
    .unwrap();
  until_queue(&mut stream, &[], &["queued one", "queued three"]).await;
  let restored = QueueApiClient::restore(&ws, opened.live).await.unwrap();
  assert!(restored.steering.is_empty());
  assert_eq!(
    restored.follow_up.iter().map(|message| message.text.as_str()).collect::<Vec<_>>(),
    ["queued one", "queued three"]
  );
  drop(stream);

  // Promotion interrupts the pending tool, so test it in a separate turn.
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(
    &channel,
    opened.live,
    "Use the ask tool exactly once to ask me which colour I prefer, with the options red and blue.",
  )
  .await;
  until_ask(&mut stream).await;
  LiveApiClient::follow_up(&channel, message(opened.live, "queued one"))
    .await
    .unwrap();
  until_queue(&mut stream, &[], &["queued one"]).await;
  let promoted = QueueApiClient::promote(
    &ws,
    PromoteQueuedRequest {
      live: opened.live,
      message: "queued one".to_owned(),
    },
  )
  .await
  .unwrap();
  assert!(promoted.promoted);
  until_queue(&mut stream, &["queued one"], &[]).await;
  until_queue(&mut stream, &[], &[]).await;

  drop((stream, channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn model_and_thinking_level_change_and_show_in_state_over_both_transports(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;

  let models = ModelApiClient::models(&channel, opened.live).await.unwrap();
  let current = LiveApiClient::state(&ws, opened.live).await.unwrap().model.unwrap();
  assert!(
    models
      .iter()
      .any(|model| model.id == current.id && model.provider == current.provider)
  );

  // Another model, set through one transport, is what the other transport's `live/state` reports.
  let other = models
    .iter()
    .find(|model| model.id != current.id)
    .expect("omp offers a second model");
  let set = ModelApiClient::set_model(
    &channel,
    SetModelRequest {
      live: opened.live,
      provider: other.provider.clone(),
      model_id: other.id.clone(),
    },
  )
  .await
  .unwrap();
  assert_eq!(set.id, other.id);
  assert_eq!(LiveApiClient::state(&ws, opened.live).await.unwrap().model.unwrap().id, other.id);

  // Cycling lands on a model omp lists, and `live/state` agrees.
  if let Some(cycled) = ModelApiClient::cycle_model(&ws, opened.live).await.unwrap() {
    assert!(models.iter().any(|model| model.id == cycled.model.id));
    assert_eq!(
      LiveApiClient::state(&channel, opened.live).await.unwrap().model.unwrap().id,
      cycled.model.id
    );
  }

  // Back to the original model, which accepts the thinking levels below.
  ModelApiClient::set_model(
    &ws,
    SetModelRequest {
      live: opened.live,
      provider: current.provider.clone(),
      model_id: current.id.clone(),
    },
  )
  .await
  .unwrap();
  let levels = ModelApiClient::thinking_levels(&channel, opened.live).await.unwrap();
  // omp reports `auto` as a cycle step, which omp-rpc cannot decode; start where the next step is a concrete level.
  assert!(levels.contains(&ThinkingLevel::Low) && levels.contains(&ThinkingLevel::High));
  ModelApiClient::set_thinking(
    &ws,
    SetThinkingRequest {
      live: opened.live,
      level: ThinkingLevel::Low,
    },
  )
  .await
  .unwrap();
  assert_eq!(
    LiveApiClient::state(&channel, opened.live).await.unwrap().thinking_level,
    Some(ThinkingLevel::Low)
  );
  let cycled = ModelApiClient::cycle_thinking(&channel, opened.live)
    .await
    .unwrap()
    .expect("a level to cycle to");
  // The step after Low depends on the levels the model offers.
  assert_ne!(cycled.level.as_str(), ThinkingLevel::Low.as_str());
  assert!(levels.iter().any(|level| level.as_str() == cycled.level.as_str()));
  assert_eq!(
    LiveApiClient::state(&ws, opened.live)
      .await
      .unwrap()
      .thinking_level
      .map(|level| level.as_str().to_owned()),
    Some(cycled.level.as_str().to_owned())
  );

  drop((channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

/// Reads events until omp opens an `ask` dialog, and returns its request.
async fn until_ask(stream: &mut EventStream) -> alvin_protocol::AskUiRequest {
  loop {
    if let EventBody::ExtensionUiRequest(ExtensionUiRequest::Ask(ask)) = next_event(stream).await.body {
      return ask;
    }
  }
}

#[test_context(Project)]
#[tokio::test]
async fn an_ask_dialog_is_answered_over_both_transports_and_the_answer_reaches_omp(project: &mut Project) {
  project.sessions(1);
  let gateway = project.ask_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(
    &channel,
    opened.live,
    "Use the ask tool exactly once to ask me which colour I prefer, with the options red and blue. \
   Then reply with the single word I chose.",
  )
  .await;

  let ask = until_ask(&mut stream).await;
  let question = ask.questions.first().expect("one question");
  let answers = vec![AskAnswer {
    id: question.id.clone(),
    selected_options: vec![question.options.first().expect("an option").label.clone()],
    custom_input: None,
  }];
  // One transport answers; the other sees the dialog's outcome in the same event log.
  DialogApiClient::respond(
    &ws,
    RespondRequest {
      live: opened.live,
      response: ExtensionUiResponse::AnswersUiResponse(AnswersUiResponse {
        id: ask.id.clone(),
        answers,
      }),
    },
  )
  .await
  .unwrap();

  // omp got the answer: the turn goes on past the dialog and settles.
  let mut seen = Vec::new();
  until_settled(&mut stream, &mut seen).await;
  assert!(
    seen
      .iter()
      .any(|event| matches!(&event.body, EventBody::Agent(RpcAgentEvent::ToolExecutionEnd(_))))
  );

  // An unknown live session is a typed error.
  let error = DialogApiClient::respond(
    &channel,
    RespondRequest {
      live: LiveSessionId::new(),
      response: ExtensionUiResponse::ConfirmUiResponse(alvin_protocol::ConfirmUiResponse {
        id: "none".to_owned(),
        confirmed: true,
      }),
    },
  )
  .await
  .unwrap_err();
  assert!(matches!(
    Error::from(error),
    Error::Gateway {
      code: ErrorCode::UnknownLiveSession,
      ..
    }
  ));

  drop((stream, channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn history_entries_agree_over_both_transports_and_since_cuts_them(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  prompt(&channel, opened.live, "Reply with the single word ok.").await;
  let mut seen = Vec::new();
  until_settled(&mut stream, &mut seen).await;

  let all = LiveApiClient::entries(
    &ws,
    EntriesRequest {
      live: opened.live,
      since: None,
    },
  )
  .await
  .unwrap();
  let over_channel = LiveApiClient::entries(
    &channel,
    EntriesRequest {
      live: opened.live,
      since: None,
    },
  )
  .await
  .unwrap();
  assert_eq!(all, over_channel);
  assert!(all.entries.iter().any(|entry| entry.kind.as_str() == "message"));
  let leaf = all.leaf.clone().expect("a session with a turn has a leaf");
  assert!(all.entries.iter().any(|entry| entry.id == leaf));

  // `since` cuts the entries after one.
  let first = all.entries.first().expect("entries").id.clone();
  let after_first = LiveApiClient::entries(
    &channel,
    EntriesRequest {
      live: opened.live,
      since: Some(first),
    },
  )
  .await
  .unwrap();
  assert_eq!(after_first.entries, all.entries[1..]);

  // An id omp does not know is its command error, with omp's code.
  let unknown = LiveApiClient::entries(
    &ws,
    EntriesRequest {
      live: opened.live,
      since: Some(EntryId::from("no-such-entry".to_owned())),
    },
  )
  .await
  .unwrap_err();
  assert!(matches!(
    Error::from(unknown),
    Error::Gateway {
      code: ErrorCode::OmpCommandFailed,
      data: Some(ErrorData::OmpCommandFailed { code: Some(code) }),
      ..
    } if code == "unknown_since"
  ));

  drop((stream, channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

/// Appends `count` chained user messages to a session file real omp wrote: one branch as deep as a long conversation.
/// Returns the id of the last entry and how many entries the file has now.
fn extend_session(file: &Path, count: usize) -> (String, usize) {
  let text = std::fs::read_to_string(file).unwrap();
  let mut records: Vec<String> = text.lines().map(str::to_owned).collect();
  let entry_id = |line: &str| -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    value.get("parentId")?;
    value.get("id")?.as_str().map(str::to_owned)
  };
  let user = records
    .iter()
    .find(|line| line.contains("\"role\":\"user\""))
    .expect("the turn wrote a user message")
    .clone();
  let mut parent = records
    .iter()
    .rev()
    .find_map(|line| entry_id(line))
    .expect("the turn wrote entries");
  for n in 0..count {
    let id = format!("{n:08x}");
    let mut value = serde_json::from_str::<serde_json::Value>(&user).unwrap();
    value["id"] = serde_json::Value::String(id.clone());
    value["parentId"] = serde_json::Value::String(parent.clone());
    records.push(value.to_string());
    parent = id;
  }
  records.push(String::new());
  std::fs::write(file, records.join("\n")).unwrap();
  let entries = records.iter().filter(|line| entry_id(line).is_some()).count();
  (parent, entries)
}

#[test_context(Project)]
#[tokio::test]
async fn a_long_session_history_crosses_the_wire(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;

  // A session real omp wrote, grown to hundreds of entries on one branch, then resumed.
  let created = open_new(&channel).await;
  let mut stream = subscribe(&channel, created.live, None).await.unwrap();
  prompt(&channel, created.live, "Reply with the single word ok.").await;
  until_settled(&mut stream, &mut Vec::new()).await;
  drop(stream);
  LiveApiClient::close(&channel, created.live).await.unwrap();
  let (last, entries) = extend_session(&created.file, 300);
  let (project_id, _checkout) = only_project(&channel).await;
  let page = SessionsApiClient::list(&channel, request(project_id, None, 10)).await.unwrap();
  let listed = page
    .sessions
    .iter()
    .find(|session| session.path == created.file)
    .expect("the turn wrote a session file");
  let opened = open_session(&channel, listed.id.as_str()).await;

  let request = || EntriesRequest {
    live: opened.live,
    since: None,
  };
  let over_channel = LiveApiClient::entries(&channel, request()).await.unwrap();
  let over_ws = LiveApiClient::entries(&ws, request()).await.unwrap();
  assert_eq!(over_channel, over_ws);
  assert_eq!(over_ws.entries.len(), entries);
  assert_eq!(over_ws.leaf, Some(EntryId::from(last)));

  drop((channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

/// Runs one turn to its end: its prompt result says the session settled. The session event that follows belongs to this
/// turn too; waiting for it would end the next turn early, so it is ignored.
async fn turn(client: &Client, stream: &mut EventStream, live: LiveSessionId, message: &str) {
  prompt(client, live, message).await;
  loop {
    if let EventBody::PromptResult(result) = next_event(stream).await.body
      && result.session_settled
    {
      return;
    }
  }
}

fn user_text(entry: &alvin_protocol::Entry) -> Option<String> {
  let message = entry.data.get("message")?;
  (message.get("role")?.as_str() == Some("user")).then_some(())?;
  message
    .get("content")?
    .as_array()?
    .iter()
    .find_map(|block| block.get("text")?.as_str().map(str::to_owned))
}

#[test_context(Project)]
#[tokio::test]
async fn the_history_tree_is_navigated_labelled_branched_and_forked_over_both_transports(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  turn(&channel, &mut stream, opened.live, "Reply with the single word one.").await;
  turn(&channel, &mut stream, opened.live, "Reply with the single word two.").await;
  let all = |client: &Client| {
    let client = client.clone();
    async move {
      LiveApiClient::entries(
        &client,
        EntriesRequest {
          live: opened.live,
          since: None,
        },
      )
      .await
      .unwrap()
    }
  };

  // The branch candidates are the user messages, with the entry ids the tree lists.
  let candidates = LiveApiClient::branch_messages(&channel, opened.live).await.unwrap();
  assert_eq!(candidates.len(), 2);
  let entries = all(&ws).await;
  for candidate in &candidates {
    let entry = entries
      .entries
      .iter()
      .find(|entry| entry.id.as_str() == candidate.entry_id)
      .expect("a candidate is an entry");
    assert_eq!(user_text(entry).as_deref(), Some(candidate.text.as_str()));
  }
  let first = EntryId::from(candidates[0].entry_id.clone());
  let second = EntryId::from(candidates[1].entry_id.clone());

  // A label is an entry that targets another; clearing it is one more.
  LiveApiClient::label(
    &ws,
    LabelRequest {
      live: opened.live,
      entry: first.clone(),
      label: Some("start".to_owned()),
    },
  )
  .await
  .unwrap();
  let after_label = all(&channel).await;
  let label = after_label.entries.last().expect("entries");
  assert_eq!(label.kind.as_str(), "label");
  assert_eq!(label.data.get("targetId").and_then(|target| target.as_str()), Some(first.as_str()));
  assert_eq!(label.data.get("label").and_then(|label| label.as_str()), Some("start"));
  LiveApiClient::label(
    &channel,
    LabelRequest {
      live: opened.live,
      entry: first.clone(),
      label: None,
    },
  )
  .await
  .unwrap();
  let after_clear = all(&ws).await;
  assert_eq!(after_clear.entries.len(), after_label.entries.len() + 1);

  // Navigating to a user message rewinds past it: the leaf moves to its parent and its text comes back for the composer.
  let second_entry = entries
    .entries
    .iter()
    .find(|entry| entry.id == second)
    .expect("the second prompt")
    .clone();
  let navigated = LiveApiClient::navigate_tree(
    &channel,
    NavigateRequest {
      live: opened.live,
      entry: second.clone(),
      summarize: None,
      custom_instructions: None,
    },
  )
  .await
  .unwrap();
  assert!(!navigated.cancelled);
  assert_eq!(navigated.editor_text, user_text(&second_entry));
  let moved = all(&ws).await;
  assert_eq!(moved.leaf, second_entry.parent_id);
  let transcript = LiveApiClient::messages(
    &channel,
    MessagesRequest {
      live: opened.live,
      cursor: None,
      limit: None,
    },
  )
  .await
  .unwrap();
  assert_eq!(transcript.total_messages, 2);

  // An entry omp does not know is its command error with omp's code.
  let unknown = LiveApiClient::navigate_tree(
    &ws,
    NavigateRequest {
      live: opened.live,
      entry: EntryId::from("nope".to_owned()),
      summarize: None,
      custom_instructions: None,
    },
  )
  .await
  .unwrap_err();
  assert!(matches!(
    Error::from(unknown),
    Error::Gateway { data: Some(ErrorData::OmpCommandFailed { code: Some(code) }), .. } if code == "entry_not_found"
  ));

  // A fork moves the live session onto a new file, and the gateway files it under that one.
  let before = LiveApiClient::list(&channel).await.unwrap();
  let forked = LiveApiClient::fork(
    &ws,
    ForkRequest {
      live: opened.live,
      entry: None,
    },
  )
  .await
  .unwrap();
  assert!(!forked.cancelled);
  let after = LiveApiClient::list(&channel).await.unwrap();
  assert_eq!(after.len(), 1);
  assert_ne!(after[0].file, before[0].file);
  assert_eq!(after[0].live, opened.live);
  let state = LiveApiClient::state(&channel, opened.live).await.unwrap();
  assert_eq!(state.session_file.map(PathBuf::from), Some(after[0].file.clone()));

  // A branch at a user message moves it again or not, and the summary agrees with omp either way.
  let branched = LiveApiClient::branch(
    &channel,
    EntryRequest {
      live: opened.live,
      entry: first,
    },
  )
  .await
  .unwrap_or_else(|error| panic!("branching at a user message failed: {error:?}"));
  assert!(!branched.cancelled);
  let listed = LiveApiClient::list(&ws).await.unwrap();
  let state = LiveApiClient::state(&ws, opened.live).await.unwrap();
  assert_eq!(state.session_file.map(PathBuf::from), Some(listed[0].file.clone()));

  drop((stream, channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn session_operations_work_over_both_transports(project: &mut Project) {
  project.sessions(1);
  let gateway = project.live_gateway();
  let ([channel, ws], server) = both_transports(&gateway).await;
  let opened = open_new(&channel).await;
  let mut stream = subscribe(&channel, opened.live, None).await.unwrap();
  turn(&channel, &mut stream, opened.live, "Reply with the single word one.").await;
  // The name is omp's: `live/state` reports it and so does the session index.
  LiveApiClient::rename(
    &ws,
    RenameRequest {
      live: opened.live,
      name: "A new name".to_owned(),
    },
  )
  .await
  .unwrap();
  let state = LiveApiClient::state(&channel, opened.live).await.unwrap();
  assert_eq!(state.session_name.as_deref(), Some("A new name"));

  // Counts of what the session holds.
  let stats = LiveApiClient::stats(&channel, opened.live).await.unwrap();
  assert_eq!((stats.user_messages, stats.assistant_messages, stats.total_messages), (1, 1, 2));
  assert_eq!(stats, LiveApiClient::stats(&ws, opened.live).await.unwrap());

  // The slash commands omp offers.
  let commands = LiveApiClient::commands(&ws, opened.live).await.unwrap();
  assert!(!commands.is_empty());
  assert_eq!(commands, LiveApiClient::commands(&channel, opened.live).await.unwrap());

  // The page lands where asked.
  let target = project.root.join("export.html");
  let written = LiveApiClient::export_html(
    &channel,
    ExportRequest {
      live: opened.live,
      output_path: Some(target.clone()),
    },
  )
  .await
  .unwrap();
  assert!(Path::new(&written).ends_with("export.html"), "{written}");
  assert!(target.is_file());

  drop((stream, channel, ws));
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn terminal_shell_output_and_resume_work_over_both_transports(project: &mut Project) {
  use std::num::NonZeroU16;

  project.sessions(1);
  let gateway = project.gateway();
  let (clients, server) = both_transports(&gateway).await;
  let size = TerminalSize {
    columns: NonZeroU16::new(80).unwrap(),
    rows: NonZeroU16::new(24).unwrap(),
    pixel_width: 800,
    pixel_height: 480,
  };
  for client in &clients {
    let (project_id, checkout) = only_project(client).await;
    let mut stream = TerminalApiClient::terminal_events(
      client,
      TerminalSubscription::Open {
        target: OpenTarget::New {
          project: project_id,
          checkout,
        },
        size,
      },
    )
    .await
    .unwrap();
    let started = stream.next().await.unwrap().unwrap();
    assert!(matches!(started.body, TerminalEventBody::Started));
    let terminal = started.terminal;
    TerminalApiClient::accepted(
      client,
      TerminalAccepted {
        terminal,
        through: started.id,
      },
    )
    .await
    .unwrap();
    let stale = TerminalApiClient::terminal_events(client, TerminalSubscription::Resume { terminal, after: None })
      .await
      .unwrap_err();
    assert!(matches!(
      Error::from(stale),
      Error::Gateway {
        code: ErrorCode::StaleCursor,
        ..
      }
    ));
    let resized = TerminalSize {
      columns: NonZeroU16::new(101).unwrap(),
      rows: NonZeroU16::new(31).unwrap(),
      ..size
    };
    TerminalApiClient::resize(client, TerminalResize { terminal, size: resized })
      .await
      .unwrap();
    let command = if cfg!(windows) {
      b"echo ALVIN_TERMINAL_OUTPUT\r\nexit\r\n".to_vec()
    } else {
      b"printf 'ALVIN_TERMINAL_OUTPUT\\n'; pwd; stty size; exit\n".to_vec()
    };
    TerminalApiClient::input(client, TerminalInput { terminal, bytes: command })
      .await
      .unwrap();
    drop(stream);
    let mut resumed = TerminalApiClient::terminal_events(
      client,
      TerminalSubscription::Resume {
        terminal,
        after: Some(started.id),
      },
    )
    .await
    .unwrap();
    let mut output = Vec::new();
    let mut last = started.id;
    loop {
      let event = resumed.next().await.unwrap().unwrap();
      assert!(event.id > last);
      last = event.id;
      TerminalApiClient::accepted(
        client,
        TerminalAccepted {
          terminal,
          through: event.id,
        },
      )
      .await
      .unwrap();
      match event.body {
        TerminalEventBody::Output { bytes } => output.extend(bytes),
        TerminalEventBody::Exited { code, signal } => {
          assert_eq!(code, Some(0));
          assert_eq!(signal, None);
          break;
        }
        TerminalEventBody::Failed { code, message } => panic!("terminal failed {code}: {message}"),
        TerminalEventBody::Started => panic!("started event repeated after cursor"),
      }
    }
    assert!(String::from_utf8_lossy(&output).contains("ALVIN_TERMINAL_OUTPUT"));
    if !cfg!(windows) {
      assert!(String::from_utf8_lossy(&output).contains(project.root.to_str().unwrap()));
      assert!(String::from_utf8_lossy(&output).contains("31 101"));
    }
    TerminalApiClient::close(client, terminal).await.unwrap();
  }
  drop(clients);
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[cfg(unix)]
#[test_context(Project)]
#[tokio::test]
async fn terminal_backpressure_preserves_large_output_over_both_transports(project: &mut Project) {
  use std::num::NonZeroU16;

  project.sessions(1);
  let gateway = project.gateway();
  let (clients, server) = both_transports(&gateway).await;
  let size = TerminalSize {
    columns: NonZeroU16::new(80).unwrap(),
    rows: NonZeroU16::new(24).unwrap(),
    pixel_width: 800,
    pixel_height: 480,
  };
  for client in &clients {
    let (project_id, checkout) = only_project(client).await;
    let mut stream = TerminalApiClient::terminal_events(
      client,
      TerminalSubscription::Open {
        target: OpenTarget::New {
          project: project_id,
          checkout,
        },
        size,
      },
    )
    .await
    .unwrap();
    let started = stream.next().await.unwrap().unwrap();
    assert!(matches!(started.body, TerminalEventBody::Started));
    let terminal = started.terminal;
    TerminalApiClient::accepted(
      client,
      TerminalAccepted {
        terminal,
        through: started.id,
      },
    )
    .await
    .unwrap();
    TerminalApiClient::input(
      client,
      TerminalInput {
        terminal,
        bytes: b"stty -echo; printf '\\001READY\\002'\n".to_vec(),
      },
    )
    .await
    .unwrap();
    let mut ready = Vec::new();
    while !ready.windows(7).any(|part| part == b"\x01READY\x02") {
      let event = stream.next().await.unwrap().unwrap();
      TerminalApiClient::accepted(
        client,
        TerminalAccepted {
          terminal,
          through: event.id,
        },
      )
      .await
      .unwrap();
      match event.body {
        TerminalEventBody::Output { bytes } => ready.extend(bytes),
        other => panic!("unexpected terminal event: {other:?}"),
      }
    }
    TerminalApiClient::input(
      client,
      TerminalInput {
        terminal,
        bytes: b"printf '\\001START\\002'; head -c 2097152 /dev/zero | tr '\\000' X; printf '\\001END\\002'; exit\n".to_vec(),
      },
    )
    .await
    .unwrap();
    let mut output = Vec::new();
    loop {
      let event = stream.next().await.unwrap().unwrap();
      TerminalApiClient::accepted(
        client,
        TerminalAccepted {
          terminal,
          through: event.id,
        },
      )
      .await
      .unwrap();
      match event.body {
        TerminalEventBody::Output { bytes } => output.extend(bytes),
        TerminalEventBody::Exited { code, signal } => {
          assert_eq!((code, signal), (Some(0), None));
          break;
        }
        other => panic!("unexpected terminal event: {other:?}"),
      }
    }
    let start = output.windows(7).position(|part| part == b"\x01START\x02").unwrap() + 7;
    let end = output.windows(5).position(|part| part == b"\x01END\x02").unwrap();
    let bytes = output.get(start..end).unwrap();
    assert_eq!(bytes.len(), 2_097_152);
    assert!(bytes.iter().all(|byte| *byte == b'X'));
    TerminalApiClient::close(client, terminal).await.unwrap();
  }
  drop(clients);
  server.shutdown().await.unwrap();
  gateway.shutdown().await.unwrap();
}

#[test_context(Project)]
#[tokio::test]
async fn terminal_shutdown_closes_shells_and_rejects_new_opens(project: &mut Project) {
  use std::num::NonZeroU16;

  project.sessions(1);
  let gateway = project.gateway();
  let (clients, server) = both_transports(&gateway).await;
  let size = TerminalSize {
    columns: NonZeroU16::new(80).unwrap(),
    rows: NonZeroU16::new(24).unwrap(),
    pixel_width: 800,
    pixel_height: 480,
  };
  let (project_id, checkout) = only_project(&clients[0]).await;
  let target = OpenTarget::New {
    project: project_id,
    checkout,
  };
  let mut terminals = Vec::new();
  let mut streams = Vec::new();
  for client in &clients {
    let mut stream = TerminalApiClient::terminal_events(
      client,
      TerminalSubscription::Open {
        target: target.clone(),
        size,
      },
    )
    .await
    .unwrap();
    let started = stream.next().await.unwrap().unwrap();
    assert!(matches!(started.body, TerminalEventBody::Started));
    TerminalApiClient::accepted(
      client,
      TerminalAccepted {
        terminal: started.terminal,
        through: started.id,
      },
    )
    .await
    .unwrap();
    terminals.push(started.terminal);
    streams.push(stream);
  }
  gateway.shutdown().await.unwrap();
  for (client, terminal) in clients.iter().zip(terminals) {
    let closed = TerminalApiClient::input(
      client,
      TerminalInput {
        terminal,
        bytes: b"test".to_vec(),
      },
    )
    .await
    .unwrap_err();
    assert!(matches!(
      Error::from(closed),
      Error::Gateway {
        code: ErrorCode::UnknownTerminal,
        ..
      }
    ));
    let reopened = TerminalApiClient::terminal_events(
      client,
      TerminalSubscription::Open {
        target: target.clone(),
        size,
      },
    )
    .await
    .unwrap_err();
    assert!(matches!(
      Error::from(reopened),
      Error::Gateway {
        code: ErrorCode::CommandNotAllowed,
        ..
      }
    ));
  }
  drop((streams, clients));
  server.shutdown().await.unwrap();
}
