//! Wiring: a gateway (embedded or remote), a client, the window, and a quit that leaves no omp process behind.
//!
//! GPUI gives `on_app_quit` hooks 200 ms, which is not enough to close omp sessions. So quitting goes through [`Quit`] or
//! the last window closing, which shut the gateway down in full and only then ask GPUI to quit. The hook stays as a
//! best-effort start for the cases that bypass both, such as a platform quit.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_conversation_ui::{Model, Workspace};
use alvin_gateway::Gateway;
use gpui_kit::{App, AppContext as _, Bounds, Entity, KeyBinding, WindowBounds, WindowOptions, actions, px, size};

use crate::config::{Settings, gateway_config};

actions!(
  alvin_app,
  [
    /// Closes every live session and quits.
    Quit
  ]
);

/// The platform's command key plus `q`.
#[cfg(target_os = "macos")]
const QUIT_KEY: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT_KEY: &str = "ctrl-q";

/// The gateway the app embeds, if any. A remote gateway is not the app's to shut down: its sessions stay live.
#[derive(Clone)]
pub(crate) struct Embedded(Option<Gateway>);

impl Embedded {
  async fn shutdown(&self) {
    if let Some(gateway) = &self.0
      && let Err(error) = gateway.shutdown().await
    {
      tracing::error!(%error, "closing the live sessions on quit");
    }
  }
}

/// Connects per the settings: embeds a gateway over the in-process channel, or connects to a running one. Call inside the
/// runtime the client's tasks run on.
pub(crate) async fn connect(settings: &Settings) -> eyre::Result<(Client, Embedded)> {
  match settings {
    Settings::Embedded { omp, model, sessions_dir } => {
      let gateway = Gateway::new(gateway_config(omp, model.as_deref(), sessions_dir));
      let client = Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(64)?))).await?;
      Ok((client, Embedded(Some(gateway))))
    }
    Settings::Remote(url) => Ok((Client::connect(Endpoint::WebSocket(url.clone())).await?, Embedded(None))),
  }
}

/// Opens the window and registers the quit paths. Returns the workspace.
pub(crate) fn start(cx: &mut App, client: Client, embedded: Embedded) -> eyre::Result<Entity<Workspace>> {
  gpui_kit::init(cx);
  alvin_conversation_ui::init(cx);
  cx.bind_keys([KeyBinding::new(QUIT_KEY, Quit, None)]);

  let quitting = Rc::new(Cell::new(false));
  let on_quit = {
    let embedded = embedded.clone();
    let quitting = Rc::clone(&quitting);
    move |cx: &mut App| quit(&embedded, &quitting, cx)
  };
  cx.on_action({
    let on_quit = on_quit.clone();
    move |_: &Quit, cx| on_quit(cx)
  });
  cx.on_window_closed({
    let on_quit = on_quit.clone();
    move |cx, _window| {
      if cx.windows().is_empty() {
        on_quit(cx);
      }
    }
  })
  .detach();
  // Best effort: GPUI cuts this off after 200 ms.
  cx.on_app_quit({
    let embedded = embedded.clone();
    move |_| {
      let embedded = embedded.clone();
      async move { embedded.shutdown().await }
    }
  })
  .detach();

  let (_window, workspace) = gpui_kit::open_window(
    WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1200.), px(800.))))),
      ..Default::default()
    },
    cx,
    |window, cx| {
      let model = cx.new(|cx| Model::new(client, cx));
      model.update(cx, |model, cx| model.refresh_projects(cx)).detach();
      cx.new(|cx| Workspace::new(model, window, cx))
    },
  )
  .map_err(|error| eyre::eyre!("opening the main window: {error}"))?;
  Ok(workspace)
}

/// Shuts the embedded gateway down in full, then quits. Does nothing the second time.
fn quit(embedded: &Embedded, quitting: &Rc<Cell<bool>>, cx: &mut App) {
  if quitting.replace(true) {
    return;
  }
  let embedded = embedded.clone();
  cx.spawn(async move |cx| {
    embedded.shutdown().await;
    cx.update(|cx| cx.quit());
  })
  .detach();
}

/// Runs the app until the user quits: builds the runtime, connects, and runs GPUI on this thread. Flushing telemetry is the
/// caller's, after this returns.
pub fn run(settings: &Settings) -> eyre::Result<()> {
  let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
  // GPUI runs on this thread; the guard gives its futures, and the client's, the runtime's timers and tasks.
  let _entered = runtime.enter();
  let (client, embedded) = runtime.block_on(connect(settings))?;
  let failure = Rc::new(RefCell::new(None));
  let reported = Rc::clone(&failure);
  gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
    if let Err(error) = start(cx, client, embedded) {
      *reported.borrow_mut() = Some(error);
      cx.quit();
    }
  });
  failure.take().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
  use alvin_client::{ChannelPair, Client, Endpoint};
  use alvin_gateway::Gateway;
  use alvin_protocol::{BashRequest, LiveApiClient};
  use gpui_kit::TestAppContext;

  use super::{Embedded, Quit, start};
  use crate::test_support::{Child, Fixture, sleeper};

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_window_lists_the_project_opens_a_session_and_quit_leaves_no_omp(cx: &mut TestAppContext) {
    // `#[tokio::test]` polls this body, so GPUI's scheduler would never step the tasks it awaits. GPUI's executor drives the
    // body instead, while tokio's runtime context stays entered for the client and gateway tasks.
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
    let fixture = Fixture::new();
    let gateway = Gateway::new(fixture.config());
    let channel = |capacity| ChannelPair::from(gateway.connect_channel(capacity).unwrap());
    let client = Client::connect(Endpoint::Channel(channel(64))).await.unwrap();
    // A second connection to the same gateway: it runs a command in the session and sees that quit closed it.
    let probe = Client::connect(Endpoint::Channel(channel(16))).await.unwrap();

    let workspace = cx.update(|cx| start(cx, client, Embedded(Some(gateway.clone())))).unwrap();
    let model = cx.read(|cx| workspace.read(cx).model().clone());
    cx.update(|cx| model.update(cx, |model, cx| model.refresh_projects(cx))).await;

    let (project, checkout) = cx.read(|cx| {
      let project = model.read(cx).conversation().workspace().projects()[0].clone();
      (project.id, project.checkouts[0].id.clone())
    });
    cx.update(|cx| model.update(cx, |model, cx| model.new_session(project, checkout, cx)))
      .await;
    let live = cx.read(|cx| workspace.read(cx).active().unwrap());
    // A foreground command that reports in and keeps running; the call is dropped once it has reported.
    let fifo = fixture.make_fifo();
    let child = tokio::select! {
      result = LiveApiClient::bash(&probe, BashRequest { live, command: sleeper(&fifo) }) => panic!("the sleeper ended on its own: {result:?}"),
      child = Child::report(&fifo) => child,
    };

    cx.update(|cx| cx.dispatch_action(&Quit));
    child.died().await;
    assert_eq!(LiveApiClient::list(&probe).await.unwrap(), vec![]);
    })
  }
}
