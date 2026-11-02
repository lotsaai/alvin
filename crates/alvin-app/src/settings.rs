//! Settings editing and the window's active gateway.

use std::path::PathBuf;

use alvin_conversation_ui::{Model, Workspace};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, Theme, ThemeMode, h_flex, v_flex};
use gpui_kit::{
  App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
  StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
};

use crate::app::{Embedded, connect};
use crate::config::{Appearance, Configuration, Settings};

pub(crate) struct AppView {
  workspace: Entity<Workspace>,
  embedded: Embedded,
  config: Option<Configuration>,
  connected_gateway: Option<Settings>,
  endpoint: Entity<InputState>,
  omp: Entity<InputState>,
  model: Entity<InputState>,
  sessions: Entity<InputState>,
  appearance: Appearance,
  open: bool,
  connecting: bool,
  error: Option<String>,
  _appearance_subscription: Subscription,
}

impl AppView {
  pub(crate) fn new(
    workspace: Entity<Workspace>,
    embedded: Embedded,
    config: Option<Configuration>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let input = |value: String, window: &mut Window, cx: &mut Context<Self>| {
      cx.new(|cx| {
        let mut input = InputState::new(window, cx);
        input.set_value(value, window, cx);
        input
      })
    };
    let current = config.as_ref().map(|config| &config.config);
    let endpoint = input(
      current
        .and_then(|config| config.endpoint.as_ref())
        .map(ToString::to_string)
        .unwrap_or_default(),
      window,
      cx,
    );
    let omp = input(
      current
        .map(|config| config.gateway.omp.to_string_lossy().into_owned())
        .unwrap_or_default(),
      window,
      cx,
    );
    let model = input(
      current.and_then(|config| config.gateway.model.clone()).unwrap_or_default(),
      window,
      cx,
    );
    let sessions = input(
      current
        .and_then(|config| config.gateway.sessions_dir.as_ref())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default(),
      window,
      cx,
    );
    let appearance = match current.map(|config| config.appearance.as_str()) {
      Some("light") => Appearance::Light,
      Some("dark") => Appearance::Dark,
      _ => Appearance::System,
    };
    apply_appearance(appearance, window, cx);
    let subscription = cx.observe_window_appearance(window, |this, window, cx| {
      if this.appearance == Appearance::System {
        apply_appearance(this.appearance, window, cx);
      }
    });
    let connected_gateway = config.as_ref().map(|config| config.gateway.clone());
    Self {
      workspace,
      embedded,
      config,
      connected_gateway,
      endpoint,
      omp,
      model,
      sessions,
      appearance,
      open: false,
      connecting: false,
      error: None,
      _appearance_subscription: subscription,
    }
  }

  fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    let endpoint = self.endpoint.read(cx).value();
    next.endpoint = if endpoint.trim().is_empty() {
      None
    } else {
      match endpoint.trim().parse() {
        Ok(url) => Some(url),
        Err(error) => {
          self.error = Some(format!("Invalid gateway URL: {error}"));
          cx.notify();
          return;
        }
      }
    };
    let omp = self.omp.read(cx).value();
    if next.gateway.omp.to_string_lossy() != omp.as_ref() {
      next.gateway.omp = PathBuf::from(omp.as_ref());
    }
    let model = self.model.read(cx).value();
    next.gateway.model = (!model.is_empty()).then(|| model.to_string());
    let sessions = self.sessions.read(cx).value();
    if next
      .gateway
      .sessions_dir
      .as_ref()
      .map(|path| path.to_string_lossy().into_owned())
      .unwrap_or_default()
      != sessions.as_ref()
    {
      next.gateway.sessions_dir = (!sessions.is_empty()).then(|| PathBuf::from(sessions.as_ref()));
    }
    next.appearance = self.appearance.as_str().to_owned();
    let requested = next.clone();
    if let Err(error) = configuration.save(next) {
      self.error = Some(format!("Couldn’t save settings: {error}"));
      cx.notify();
      return;
    }
    self.appearance = match Appearance::parse(&configuration.config.appearance) {
      Ok(appearance) => appearance,
      Err(error) => {
        self.error = Some(error.to_string());
        cx.notify();
        return;
      }
    };
    apply_appearance(self.appearance, window, cx);
    let effective = &configuration.config;
    self.error = (requested.endpoint != effective.endpoint
      || requested.gateway.omp != effective.gateway.omp
      || requested.gateway.model != effective.gateway.model
      || requested.gateway.sessions_dir != effective.gateway.sessions_dir)
      .then(|| "Saved, but command-line, environment, or higher-priority config overrides the requested value.".to_owned());
    let gateway = configuration.gateway.clone();
    if self.connected_gateway.as_ref() != Some(&gateway) {
      self.connecting = true;
      cx.spawn_in(window, async move |this, cx| {
        let result = connect(&gateway).await;
        let old = this.update_in(cx, |this, window, cx| {
          this.connecting = false;
          match result {
            Ok((client, embedded)) => {
              let old = this.embedded.replace(&embedded);
              let model = cx.new(|cx| Model::new(client, cx));
              model.update(cx, |model, cx| model.refresh_projects(cx)).detach();
              this.workspace = cx.new(|cx| Workspace::new(model, window, cx));
              this.connected_gateway = Some(gateway);
              cx.notify();
              Some(old)
            }
            Err(error) => {
              this.error = Some(format!("Couldn’t connect to gateway: {error}"));
              cx.notify();
              None
            }
          }
        });
        if let Ok(Some(old)) = old {
          old.shutdown().await;
        }
      })
      .detach();
    }
    cx.notify();
  }
}

