//! Helpers for headless UI tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_conversation::SessionsKey;
use alvin_gateway::HostConfig;
use alvin_protocol::{
  CheckoutId, CheckoutSummary, LiveSessionId, LiveSessionSummary, ProjectId, ProjectSummary, SessionPage, SessionSummary,
};
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};
use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::model::Model;
use crate::workspace::Workspace;

/// The root of the one project every test window knows.
pub(crate) const ROOT: &str = "/work/alvin";

pub(crate) fn omp_config(base: &Path, extra_args: Vec<OsString>) -> HostConfig {
  let home = base.join("home");
  let agent = base.join("agent");
  std::fs::create_dir_all(&home).unwrap();
  std::fs::create_dir_all(&agent).unwrap();
  let paths: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
    .map(|dir| {
      if dir.is_absolute() {
        dir
      } else {
        std::env::current_dir().unwrap().join(dir)
      }
    })
    .collect();
  let find = |name: &str| {
    let names = if cfg!(windows) {
      vec![format!("{name}.exe"), format!("{name}.cmd"), name.to_owned()]
    } else {
      vec![name.to_owned()]
    };
    paths
      .iter()
      .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
      .find(|path| path.is_file())
  };
  let omp = find("omp").expect("omp must be on PATH");
  let bun = find("bun").expect("bun must be on PATH");
  let mut runtime_paths = vec![bun.parent().unwrap().to_path_buf()];
  if cfg!(windows) {
    runtime_paths.push(PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot is required")).join("System32"));
  } else {
    runtime_paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
  }
  let path = std::env::join_paths(runtime_paths).unwrap();
  let mut env = vec![
    ("HOME".into(), home.into_os_string()),
    ("PI_CODING_AGENT_DIR".into(), agent.into_os_string()),
    ("PATH".into(), path),
  ];
  if let Some(key) = std::env::var_os("OPENROUTER_API_KEY") {
    env.push(("OPENROUTER_API_KEY".into(), key));
  }
  if cfg!(windows) {
    for name in ["SystemRoot", "TEMP", "TMP"] {
      if let Some(value) = std::env::var_os(name) {
        env.push((OsString::from(name), value));
      }
    }
  }
  HostConfig::builder()
    .omp(omp)
    .extra_args(extra_args)
    .clear_env(true)
    .env(env)
    .build()
}

/// A real gateway and repository whose omp sessions stay in the test directory.
pub(crate) struct RealGateway {
  pub(crate) directory: tempfile::TempDir,
  pub(crate) gateway: alvin_gateway::Gateway,
}

impl test_context::AsyncTestContext for RealGateway {
  async fn setup() -> Self {
    use alvin_gateway::{Gateway, GatewayConfig};

    let directory = tempfile::tempdir().unwrap();
    let repo = directory.path().join("repo");
    let sessions = directory.path().join("sessions");
    let session_dir = sessions.join("-repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&session_dir).unwrap();
    assert!(
      std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success()
    );
    let repo = repo.canonicalize().unwrap();
    let seed = include_str!("../../alvin-app/fixtures/seed-session.jsonl").replace(
      &serde_json::to_string("/tmp/alvin-probe.hnBA/proj").unwrap(),
      &serde_json::to_string(&repo).unwrap(),
    );
    std::fs::write(session_dir.join("2026-01-01T00-00-00-000Z_seed.jsonl"), seed).unwrap();
    let omp = omp_config(directory.path(), vec!["--session-dir".into(), session_dir.into_os_string()]);
    let gateway = Gateway::new(
      GatewayConfig::builder()
        .sessions_dir(sessions)
        .omp(omp)
        .default_model(Some("openrouter/inclusionai/ling-3.0-flash".to_owned()))
        .build(),
    );
    Self { directory, gateway }
  }

  async fn teardown(self) {
    self.gateway.shutdown().await.unwrap();
    self.directory.close().unwrap();
  }
}

#[tokio::test]
async fn real_gateway_fixture_isolates_sessions_and_cleans_up() {
  use alvin_protocol::{LiveApiClient, OpenRequest, OpenTarget, ProjectsApiClient};
  use test_context::AsyncTestContext as _;

  let fixture = RealGateway::setup().await;
  let directory = fixture.directory.path().canonicalize().unwrap();
  let client = Client::connect(Endpoint::Channel(ChannelPair::from(fixture.gateway.connect_channel(64).unwrap())))
    .await
    .unwrap();
  let projects = ProjectsApiClient::list(&client).await.unwrap();
  let project = projects.first().unwrap();
  let session = LiveApiClient::open(
    &client,
    OpenRequest {
      target: OpenTarget::New {
        project: project.id.clone(),
        checkout: project.checkouts.first().unwrap().id.clone(),
      },
      model: None,
    },
  )
  .await
  .unwrap();
  assert!(session.file.starts_with(directory.join("sessions")));
  assert!(directory.join("agent/agent.db").is_file());
  fixture.teardown().await;
  assert!(!directory.exists());
}

/// The gateway end of a channel nobody serves. A client over it is connected, and none of its calls is ever answered, so
/// a view that issues a command has nothing come back to it. Tests put the state a reply would have produced in the model
/// themselves.
pub(crate) struct IdleGateway {
  _inbox: Receiver<String>,
  _outbox: Sender<String>,
}

impl IdleGateway {
  /// A connected client and the gateway end that keeps it connected. Call inside a tokio runtime.
  pub(crate) async fn connect() -> (Client, Self) {
    let (to_gateway, inbox) = channel(4);
    let (outbox, from_gateway) = channel(4);
    let client = Client::connect(Endpoint::Channel(ChannelPair::from((to_gateway, from_gateway))))
      .await
      .unwrap();
    (
      client,
      Self {
        _inbox: inbox,
        _outbox: outbox,
      },
    )
  }
}

/// A listed session with the given id and title.
pub(crate) fn session_summary(id: &str, title: &str) -> SessionSummary {
  serde_json::from_value(serde_json::json!({
    "id": id,
    "path": format!("/sessions/{id}.jsonl"),
    "title": title,
    "created": "2026-01-01T00:00:00Z",
    "modified": "2026-01-01T00:00:00Z",
    "status": "complete",
    "parent": null,
  }))
  .unwrap()
}

/// A live-session event with the given body, as the gateway would have published it.
pub(crate) fn event(live: LiveSessionId, body: serde_json::Value) -> alvin_protocol::Event {
  serde_json::from_value(serde_json::json!({
    "id": alvin_protocol::EventId::new(),
    "live": live,
    "at": "2026-11-01T00:00:00Z",
    "body": body,
  }))
  .unwrap()
}

/// A live session as `live/open` would have reported it.
pub(crate) fn live_session(cwd: &str) -> LiveSessionSummary {
  LiveSessionSummary {
    live: LiveSessionId::new(),
    cwd: PathBuf::from(cwd),
    file: PathBuf::from(cwd).join("session.jsonl"),
    exited: false,
  }
}

/// A window over a workspace whose model knows one project with two sessions and is connected to nothing.
pub(crate) struct Opened {
  pub(crate) handle: AnyWindowHandle,
  pub(crate) workspace: Entity<Workspace>,
  pub(crate) model: Entity<Model>,
  _gateway: IdleGateway,
  _speech_cache: tempfile::TempDir,
}

/// Opens the test window.
pub(crate) async fn open_workspace(cx: &mut TestAppContext) -> Opened {
  cx.update(gpui_kit::init);
  let speech_cache = tempfile::tempdir().expect("create speech cache");
  cx.update(|cx| crate::init(speech_cache.path().to_path_buf(), cx));
  let (client, gateway) = IdleGateway::connect().await;
  let project = ProjectId::from("project".to_owned());
  let checkout = CheckoutId::from("checkout".to_owned());
  let key = SessionsKey {
    project: project.clone(),
    checkout: Some(checkout.clone()),
  };
  let summary = ProjectSummary {
    id: project,
    root: PathBuf::from(ROOT),
    checkouts: vec![CheckoutSummary {
      id: checkout,
      root: PathBuf::from(ROOT),
      vcs: None,
    }],
  };
  let page = SessionPage {
    sessions: vec![
      session_summary("first", "Fix the build"),
      session_summary("second", "Write the docs"),
    ],
    next: None,
  };
  let (handle, workspace, model) = cx.update(|cx| {
    let model = cx.new(|cx| {
      let mut model = Model::new(client.clone(), cx);
      model.seed_workspace(vec![summary], vec![(key, page)], cx);
      model
    });
    let (handle, workspace) = gpui_kit::open_window(
      WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1100.), px(700.))))),
        ..Default::default()
      },
      cx,
      |window, cx| cx.new(|cx| Workspace::new(model.clone(), client, window, cx)),
    )
    .expect("open the test window");
    (handle, workspace, model)
  });
  Opened {
    handle,
    workspace,
    model,
    _gateway: gateway,
    _speech_cache: speech_cache,
  }
}
