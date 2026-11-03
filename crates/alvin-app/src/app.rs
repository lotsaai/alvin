//! Wiring: a gateway (embedded or remote), a client, the window, and a quit that leaves no omp process behind.
//!
//! GPUI gives `on_app_quit` hooks 200 ms, which is not enough to close omp sessions. So quitting goes through [`Quit`] or
//! the last window closing, which shut the gateway down in full and only then ask GPUI to quit. The hook stays as a
//! best-effort start for the cases that bypass both, such as a platform quit.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use alvin_client::{ChannelPair, Client, Endpoint};
use alvin_conversation_ui::{Model, Workspace};
use alvin_gateway::Gateway;
use gpui_kit::component::TitleBar;
use gpui_kit::{
  App, AppContext as _, AssetSource, Bounds, Entity, KeyBinding, SharedString, WindowBounds, WindowOptions, actions, px, size,
};

use crate::config::{Configuration, Settings, gateway_config};
use crate::project_preferences::ProjectPreferencesFile;
use crate::settings::AppView;

gpui_kit::assets::icon_assets!(ExtraIcons, [GitFork, Smartphone]);

struct AppAssets;

impl AssetSource for AppAssets {
  fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
    if let Some(bytes) = ExtraIcons.load(path)? {
      return Ok(Some(bytes));
    }
    gpui_kit::assets::Assets.load(path)
  }

  fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
    let mut paths = gpui_kit::assets::Assets.list(path)?;
    paths.extend(ExtraIcons.list(path)?);
    paths.sort();
    paths.dedup();
    Ok(paths)
  }
}

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
pub(crate) struct Embedded(Rc<RefCell<Option<Gateway>>>);

impl Embedded {
  pub(crate) fn new(gateway: Option<Gateway>) -> Self {
    Self(Rc::new(RefCell::new(gateway)))
  }

  pub(crate) fn replace(&self, other: &Self) -> Self {
    Self::new(self.0.replace(other.0.borrow_mut().take()))
  }

  pub(crate) async fn shutdown(&self) {
    let gateway = self.0.borrow_mut().take();
    if let Some(gateway) = gateway
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
      Ok((client, Embedded::new(Some(gateway))))
    }
    Settings::Remote(url) => Ok((Client::connect(Endpoint::WebSocket(url.clone())).await?, Embedded::new(None))),
  }
}

fn local_user_name(value: Result<String, std::env::VarError>) -> Result<Option<String>, std::env::VarError> {
  match value {
    Ok(name) if name.trim().is_empty() => Ok(None),
    Ok(name) => Ok(Some(name)),
    Err(std::env::VarError::NotPresent) => Ok(None),
    Err(error) => Err(error),
  }
}

fn read_local_user_name() -> Result<Option<String>, std::env::VarError> {
  let variable = if cfg!(windows) { "USERNAME" } else { "USER" };
  local_user_name(std::env::var(variable))
}

/// Opens the window and registers the quit paths. Returns the workspace.
#[cfg(test)]
pub(crate) fn start(cx: &mut App, client: Client, embedded: Embedded) -> eyre::Result<Entity<Workspace>> {
  start_with_config(cx, client, embedded, None)
}

