//! Black-box tests of the gateway's wire contract, exercised through `alvin-client` over both transports.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use alvin_client::{ChannelPair, Client, Endpoint, Error};
use alvin_gateway::{Gateway, GatewayConfig, WsServer};
use alvin_protocol::{
  CheckoutId, ErrorCode, ErrorData, EventId, ListSessions, ProjectId, ProjectSummary, ProjectsApiClient, SessionCursor, SessionPage,
  SessionStatus, SessionsApiClient,
};

/// Session files written by real omp (the first two records of one run).
fn fixture_sessions() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sessions")
}

/// The records of one real omp run: title slot, header, user message, assistant message.
const SESSION_RECORDS: &str = include_str!("../fixtures/session-records.jsonl");
/// The `cwd` those records were written with.
const RECORDED_CWD: &str = "/tmp/alvin-probe.hnBA/proj";

/// A real git repository with `count` sessions under a sessions directory, session `n` modified `n` seconds
/// after the first. Removed on drop.
struct Project {
  root: PathBuf,
  sessions: PathBuf,
}

impl Project {
  fn new(count: usize) -> Self {
    let base = std::env::temp_dir().join(format!("alvin-gateway-{}", EventId::new().as_uuid()));
    std::fs::create_dir_all(&base).unwrap();
    let base = base.canonicalize().unwrap();
    let root = base.join("repo");
    std::fs::create_dir(&root).unwrap();
    let git = Command::new("git").args(["init", "--quiet"]).current_dir(&root).output().unwrap();
    assert!(git.status.success(), "{}", String::from_utf8_lossy(&git.stderr));

    let sessions = base.join("sessions");
    let dir = sessions.join("-repo");
    std::fs::create_dir_all(&dir).unwrap();
    for n in 0..count {
      let records = SESSION_RECORDS
        .replace(RECORDED_CWD, root.to_str().unwrap())
        .replace("01a18ff1-ab74-73b8-a3a2-79905565c49b", &format!("session-{n}"));
      let file = dir.join(format!("2026-01-01T00-00-0{n}-000Z_session-{n}.jsonl"));
      std::fs::write(&file, records).unwrap();
      let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + n as u64);
      std::fs::File::options().write(true).open(&file).unwrap().set_modified(at).unwrap();
    }
    Self { root, sessions }
  }

  fn gateway(&self) -> Gateway {
    gateway(&self.sessions)
  }
}

impl Drop for Project {
  fn drop(&mut self) {
    if let Some(base) = self.root.parent() {
      // Best effort: a leftover temporary directory must not fail an otherwise passing test.
      drop(std::fs::remove_dir_all(base));
    }
  }
}

fn gateway(sessions_dir: impl Into<PathBuf>) -> Gateway {
  Gateway::new(GatewayConfig::builder().sessions_dir(sessions_dir.into()).build())
}

async fn channel_client(gateway: &Gateway) -> Client {
  Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(16).unwrap())))
    .await
    .unwrap()
}

async fn ws_client(gateway: &Gateway) -> (Client, WsServer) {
  let server = gateway.serve_ws(([127, 0, 0, 1], 0).into()).await.unwrap();
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

#[tokio::test]
async fn projects_list_returns_the_same_projects_over_channel_and_websocket() {
  let project = Project::new(2);
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

#[tokio::test]
async fn sessions_list_pages_newest_first_and_matches_over_both_transports() {
  let project = Project::new(3);
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

#[tokio::test]
async fn an_unknown_project_or_checkout_is_a_typed_error_over_both_transports() {
  let project = Project::new(1);
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

#[tokio::test]
async fn a_page_of_no_sessions_is_an_invalid_request() {
  let project = Project::new(1);
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