fn apply_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
  match appearance {
    Appearance::System => Theme::sync_system_appearance(Some(window), cx),
    Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
    Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
  }
}

fn field(label: &'static str, input: &Entity<InputState>) -> impl IntoElement {
  v_flex().gap_1().child(label).child(Input::new(input).aria_label(label))
}

impl Render for AppView {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let header = h_flex().px_4().py_2().border_b_1().border_color(cx.theme().border).child(
      Button::new("open-settings")
        .ghost()
        .label(if self.open { "Back to sessions" } else { "Settings" })
        .on_click(cx.listener(|this, _, _, cx| {
          this.open = !this.open;
          cx.notify();
        })),
    );
    let content = if self.open {
      let selector = ButtonGroup::new("appearance")
        .outline()
        .child(
          Button::new("appearance-system")
            .label("System")
            .selected(self.appearance == Appearance::System),
        )
        .child(
          Button::new("appearance-light")
            .label("Light")
            .selected(self.appearance == Appearance::Light),
        )
        .child(
          Button::new("appearance-dark")
            .label("Dark")
            .selected(self.appearance == Appearance::Dark),
        )
        .on_click(cx.listener(|this, selected: &Vec<usize>, window, cx| {
          if let Some(appearance) = selected
            .first()
            .and_then(|index| [Appearance::System, Appearance::Light, Appearance::Dark].get(*index))
            .copied()
          {
            if let Some(configuration) = &mut this.config {
              let mut next = configuration.config.clone();
              next.appearance = appearance.as_str().to_owned();
              match configuration.save(next) {
                Ok(()) => match Appearance::parse(&configuration.config.appearance) {
                  Ok(effective) => {
                    this.appearance = effective;
                    apply_appearance(effective, window, cx);
                    this.error = (effective != appearance)
                      .then(|| "Saved, but appearance is overridden by command-line, environment, or higher-priority config.".to_owned());
                  }
                  Err(error) => this.error = Some(error.to_string()),
                },
                Err(error) => this.error = Some(format!("Couldn’t save appearance: {error}")),
              }
            }
            cx.notify();
          }
        }));
      div()
        .id("settings-scroll")
        .size_full()
        .overflow_y_scroll()
        .child(
          v_flex()
            .p_6()
            .gap_4()
            .w_full()
            .child(div().text_xl().child("Settings"))
            .child(v_flex().gap_1().child("Appearance").child(selector))
            .child(div().text_lg().child("Gateway"))
            .child(
              div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Leave the URL empty to use the embedded gateway. Remote gateways are configured where they run."),
            )
            .child(
              div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Changing an embedded gateway closes its live sessions."),
            )
            .child(field("Gateway URL", &self.endpoint))
            .child(field("omp executable", &self.omp))
            .child(field("Default model", &self.model))
            .child(field("Sessions directory", &self.sessions))
            .children(
              self
                .error
                .as_ref()
                .map(|error| div().text_color(cx.theme().danger).child(SharedString::from(error.clone()))),
            )
            .child(
              Button::new("save-settings")
                .primary()
                .label(if self.connecting { "Connecting…" } else { "Save" })
                .disabled(self.connecting)
                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            ),
        )
        .into_any_element()
    } else {
      self.workspace.clone().into_any_element()
    };
    v_flex()
      .size_full()
      .bg(cx.theme().background)
      .text_color(cx.theme().foreground)
      .child(header)
      .child(div().flex_1().min_h_0().child(content))
  }
}