fn start_with_config(cx: &mut App, client: Client, embedded: Embedded, config: Option<Configuration>) -> eyre::Result<Entity<Workspace>> {
  let user_name = read_local_user_name()?;
  let preferences_file = config
    .as_ref()
    .map(|config| ProjectPreferencesFile::new(config.project_preferences_path()));
  let preferences = preferences_file
    .as_ref()
    .map(ProjectPreferencesFile::load)
    .transpose()?
    .unwrap_or_default();
  gpui_kit::init(cx);
  alvin_conversation_ui::init(cx);
  cx.bind_keys([KeyBinding::new(QUIT_KEY, Quit, None)]);

  let quitting = Rc::new(Cell::new(false));
  let app_view: Rc<RefCell<Option<Entity<AppView>>>> = Rc::new(RefCell::new(None));
  let on_quit = {
    let embedded = embedded.clone();
    let quitting = Rc::clone(&quitting);
    let app_view = Rc::clone(&app_view);
    move |cx: &mut App| quit(&embedded, &quitting, &app_view, cx)
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
    let app_view = Rc::clone(&app_view);
    move |cx| {
      let view = app_view.borrow_mut().take();
      let cleanup = view.as_ref().map(|view| view.update(cx, |view, cx| view.shutdown(cx)));
      let embedded = embedded.clone();
      async move {
        if let Some(cleanup) = cleanup {
          cleanup.await;
        }
        drop(view);
        embedded.shutdown().await;
      }
    }
  })
  .detach();

  let workspace = Rc::new(RefCell::new(None));
  let saved_workspace = Rc::clone(&workspace);
  let (_window, root) = gpui_kit::open_window(
    WindowOptions {
      window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1200.), px(800.))))),
      ..if cfg!(target_os = "macos") {
        TitleBar::window_options()
      } else {
        WindowOptions::default()
      }
    },
    cx,
    |window, cx| {
      let model = cx.new(|cx| Model::new(client.clone(), cx));
      model.update(cx, |model, cx| model.refresh_projects(cx)).detach();
      let view = cx.new(|cx| Workspace::new(model, client, window, cx));
      if let Some(name) = user_name {
        view.update(cx, |workspace, cx| workspace.set_user_name(name, cx));
      }
      *saved_workspace.borrow_mut() = Some(view.clone());
      cx.new(|cx| AppView::new(view, embedded, config, preferences_file, preferences, window, cx))
    },
  )
  .map_err(|error| eyre::eyre!("opening the main window: {error}"))?;
  *app_view.borrow_mut() = Some(root);
  let view = workspace.borrow_mut().take().ok_or_else(|| eyre::eyre!("opening the workspace"))?;
  Ok(view)
}

/// Shuts retained terminal views and the embedded gateway down in full, then quits.
fn quit(embedded: &Embedded, quitting: &Rc<Cell<bool>>, app_view: &Rc<RefCell<Option<Entity<AppView>>>>, cx: &mut App) {
  if quitting.replace(true) {
    return;
  }
  let embedded = embedded.clone();
  let view = app_view.borrow_mut().take();
  let cleanup = view.as_ref().map(|view| view.update(cx, |view, cx| view.shutdown(cx)));
  cx.spawn(async move |cx| {
    if let Some(cleanup) = cleanup {
      cleanup.await;
    }
    drop(view);
    embedded.shutdown().await;
    cx.update(|cx| cx.quit());
  })
  .detach();
}

/// Runs the app until the user quits: builds the runtime, connects, and runs GPUI on this thread. Flushing telemetry is the
/// caller's, after this returns.
pub(crate) fn run(settings: Configuration) -> eyre::Result<()> {
  let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
  // GPUI runs on this thread; the guard gives its futures, and the client's, the runtime's timers and tasks.
  let _entered = runtime.enter();
  let (client, embedded) = runtime.block_on(connect(&settings.gateway))?;
  let failure = Rc::new(RefCell::new(None));
  let reported = Rc::clone(&failure);
  gpui_kit::application().with_assets(AppAssets).run(move |cx| {
    if let Err(error) = start_with_config(cx, client, embedded, Some(settings)) {
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

  use super::{AppAssets, Embedded, Quit, local_user_name, start};
  use crate::test_support::{Child, Fixture, sleeper};

  #[test]
  fn application_assets_include_sidebar_and_default_icons() {
    use gpui_kit::AssetSource as _;

    for path in ["icons/git-fork.svg", "icons/smartphone.svg", "icons/settings.svg"] {
      assert!(AppAssets.load(path).unwrap().is_some(), "missing {path}");
    }
  }

  #[test]
  fn local_user_name_preserves_absence_and_decoding_errors() {
    use std::env::VarError;
    use std::ffi::OsString;

    assert_eq!(local_user_name(Ok("alice".to_owned())).unwrap(), Some("alice".to_owned()));
    assert_eq!(local_user_name(Ok("  ".to_owned())).unwrap(), None);
    assert_eq!(local_user_name(Err(VarError::NotPresent)).unwrap(), None);
    let invalid = OsString::from("invalid");
    assert_eq!(
      local_user_name(Err(VarError::NotUnicode(invalid.clone()))).unwrap_err(),
      VarError::NotUnicode(invalid)
    );
  }

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

    let workspace = cx.update(|cx| start(cx, client, Embedded::new(Some(gateway.clone())))).unwrap();
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
