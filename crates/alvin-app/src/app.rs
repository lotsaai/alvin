//! Desktop connection, window, and shutdown lifecycle.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use alvin_client::{Client, Endpoint};
use alvin_conversation_ui::{Model, Workspace};
use gpui_kit::component::TitleBar;
use gpui_kit::{
  App, AppContext as _, AssetSource, Bounds, Entity, KeyBinding, SharedString, WindowBounds, WindowOptions, actions, px, size,
};

use crate::config::Configuration;
use crate::project_preferences::ProjectPreferencesFile;
use crate::settings::AppView;

gpui_kit::assets::icon_assets!(ExtraIcons, [GitFork, MicOff, Smartphone]);

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
    /// Closes the desktop client and quits.
    Quit
  ]
);

/// The platform's command key plus `q`.
#[cfg(target_os = "macos")]
const QUIT_KEY: &str = "cmd-q";
#[cfg(not(target_os = "macos"))]
const QUIT_KEY: &str = "ctrl-q";

/// Connects to the running gateway.
pub(crate) async fn connect(endpoint: &url::Url) -> eyre::Result<Client> {
  Ok(Client::connect(Endpoint::WebSocket(endpoint.clone())).await?)
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
pub(crate) fn start(cx: &mut App, client: Client) -> eyre::Result<Entity<Workspace>> {
  start_with_config(cx, client, None)
}

fn start_with_config(cx: &mut App, client: Client, config: Option<Configuration>) -> eyre::Result<Entity<Workspace>> {
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
  let speech_cache = dirs::cache_dir()
    .ok_or_else(|| eyre::eyre!("cannot determine the cache directory"))?
    .join("alvin")
    .join("speech");
  alvin_conversation_ui::init(speech_cache, cx);
  cx.bind_keys([KeyBinding::new(QUIT_KEY, Quit, None)]);

  let quitting = Rc::new(Cell::new(false));
  let app_view: Rc<RefCell<Option<Entity<AppView>>>> = Rc::new(RefCell::new(None));
  let on_quit = {
    let quitting = Rc::clone(&quitting);
    let app_view = Rc::clone(&app_view);
    move |cx: &mut App| quit(&quitting, &app_view, cx)
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
    let app_view = Rc::clone(&app_view);
    move |cx| {
      let view = app_view.borrow_mut().take();
      let cleanup = view.as_ref().map(|view| view.update(cx, |view, cx| view.shutdown(cx)));
      let speech = alvin_conversation_ui::shutdown(cx);
      async move {
        if let Some(cleanup) = cleanup {
          cleanup.await;
        }
        speech.await;
        drop(view);
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
      cx.new(|cx| AppView::new(view, config, preferences_file, preferences, window, cx))
    },
  )
  .map_err(|error| eyre::eyre!("opening the main window: {error}"))?;
  *app_view.borrow_mut() = Some(root);
  let view = workspace.borrow_mut().take().ok_or_else(|| eyre::eyre!("opening the workspace"))?;
  Ok(view)
}

/// Stops retained work and quits after terminal cleanup.
fn quit(quitting: &Rc<Cell<bool>>, app_view: &Rc<RefCell<Option<Entity<AppView>>>>, cx: &mut App) {
  if quitting.replace(true) {
    return;
  }
  let view = app_view.borrow_mut().take();
  let cleanup = view.as_ref().map(|view| view.update(cx, |view, cx| view.shutdown(cx)));
  alvin_conversation_ui::shutdown(cx).detach();
  cx.spawn(async move |cx| {
    if let Some(cleanup) = cleanup {
      cleanup.await;
    }
    drop(view);
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
  let client = runtime.block_on(connect(&settings.config.endpoint))?;
  let failure = Rc::new(RefCell::new(None));
  let reported = Rc::clone(&failure);
  gpui_kit::application().with_assets(AppAssets).run(move |cx| {
    if let Err(error) = start_with_config(cx, client, Some(settings)) {
      *reported.borrow_mut() = Some(error);
      cx.quit();
    }
  });
  failure.take().map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{BashRequest, LiveApiClient};
  use alvin_protocol::{ModelApiClient as _, OpenRequest, OpenTarget};
  use gpui_kit::AppContext as _;
  use gpui_kit::TestAppContext;
  use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};

  use super::{AppAssets, Quit, connect, local_user_name, start};
  use crate::test_support::ServerFixture;
  use test_context::test_context;

  #[test]
  fn application_assets_include_sidebar_and_default_icons() {
    use gpui_kit::AssetSource as _;

    for path in [
      "icons/git-fork.svg",
      "icons/mic.svg",
      "icons/mic-off.svg",
      "icons/smartphone.svg",
      "icons/settings.svg",
    ] {
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

  #[test_context(ServerFixture)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_window_quit_preserves_live_session(fixture: &mut ServerFixture, cx: &mut TestAppContext) {
    // `#[tokio::test]` polls this body, so GPUI's scheduler would never step the tasks it awaits. GPUI's executor drives the
    // body instead, while tokio's runtime context stays entered for the client and gateway tasks.
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let client = connect(&fixture.endpoint).await.unwrap();
      let probe = connect(&fixture.endpoint).await.unwrap();
      let workspace = cx.update(|cx| start(cx, client)).unwrap();
      let model = cx.read(|cx| workspace.read(cx).model().clone());
      cx.update(|cx| model.update(cx, |model, cx| model.refresh_projects(cx))).await;

      let (project, checkout) = cx.read(|cx| {
        let project = model.read(cx).conversation().workspace().projects()[0].clone();
        (project.id, project.checkouts[0].id.clone())
      });
      cx.update(|cx| model.update(cx, |model, cx| model.new_session(project, checkout, cx)))
        .await;
      let live = cx.read(|cx| workspace.read(cx).active().unwrap());
      drop(model);
      drop(workspace);

      cx.update(|cx| cx.dispatch_action(&Quit));
      // The headless platform does not quit; drain client cleanup before probing the server.
      cx.run_until_parked();
      assert!(
        LiveApiClient::list(&probe)
          .await
          .unwrap()
          .iter()
          .any(|session| session.live == live)
      );
      let reply = LiveApiClient::bash(
        &probe,
        BashRequest {
          live,
          command: "printf alvin-session-survived".to_owned(),
        },
      )
      .await
      .unwrap();
      assert_eq!(reply.exit_code, Some(0));
      assert_eq!(reply.output, "alvin-session-survived");
    })
  }

  #[test_context(ServerFixture)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn composer_editing_keeps_the_draft_in_the_live_window(fixture: &mut ServerFixture, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    // GPUI drives these futures, so Tokio's per-poll budget must not span the whole external executor run.
    tokio::task::unconstrained(async {
      executor.block_test(async {
        let client = connect(&fixture.endpoint).await.unwrap();
        let probe = connect(&fixture.endpoint).await.unwrap();
        let workspace = cx.update(|cx| start(cx, client)).unwrap();
        cx.update(|cx| cx.set_reduce_motion(true));
        let model = cx.read(|cx| workspace.read(cx).model().clone());
        let phase = std::rc::Rc::new(std::cell::Cell::new("startup"));
        let _errors = cx.update(|cx| {
          let phase = phase.clone();
          cx.subscribe(&model, move |_, event: &alvin_conversation_ui::ModelEvent, _| {
            if let alvin_conversation_ui::ModelEvent::Failed { code, message } = event {
              panic!("composer {} failed ({code:?}): {message}", phase.get());
            }
          })
        });
        let (notifications, mut notified) = tokio::sync::mpsc::unbounded_channel();
        let _observer = cx.update(|cx| {
          cx.observe(&model, move |_, _| {
            notifications.send(()).unwrap();
          })
        });
        cx.update(|cx| model.update(cx, |model, cx| model.refresh_projects(cx))).await;
        let (project, checkout) = cx.read(|cx| {
          let project = model.read(cx).conversation().workspace().projects()[0].clone();
          (project.id, project.checkouts[0].id.clone())
        });
        cx.update(|cx| model.update(cx, |model, cx| model.new_session(project.clone(), checkout.clone(), cx)))
          .await;
        let live = cx.read(|cx| workspace.read(cx).active().unwrap());
        model
          .condition::<alvin_conversation_ui::ModelEvent>(cx, move |model, _| {
            model.conversation().live(live).is_some_and(|view| view.model.is_some())
          })
          .await;
        let handle = cx.read(|cx| cx.windows()[0]);
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          let point = window.find_by_label("Message").bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
          window.input("abc", cx);
          window.press("left", cx);
          window.press("backspace", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("ac"));
          window.press("ctrl-c", cx);
          window.input("hello world", cx);
          window.press("ctrl-w", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("hello "));
          window.input("world", cx);
          window.press("ctrl-a", cx);
          window.press("ctrl-d", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          assert_eq!(window.find_by_label("Message").value(), Some("ello world"));
          window.press("ctrl-k", cx);
          assert_eq!(window.find_by_label("Message").value(), Some(""));
          window.input("draft", cx);
          window.press("alt-enter", cx);
          window.input("line two", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("draft\nline two"));
          window.press("ctrl-c", cx);
          assert_eq!(window.find_by_label("Message").value(), Some(""));
        })
        .unwrap();
        let models = probe.models(live).await.unwrap();
        phase.set("fresh history prompt");
        cx.update(|cx| model.update(cx, |model, cx| model.send(live, "Reply with the single word ok.", false, cx)))
          .await;
        model
          .condition::<alvin_conversation_ui::ModelEvent>(cx, move |model, _| {
            model.conversation().live(live).is_some_and(|view| {
              view.last_prompt.is_some()
                && view
                  .transcript
                  .iter()
                  .any(|message| matches!(message, alvin_protocol::AgentMessage::User(_)))
            })
          })
          .await;
        assert_eq!(
          cx.read(|cx| model.read(cx).conversation().live(live).unwrap().last_prompt),
          Some(alvin_protocol::PromptStatus::Completed)
        );
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          let point = window.find_by_label("Message").bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
          window.input("recover this draft", cx);
          window.press("secondary-a", cx);
          window.press("backspace", cx);
          window.press("up", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("Reply with the single word ok."));
          window.press("down", cx);
          assert_eq!(window.find_by_label("Message").value(), Some(""));
          window.press("secondary-z", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("Reply with the single word ok."));
          window.press("secondary-z", cx);
          assert_eq!(window.find_by_label("Message").value(), Some(""));
          window.press("secondary-z", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("recover this draft"));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| window.press("ctrl-c", cx)).unwrap();
        let initial = LiveApiClient::state(&probe, live).await.unwrap();
        let original = initial.model.as_ref().unwrap();
        let mut target = None;
        for candidate in models
          .iter()
          .filter(|candidate| candidate.provider == original.provider && candidate.id != original.id && candidate.reasoning)
        {
          let discovery = LiveApiClient::open(
            &probe,
            OpenRequest {
              target: OpenTarget::New {
                project: project.clone(),
                checkout: checkout.clone(),
              },
              model: Some(format!("{}/{}", candidate.provider, candidate.id)),
            },
          )
          .await
          .unwrap()
          .live;
          let supports_cycle = probe.thinking_levels(discovery).await.unwrap().len() > 1;
          LiveApiClient::close(&probe, discovery).await.unwrap();
          if supports_cycle {
            target = Some(candidate.clone());
            break;
          }
        }
        let target = target.expect("catalog contains another model with multiple thinking levels");
        let target_label = format!("{} · {}/{}", target.name, target.provider, target.id);
        let first = models.iter().find(|candidate| candidate.provider == target.provider).unwrap();
        let current_suffix = if first.id == original.id { " · Current" } else { "" };
        let provider_label = format!("{} · {}/{}{}", first.name, first.provider, first.id, current_suffix);
        cx.update_window(handle, |_, window, cx| {
          window.input("unsent draft", cx);
          window.click("model-picker", cx);
          window.input(&target.id, cx);
        })
        .unwrap();
        loop {
          let found = cx
            .update_window(handle, |_, window, cx| {
              window.render_frame(cx);
              window.try_find_by_label(&target_label).is_some()
            })
            .unwrap();
          if found {
            break;
          }
          notified.recv().await.expect("model observer remains registered");
        }
        cx.update_window(handle, |_, window, cx| {
          window.press("secondary-a", cx);
          window.input(&target.provider, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert!(window.try_find_by_label(&provider_label).is_some());
          window.press("secondary-a", cx);
          window.input(&target.name, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert!(window.try_find_by_label(&target_label).is_some());
          window.press("secondary-a", cx);
          window.input("no-model-matches-this-query", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          window.render_frame(cx);
          assert!(window.try_find_by_label(&target_label).is_none());
          window.press("secondary-a", cx);
          window.input(&target.id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          let point = window.find_by_label(&target_label).bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
        })
        .unwrap();
        assert_eq!(LiveApiClient::state(&probe, live).await.unwrap().model, initial.model);
        cx.update_window(handle, |_, window, cx| {
          window.press("escape", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("unsent draft"));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
          window.click("model-picker", cx);
          window.input(&target.id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          assert!(window.try_find_by_label(&target_label).is_some());
          let point = window.find_by_label(&target_label).bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
          window.click("model-picker-close", cx);
          assert_eq!(window.find_by_label("Message").value(), Some("unsent draft"));
        })
        .unwrap();
        cx.update_window(handle, |_, window, cx| {
          let point = window.find_by_label("Message").bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
          window.press("alt-p", cx);
          window.press("escape", cx);
          assert_eq!(window.find_by_label("Message").focused(), Some(true));
          assert_eq!(window.find_by_label("Message").value(), Some("unsent draft"));
        })
        .unwrap();
        assert_eq!(LiveApiClient::state(&probe, live).await.unwrap().model, initial.model);
        let expected_provider = target.provider.clone();
        let expected_id = target.id.clone();
        cx.update_window(handle, |_, window, cx| {
          window.click("model-picker", cx);
          window.input(&target.id, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
          let point = window.find_by_label(&target_label).bounds().center();
          window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
          window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
          window.click("model-picker-use", cx);
        })
        .unwrap();
        let switched = model.condition::<alvin_conversation_ui::ModelEvent>(cx, move |model, _| {
          model
            .conversation()
            .live(live)
            .and_then(|view| view.model.as_ref())
            .is_some_and(|current| current.provider == expected_provider && current.id == expected_id)
        });
        switched.await;
        let state = LiveApiClient::state(&probe, live).await.unwrap();
        assert_eq!(
          state.model.as_ref().map(|model| (&model.provider, &model.id)),
          Some((&target.provider, &target.id))
        );
        let levels = probe.thinking_levels(live).await.unwrap();
        phase.set("thinking cycle");
        assert!(levels.len() > 1, "selected reasoning model must offer multiple thinking levels");
        let initial_effort = *levels.last().unwrap();
        cx.update(|cx| model.update(cx, |model, cx| model.set_thinking(live, initial_effort, cx)))
          .await;
        model
          .condition::<alvin_conversation_ui::ModelEvent>(cx, move |model, _| {
            model.conversation().live(live).is_some_and(|view| {
              view.thinking == Some(initial_effort) && view.thinking_selector != Some(alvin_protocol::ConfiguredThinkingLevel::Auto)
            })
          })
          .await;
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..levels.len() + 2 {
          let before = cx.read(|cx| {
            let view = model.read(cx).conversation().live(live).unwrap();
            (view.thinking, view.thinking_selector)
          });
          cx.update_window(handle, |_, window, cx| {
            let point = window.find_by_label("Message").bounds().center();
            window.pointer_down(point, gpui_kit::MouseButton::Left, cx);
            window.pointer_up(point, gpui_kit::MouseButton::Left, cx);
            window.press("shift-tab", cx);
          })
          .unwrap();
          model
            .condition::<alvin_conversation_ui::ModelEvent>(cx, move |model, _| {
              model
                .conversation()
                .live(live)
                .is_some_and(|view| (view.thinking, view.thinking_selector) != before)
            })
            .await;
          let selector = cx.read(|cx| {
            let view = model.read(cx).conversation().live(live).unwrap();
            view
              .thinking_selector
              .map(|selector| selector.as_str())
              .or_else(|| view.thinking.map(|level| level.as_str()))
              .unwrap()
          });
          seen.insert(selector);
        }
        assert!(seen.contains("off"));
        for level in levels {
          assert!(seen.contains(level.as_str()), "cycle omitted {level:?}");
        }
        phase.set("second session");
        let other = LiveApiClient::open(
          &probe,
          OpenRequest {
            target: OpenTarget::New { project, checkout },
            model: None,
          },
        )
        .await
        .unwrap()
        .live;
        assert_ne!(other, live);
        assert_eq!(LiveApiClient::state(&probe, other).await.unwrap().model, initial.model);
        LiveApiClient::close(&probe, other).await.unwrap();
        let after = LiveApiClient::state(&probe, live).await.unwrap();
        assert_eq!(after.message_count, initial.message_count);
        assert_eq!(after.queued_message_count, initial.queued_message_count);
      });
    })
    .await;
  }
}
