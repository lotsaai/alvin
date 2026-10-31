//! Helpers for headless UI tests.

use std::path::PathBuf;

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_conversation::SessionsKey;
use alvin_protocol::{
  CheckoutId, CheckoutSummary, LiveSessionId, LiveSessionSummary, ProjectId, ProjectSummary, SessionPage, SessionSummary,
};
use gpui_kit::{AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size};
use tokio::sync::mpsc::{Receiver, Sender, channel};

use crate::model::Model;
use crate::workspace::Workspace;

/// The root of the one project every test window knows.
pub(crate) const ROOT: &str = "/work/alvin";

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
}

/// Opens the test window.
pub(crate) async fn open_workspace(cx: &mut TestAppContext) -> Opened {
  cx.update(gpui_kit::init);
  cx.update(crate::init);
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
      let mut model = Model::new(client, cx);
      model.seed_workspace(vec![summary], vec![(key, page)], cx);
      model
    });
    let (handle, workspace) = gpui_kit::open_window(
      WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1100.), px(700.))))),
        ..Default::default()
      },
      cx,
      |window, cx| cx.new(|cx| Workspace::new(model.clone(), window, cx)),
    )
    .expect("open the test window");
    (handle, workspace, model)
  });
  Opened {
    handle,
    workspace,
    model,
    _gateway: gateway,
  }
}
