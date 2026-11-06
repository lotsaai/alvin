//! Settings editing and the window's active gateway.

use std::sync::Arc;

use alvin_conversation_ui::{ConversationPreferences, Model, ProjectPreferences, UserMessageColors, Workspace, WorkspaceEvent};
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState, ColorSelect};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::searchable_list::SearchableVec;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Icon, Selectable as _, Theme, ThemeMode, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
  StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, rgb,
};

use crate::app::connect;
use crate::config::{Appearance, ColorChoice, Configuration};
use crate::project_preferences::ProjectPreferencesFile;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsTopic {
  General,
  Appearance,
  Hosts,
  Terminal,
}

impl SettingsTopic {
  const ALL: [Self; 4] = [Self::General, Self::Appearance, Self::Hosts, Self::Terminal];

  fn label(self) -> &'static str {
    match self {
      Self::General => "General",
      Self::Appearance => "Appearance",
      Self::Hosts => "Hosts",
      Self::Terminal => "Terminal",
    }
  }

  fn id(self) -> &'static str {
    match self {
      Self::General => "settings-general",
      Self::Appearance => "settings-appearance",
      Self::Hosts => "settings-hosts",
      Self::Terminal => "settings-terminal",
    }
  }

  fn icon(self) -> gpui_kit::assets::IconName {
    match self {
      Self::General => gpui_kit::assets::IconName::SlidersHorizontal,
      Self::Appearance => gpui_kit::assets::IconName::Palette,
      Self::Hosts => gpui_kit::assets::IconName::Server,
      Self::Terminal => gpui_kit::assets::IconName::Terminal,
    }
  }
}

pub(crate) struct AppView {
  workspace: Entity<Workspace>,
  config: Option<Configuration>,
  preferences_file: Option<ProjectPreferencesFile>,
  preferences: ProjectPreferences,
  preferences_error: Option<String>,
  connected_gateway: Option<Url>,
  endpoint: Entity<InputState>,
  appearance: Appearance,
  light_theme: Entity<SelectState<SearchableVec<String>>>,
  dark_theme: Entity<SelectState<SearchableVec<String>>>,
  accent_color: Entity<ColorPickerState>,
  user_color: Entity<ColorPickerState>,
  terminal_family: Entity<SelectState<SearchableVec<String>>>,
  terminal_size: Entity<InputState>,
  terminal_error: Option<String>,
  _settings_subscriptions: Vec<Subscription>,
  open: bool,
  topic: SettingsTopic,
  connecting: bool,
  shutting_down: bool,
  error: Option<String>,
  _workspace_subscription: Subscription,
  _workspace_event_subscription: Subscription,
  _appearance_subscription: Subscription,
}

impl AppView {
  pub(crate) fn shutdown(&mut self, cx: &mut Context<Self>) -> gpui_kit::Task<()> {
    self.shutting_down = true;
    self.workspace.update(cx, |workspace, cx| workspace.shutdown(cx))
  }

  pub(crate) fn new(
    workspace: Entity<Workspace>,
    config: Option<Configuration>,
    preferences_file: Option<ProjectPreferencesFile>,
    preferences: ProjectPreferences,
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
    cx.set_global(current.map_or_else(ConversationPreferences::default, |config| {
      ConversationPreferences::new(config.expand_mutating_tools, config.expand_readonly_tools, config.expand_thinking)
    }));
    let endpoint = input(current.map(|config| config.endpoint.to_string()).unwrap_or_default(), window, cx);
    let appearance = match current.map(|config| config.appearance.as_str()) {
      Some("light") => Appearance::Light,
      Some("dark") => Appearance::Dark,
      _ => Appearance::System,
    };
    let names: Vec<String> = verdant_themes::THEMES.iter().map(|name| (*name).to_owned()).collect();
    let light = current
      .map(|config| config.light_theme.as_str())
      .unwrap_or("catppuccin::latte")
      .to_owned();
    let dark = current
      .map(|config| config.dark_theme.as_str())
      .unwrap_or("catppuccin::mocha")
      .to_owned();
    let light_theme = cx.new(|cx| {
      let mut state = SelectState::new(SearchableVec::new(names.clone()), None, window, cx).searchable(true);
      state.set_selected_value(&light, window, cx);
      state
    });
    let dark_theme = cx.new(|cx| {
      let mut state = SelectState::new(SearchableVec::new(names), None, window, cx).searchable(true);
      state.set_selected_value(&dark, window, cx);
      state
    });
    let accent = current.map(|config| config.accent_color).unwrap_or(ColorChoice::MODULE);
    let user = current.map(|config| config.user_message_color).unwrap_or(ColorChoice::OPERATOR);
    apply_appearance(appearance, &light, &dark, accent, user, window, cx);
    let accent_color = cx.new(|cx| ColorPickerState::new(window, cx).default_value(cx.theme().primary));
    let user_color = cx.new(|cx| ColorPickerState::new(window, cx).default_value(cx.global::<UserMessageColors>().background()));
    let terminal_family =
      cx.new(|cx| SelectState::new(SearchableVec::new(cx.text_system().all_font_names()), None, window, cx).searchable(true));
    let terminal_size = input(
      current
        .and_then(|config| config.terminal_font_size)
        .map(|size| size.to_string())
        .unwrap_or_default(),
      window,
      cx,
    );
    let mut settings_subscriptions = vec![
      cx.subscribe_in(
        &light_theme,
        window,
        |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
          if let SelectEvent::Confirm(Some(name)) = event {
            this.select_theme(true, name, window, cx);
          }
        },
      ),
      cx.subscribe_in(
        &dark_theme,
        window,
        |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
          if let SelectEvent::Confirm(Some(name)) = event {
            this.select_theme(false, name, window, cx);
          }
        },
      ),
    ];
    for (picker, user) in [(&accent_color, false), (&user_color, true)] {
      settings_subscriptions.push(cx.subscribe_in(picker, window, move |_, _, event: &ColorPickerEvent, window, cx| {
        if let ColorPickerEvent::Change(Some(color)) = event {
          let choice = ColorChoice::from_color(*color);
          cx.defer_in(window, move |this, window, cx| this.select_color(user, choice, window, cx));
        }
      }));
    }
    settings_subscriptions.push(cx.subscribe_in(
      &terminal_family,
      window,
      |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
        if let SelectEvent::Confirm(Some(name)) = event {
          this.save_terminal_family(Some(name), window, cx);
        }
      },
    ));
    settings_subscriptions.push(cx.subscribe_in(&terminal_size, window, |this, _, event: &InputEvent, window, cx| {
      if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
        this.save_terminal_size(window, cx);
      }
    }));
    settings_subscriptions.push(cx.subscribe_in(&endpoint, window, |this, _, event: &InputEvent, window, cx| {
      if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
        this.save_gateway_field(window, cx);
      }
    }));
    let subscription = cx.observe_window_appearance(window, |this, window, cx| {
      if this.appearance == Appearance::System {
        this.apply_appearance(window, cx);
        this.apply_terminal_settings(window, cx);
      }
    });
    workspace.update(cx, |workspace, cx| workspace.set_project_preferences(preferences.clone(), cx));
    let workspace_event_subscription = Self::subscribe_workspace_events(&workspace, cx);
    let workspace_subscription = cx.observe(&workspace, |_, _, cx| cx.notify());
    let connected_gateway = config.as_ref().map(|config| config.config.endpoint.clone());
    let mut view = Self {
      workspace,
      config,
      preferences_file,
      preferences,
      preferences_error: None,
      connected_gateway,
      endpoint,
      appearance,
      light_theme,
      dark_theme,
      accent_color,
      user_color,
      terminal_family,
      terminal_size,
      terminal_error: None,
      _settings_subscriptions: settings_subscriptions,
      open: false,
      topic: SettingsTopic::General,
      connecting: false,
      shutting_down: false,
      error: None,
      _workspace_event_subscription: workspace_event_subscription,
      _workspace_subscription: workspace_subscription,
      _appearance_subscription: subscription,
    };
    view.apply_terminal_settings(window, cx);
    view
  }

  fn apply_terminal_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let Some(configuration) = &self.config else { return };
    let dark = cx.theme().mode == ThemeMode::Dark;
    let (mut settings, mut warning) = crate::terminal_font::load(configuration.home(), dark);
    if let Some(size) = configuration.config.terminal_font_size {
      settings.font_size = Some(size);
    }
    if settings.font_size.is_some_and(|size| !size.is_finite() || size <= 0.0) {
      self.terminal_error = Some("Terminal font size must be a finite positive number.".to_owned());
      cx.notify();
      return;
    }
    let installed = cx.text_system().all_font_names();
    let previous = self.workspace.read(cx).terminal_settings();
    let missing = crate::terminal_font::resolve_families(
      &mut settings,
      previous,
      &installed,
      configuration.config.terminal_font_family.as_deref(),
    );
    for family in missing {
      let message = format!("Terminal font family {family:?} is not installed.");
      warning = Some(match warning.take() {
        Some(previous) => format!("{previous} {message}"),
        None => message,
      });
    }
    self.terminal_family.update(cx, |picker, cx| {
      if let Some(family) = &settings.font_family {
        picker.set_selected_value(family, window, cx);
      } else {
        picker.set_selected_index(None, window, cx);
      }
    });
    self.terminal_error = warning;
    self
      .workspace
      .update(cx, |workspace, cx| workspace.set_terminal_settings(settings, cx));
    cx.notify();
  }

  fn save_terminal_family(&mut self, family: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    next.terminal_font_family = family.map(str::to_owned);
    match configuration.save(next) {
      Ok(()) => cx.defer_in(window, |this, window, cx| this.apply_terminal_settings(window, cx)),
      Err(error) => self.terminal_error = Some(format!("Couldn’t save terminal font: {error}")),
    }
    cx.notify();
  }

  fn save_terminal_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let value = self.terminal_size.read(cx).value();
    let size = if value.trim().is_empty() {
      None
    } else {
      match value.trim().parse::<f32>() {
        Ok(size) if size.is_finite() && size > 0.0 => Some(size),
        _ => {
          self.terminal_error = Some("Terminal font size must be a finite positive number.".to_owned());
          cx.notify();
          return;
        }
      }
    };
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    next.terminal_font_size = size;
    match configuration.save(next) {
      Ok(()) => self.apply_terminal_settings(window, cx),
      Err(error) => self.terminal_error = Some(format!("Couldn’t save terminal font size: {error}")),
    }
    cx.notify();
  }

  fn save_conversation_preference(&mut self, field: usize, checked: bool, cx: &mut Context<Self>) {
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    match field {
      0 => next.expand_mutating_tools = checked,
      1 => next.expand_readonly_tools = checked,
      _ => next.expand_thinking = checked,
    }
    match configuration.save(next) {
      Ok(()) => {
        let config = &configuration.config;
        cx.set_global(ConversationPreferences::new(
          config.expand_mutating_tools,
          config.expand_readonly_tools,
          config.expand_thinking,
        ));
        let effective = match field {
          0 => config.expand_mutating_tools,
          1 => config.expand_readonly_tools,
          _ => config.expand_thinking,
        };
        self.error =
          (effective != checked).then(|| "Saved, but conversation preference is overridden by higher-priority configuration.".to_owned());
      }
      Err(error) => self.error = Some(format!("Couldn’t save conversation preference: {error}")),
    }
    cx.notify();
  }

  fn subscribe_workspace_events(workspace: &Entity<Workspace>, cx: &mut Context<Self>) -> Subscription {
    cx.subscribe(workspace, |this, workspace, event: &WorkspaceEvent, cx| {
      match event {
        WorkspaceEvent::ProjectPreferencesChanged(preferences) => {
          let result = this.preferences_file.as_ref().map(|file| file.save(preferences)).transpose();
          match result {
            Ok(_) => {
              this.preferences = preferences.clone();
              this.preferences_error = None;
            }
            Err(error) => {
              workspace.update(cx, |workspace, cx| workspace.set_project_preferences(this.preferences.clone(), cx));
              this.preferences_error = Some(format!("Couldn’t save project preferences: {error}"));
            }
          }
        }
        WorkspaceEvent::SettingsRequested => {
          workspace.update(cx, |workspace, cx| workspace.cancel_dictation(cx));
          this.open = true;
        }
      }
      cx.notify();
    })
  }

  fn apply_appearance(&self, window: &mut Window, cx: &mut Context<Self>) {
    let light = self
      .light_theme
      .read(cx)
      .selected_value()
      .cloned()
      .unwrap_or_else(|| "catppuccin::latte".into());
    let dark = self
      .dark_theme
      .read(cx)
      .selected_value()
      .cloned()
      .unwrap_or_else(|| "catppuccin::mocha".into());
    let accent = self
      .config
      .as_ref()
      .map(|config| config.config.accent_color)
      .unwrap_or(ColorChoice::MODULE);
    let user = self
      .config
      .as_ref()
      .map(|config| config.config.user_message_color)
      .unwrap_or(ColorChoice::OPERATOR);
    apply_appearance(self.appearance, &light, &dark, accent, user, window, cx);
    let accent = cx.theme().primary;
    let user = cx.global::<UserMessageColors>().background();
    self.accent_color.update(cx, |picker, cx| picker.set_value(accent, window, cx));
    self.user_color.update(cx, |picker, cx| picker.set_value(user, window, cx));
  }

  fn select_color(&mut self, user: bool, choice: ColorChoice, window: &mut Window, cx: &mut Context<Self>) {
    let Some(configuration) = &mut self.config else {
      self.apply_appearance(window, cx);
      return;
    };
    let mut next = configuration.config.clone();
    if user {
      next.user_message_color = choice;
    } else {
      next.accent_color = choice;
    }
    match configuration.save(next) {
      Ok(()) => {
        let effective = if user {
          configuration.config.user_message_color
        } else {
          configuration.config.accent_color
        };
        self.error = (effective != choice)
          .then(|| "Saved, but color is overridden by command-line, environment, or higher-priority config.".to_owned());
      }
      Err(error) => self.error = Some(format!("Couldn’t save color: {error}")),
    }
    self.apply_appearance(window, cx);
    cx.notify();
  }

  fn select_theme(&mut self, light: bool, name: &str, window: &mut Window, cx: &mut Context<Self>) {
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    if light {
      next.light_theme = name.to_owned();
    } else {
      next.dark_theme = name.to_owned();
    }
    match configuration.save(next) {
      Ok(()) => {
        let effective = if light {
          &configuration.config.light_theme
        } else {
          &configuration.config.dark_theme
        };
        self.error =
          (effective != name).then(|| "Saved, but theme is overridden by command-line, environment, or higher-priority config.".to_owned());
      }
      Err(error) => self.error = Some(format!("Couldn’t save theme: {error}")),
    }
    let effective = if light {
      &configuration.config.light_theme
    } else {
      &configuration.config.dark_theme
    };
    let picker = if light { &self.light_theme } else { &self.dark_theme };
    picker.update(cx, |picker, cx| picker.set_selected_value(effective, window, cx));
    self.apply_appearance(window, cx);
    self.apply_terminal_settings(window, cx);
    cx.notify();
  }

  fn save_gateway_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    if self.connecting {
      return;
    }
    let Some(configuration) = &mut self.config else { return };
    let mut next = configuration.config.clone();
    let endpoint = self.endpoint.read(cx).value();
    next.endpoint = match endpoint.trim().parse() {
      Ok(url) => url,
      Err(error) => {
        self.error = Some(format!("Invalid gateway URL: {error}"));
        cx.notify();
        return;
      }
    };
    let requested = next.clone();
    if let Err(error) = configuration.save(next) {
      self.error = Some(format!("Couldn’t save settings: {error}"));
      cx.notify();
      return;
    }
    let effective = &configuration.config;
    self.error = (requested.endpoint != effective.endpoint)
      .then(|| "Saved, but command-line, environment, or higher-priority config overrides the requested value.".to_owned());
    let gateway = configuration.config.endpoint.clone();
    if self.connected_gateway.as_ref() != Some(&gateway) {
      self.connecting = true;
      cx.spawn_in(window, async move |this, cx| {
        let result = connect(&gateway).await;
        if result.is_ok() {
          let cleanup = this.update(cx, |this, cx| {
            (!this.shutting_down).then(|| this.workspace.update(cx, |workspace, cx| workspace.shutdown(cx)))
          });
          match cleanup {
            Ok(Some(cleanup)) => cleanup.await,
            Ok(None) | Err(_) => return,
          }
          if !this.update(cx, |this, _| !this.shutting_down).unwrap_or(false) {
            return;
          }
        }
        drop(this.update_in(cx, |this, window, cx| {
          this.connecting = false;
          match result {
            Ok(client) => {
              let model = cx.new(|cx| Model::new(client.clone(), cx));
              model.update(cx, |model, cx| model.refresh_projects(cx)).detach();
              let user_name = this.workspace.read(cx).user_name().to_owned();
              this.workspace = cx.new(|cx| Workspace::new(model, client, window, cx));
              this.workspace.update(cx, |workspace, cx| workspace.set_user_name(user_name, cx));
              this
                .workspace
                .update(cx, |workspace, cx| workspace.set_project_preferences(this.preferences.clone(), cx));
              this.apply_terminal_settings(window, cx);
              this._workspace_event_subscription = Self::subscribe_workspace_events(&this.workspace, cx);
              this._workspace_subscription = cx.observe(&this.workspace, |_, _, cx| cx.notify());
              this.connected_gateway = Some(gateway);
              cx.notify();
              Some(())
            }
            Err(error) => {
              this.error = Some(format!("Couldn’t connect to gateway: {error}"));
              cx.notify();
              None
            }
          }
        }));
      })
      .detach();
    }
    cx.notify();
  }
}
fn convert_color(red: u8, green: u8, blue: u8) -> gpui_kit::Hsla {
  gpui_kit::Hsla::from(rgb((u32::from(red) << 16) | (u32::from(green) << 8) | u32::from(blue)))
}

fn luminance(color: gpui_kit::Hsla) -> f32 {
  let color = gpui_kit::Rgba::from(color);
  let linear = |value: f32| {
    if value <= 0.04045 {
      value / 12.92
    } else {
      ((value + 0.055) / 1.055).powf(2.4)
    }
  };
  0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: gpui_kit::Hsla, b: gpui_kit::Hsla) -> f32 {
  let (a, b) = (luminance(a), luminance(b));
  (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn contrasting(color: gpui_kit::Hsla, background: gpui_kit::Hsla, foreground: gpui_kit::Hsla) -> gpui_kit::Hsla {
  let color = background.blend(color);
  let chosen = if contrast(color, foreground) > contrast(color, background) {
    foreground
  } else {
    background
  };
  if contrast(color, chosen) >= 4.5 {
    chosen
  } else if luminance(color) > 0.179 {
    gpui_kit::Hsla::from(rgb(0))
  } else {
    gpui_kit::Hsla::from(rgb(0xffffff))
  }
}

fn apply_appearance(
  appearance: Appearance,
  light: &str,
  dark: &str,
  accent: ColorChoice,
  user: ColorChoice,
  window: &mut Window,
  cx: &mut App,
) {
  match appearance {
    Appearance::System => Theme::sync_system_appearance(Some(window), cx),
    Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
    Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
  }
  let name = if cx.theme().mode == ThemeMode::Dark { dark } else { light };
  if let Some(theme) = verdant_themes::from_str(name) {
    let catppuccin = match name {
      "catppuccin::latte" => Some((0xe6e9ef, 0xccd0da, 0xbcc0cc, 0x5c5f77, 0xd20f39, 0x40a02b, 0xdf8e1d, 0x1e66f5)),
      "catppuccin::mocha" => Some((0x181825, 0x313244, 0x45475a, 0xbac2de, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa)),
      _ => None,
    };
    let convert = convert_color;
    let resolved = |key: &str| {
      theme.find_style(key).map(|style| {
        let color = style.color();
        convert(color.red, color.green, color.blue)
      })
    };
    let syntax: serde_json::Map<String, serde_json::Value> = [
      ("attribute", "attribute"),
      ("boolean", "boolean"),
      ("comment", "comment"),
      ("constant", "constant"),
      ("constructor", "constructor"),
      ("emphasis", "markup.italic"),
      ("emphasis.strong", "markup.strong"),
      ("function", "function"),
      ("keyword", "keyword"),
      ("label", "label"),
      ("link_text", "markup.link.label"),
      ("link_uri", "markup.link.url"),
      ("number", "number"),
      ("operator", "operator"),
      ("property", "property"),
      ("punctuation", "punctuation"),
      ("string", "string"),
      ("string.escape", "string.escape"),
      ("tag", "tag"),
      ("text.literal", "markup.raw"),
      ("title", "markup.heading"),
      ("type", "type"),
      ("variable", "variable"),
    ]
    .into_iter()
    .filter_map(|(target, source)| {
      theme.find_style(source).map(|style| {
        let color = style.color();
        let color = convert(color.red, color.green, color.blue);
        (
          target.to_owned(),
          serde_json::json!({
            "color": color,
            "font_style": if style.italic() { Some("italic") } else if style.underline() { Some("underline") } else { None },
            "font_weight": if style.bold() { Some(700) } else { None },
          }),
        )
      })
    })
    .collect();
    let syntax = serde_json::from_value(serde_json::Value::Object(syntax));
    Theme::update(cx, |current| {
      match syntax {
        Ok(syntax) => {
          let mut highlight = (*current.highlight_theme).clone();
          highlight.name = name.to_owned();
          highlight.style.syntax = syntax;
          highlight.style.editor_background = theme.bg().map(|color| convert(color.red, color.green, color.blue));
          highlight.style.editor_foreground = theme.fg().map(|color| convert(color.red, color.green, color.blue));
          current.highlight_theme = Arc::new(highlight);
        }
        Err(error) => tracing::error!(%error, theme = name, "Could not apply syntax theme"),
      }
      let colors = &mut current.colors;
      if let Some(color) = theme.bg() {
        let background = convert(color.red, color.green, color.blue);
        colors.background = background;
        let foreground = theme
          .fg()
          .map(|color| convert(color.red, color.green, color.blue))
          .unwrap_or(colors.foreground);
        let mut surface = background;
        surface.l = (surface.l + if foreground.l > background.l { -0.035 } else { 0.035 }).clamp(0.0, 1.0);
        colors.sidebar = surface;
        colors.popover = background.blend(foreground.alpha(0.08));
        colors.list = background;
        colors.title_bar = surface;
        colors.status_bar = surface;
        colors.tab_bar = surface;
        colors.tab = surface;
        colors.group_box = background;
        colors.table = background;
        colors.scrollbar = surface;
        colors.table_head = surface;
        colors.table_foot = surface;
        colors.tab_active = background;
        colors.accordion = surface;
      }
      if let Some(color) = theme.fg() {
        let foreground = convert(color.red, color.green, color.blue);
        colors.foreground = foreground;
        colors.sidebar_foreground = foreground;
        colors.popover_foreground = foreground;
        colors.tab_foreground = foreground;
        colors.group_box_foreground = foreground;
        colors.tab_active_foreground = foreground;
        colors.table_head_foreground = foreground;
        colors.table_foot_foreground = foreground;
        let background = colors.background;
        let subtle = background.blend(foreground.alpha(0.08));
        let hover = background.blend(foreground.alpha(0.14));
        let border = background.blend(foreground.alpha(0.25));
        colors.accent = hover;
        colors.accent_foreground = foreground;
        colors.border = border;
        colors.input = border;
        colors.sidebar_border = border;
        colors.title_bar_border = border;
        colors.status_bar_border = border;
        colors.list_active = hover;
        colors.table_row_border = border;
        colors.window_border = border;
        colors.list_active_border = border;
        colors.table_active_border = border;
        colors.table_active = hover;
        colors.table_hover = subtle;
        colors.table_even = background;
        colors.scrollbar_thumb = border;
        colors.scrollbar_thumb_hover = hover;
        colors.list_hover = subtle;
        colors.list_even = background;
        colors.list_head = subtle;
        colors.muted = subtle;
        colors.skeleton = subtle;
        colors.switch = border;
        colors.slider_bar = border;
        colors.description_list_label = subtle;
        colors.description_list_label_foreground = foreground;
        colors.chart_grid = border;
        colors.muted_foreground = background.blend(foreground.alpha(0.7));
        colors.secondary = subtle;
        colors.secondary_foreground = foreground;
        colors.secondary_hover = hover;
        colors.secondary_active = border;
        colors.button_secondary = subtle;
        colors.button_secondary_foreground = foreground;
        colors.button_secondary_hover = hover;
        colors.button_secondary_active = border;
        colors.button = subtle;
        colors.button_foreground = foreground;
        colors.button_hover = hover;
        colors.button_active = border;
        colors.selection = background.blend(colors.muted_foreground.alpha(0.25));
        colors.sidebar_accent = hover;
        colors.sidebar_accent_foreground = foreground;
        colors.tab_bar_segmented = subtle;
      }
      if let Some((mantle, surface, border, subtext, _, _, _, _)) = catppuccin {
        let mantle = gpui_kit::Hsla::from(rgb(mantle));
        let surface = gpui_kit::Hsla::from(rgb(surface));
        let border = gpui_kit::Hsla::from(rgb(border));
        colors.sidebar = mantle;
        colors.title_bar = mantle;
        colors.status_bar = mantle;
        colors.tab_bar = mantle;
        colors.tab = mantle;
        colors.scrollbar = mantle;
        colors.popover = colors.background;
        colors.accordion = surface;
        colors.table_head = surface;
        colors.table_foot = surface;
        colors.muted = surface;
        let subtext = gpui_kit::Hsla::from(rgb(subtext));
        colors.muted_foreground = if contrast(subtext, surface) >= 4.5 {
          subtext
        } else {
          colors.foreground
        };
        colors.secondary = surface;
        colors.button = surface;
        colors.button_secondary = surface;
        colors.skeleton = surface;
        colors.description_list_label = surface;
        colors.list_head = surface;
        colors.tab_bar_segmented = surface;
        colors.border = border;
        colors.input = border;
        colors.sidebar_border = border;
        colors.title_bar_border = border;
        colors.status_bar_border = border;
        colors.table_row_border = border;
        colors.window_border = border;
        colors.scrollbar_thumb = border;
        colors.switch = border;
        colors.slider_bar = border;
        colors.chart_grid = border;
      }
      if let Some(accent) = accent.color().or_else(|| accent.style_key().and_then(resolved)) {
        let background = colors.background;
        let foreground = colors.foreground;
        colors.primary = accent;
        colors.primary_foreground = contrasting(accent, background, foreground);
        colors.primary_hover = accent.blend(foreground.alpha(0.12));
        colors.primary_active = accent.blend(foreground.alpha(0.2));
        colors.button_primary = colors.primary;
        colors.button_primary_foreground = colors.primary_foreground;
        colors.button_primary_hover = colors.primary_hover;
        colors.button_primary_active = colors.primary_active;
        colors.sidebar_primary = accent;
        colors.sidebar_primary_foreground = colors.primary_foreground;
        colors.ring = accent;
        colors.caret = resolved("markup.link.url").unwrap_or(foreground);
        colors.link = resolved("function").unwrap_or(foreground);
        colors.link_hover = colors.link.blend(foreground.alpha(0.12));
        colors.link_active = colors.link.blend(foreground.alpha(0.2));
        colors.list_active = background.blend(accent.alpha(0.2));
        colors.table_active = colors.list_active;
        colors.sidebar_accent = colors.list_active;
        colors.list_active_border = accent;
        colors.table_active_border = accent;
        colors.drag_border = accent;
        colors.drop_target = colors.list_active;
        colors.progress_bar = accent;
        colors.slider_thumb = accent;
        colors.switch_thumb = accent;
      }
      if let Some(color) = catppuccin.map(|palette| gpui_kit::Hsla::from(rgb(palette.4))).or_else(|| {
        theme.get("diff.minus").map(|style| {
          let color = style.color();
          convert(color.red, color.green, color.blue)
        })
      }) {
        colors.danger = color;
        colors.danger_foreground = contrasting(colors.danger, colors.background, colors.foreground);
        colors.danger_hover = colors.danger.blend(colors.foreground.alpha(0.12));
        colors.danger_active = colors.danger.blend(colors.foreground.alpha(0.2));
        colors.button_danger = colors.danger;
        colors.button_danger_foreground = colors.danger_foreground;
        colors.button_danger_hover = colors.danger_hover;
        colors.button_danger_active = colors.danger_active;
      }
      if let Some(color) = catppuccin.map(|palette| gpui_kit::Hsla::from(rgb(palette.5))).or_else(|| {
        theme.get("diff.plus").map(|style| {
          let color = style.color();
          convert(color.red, color.green, color.blue)
        })
      }) {
        colors.success = color;
        colors.success_foreground = contrasting(colors.success, colors.background, colors.foreground);
        colors.success_hover = colors.success.blend(colors.foreground.alpha(0.12));
        colors.success_active = colors.success.blend(colors.foreground.alpha(0.2));
        colors.button_success = colors.success;
        colors.button_success_foreground = colors.success_foreground;
        colors.button_success_hover = colors.success_hover;
        colors.button_success_active = colors.success_active;
      }
      if let Some(color) = catppuccin.map(|palette| gpui_kit::Hsla::from(rgb(palette.6))).or_else(|| {
        theme.get("comment.warning").map(|style| {
          let color = style.bg().unwrap_or_else(|| style.color());
          convert(color.red, color.green, color.blue)
        })
      }) {
        colors.warning = color;
        colors.warning_foreground = contrasting(colors.warning, colors.background, colors.foreground);
        colors.warning_hover = colors.warning.blend(colors.foreground.alpha(0.12));
        colors.warning_active = colors.warning.blend(colors.foreground.alpha(0.2));
        colors.button_warning = colors.warning;
        colors.button_warning_foreground = colors.warning_foreground;
        colors.button_warning_hover = colors.warning_hover;
        colors.button_warning_active = colors.warning_active;
      }
      if let Some(color) = catppuccin.map(|palette| gpui_kit::Hsla::from(rgb(palette.7))).or_else(|| {
        theme.get("comment.note").map(|style| {
          let color = style.bg().unwrap_or_else(|| style.color());
          convert(color.red, color.green, color.blue)
        })
      }) {
        colors.info = color;
        colors.info_foreground = contrasting(colors.info, colors.background, colors.foreground);
        colors.info_hover = colors.info.blend(colors.foreground.alpha(0.12));
        colors.info_active = colors.info.blend(colors.foreground.alpha(0.2));
        colors.button_info = colors.info;
        colors.button_info_foreground = colors.info_foreground;
        colors.button_info_hover = colors.info_hover;
        colors.button_info_active = colors.info_active;
      }
    });
    let background = cx.theme().background;
    let foreground = cx.theme().foreground;
    let key = if user == ColorChoice::ACCENT { accent } else { user };
    let user_background = key.color().or_else(|| key.style_key().and_then(resolved)).unwrap_or(foreground);
    cx.set_global(UserMessageColors::new(
      user_background,
      contrasting(user_background, background, foreground),
    ));
  } else {
    let background = cx.theme().primary;
    let foreground = cx.theme().primary_foreground;
    cx.set_global(UserMessageColors::new(background, foreground));
  }
  alvin_conversation_ui::refresh_markdown_theme(cx);
  cx.refresh_windows();
}

fn field(label: &'static str, input: &Entity<InputState>, disabled: bool) -> impl IntoElement {
  v_flex()
    .gap_1()
    .child(label)
    .child(Input::new(input).id(label).aria_label(label).disabled(disabled))
}

impl Render for AppView {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    this.apply_appearance(window, cx);
                    this.apply_terminal_settings(window, cx);
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
      let form = match self.topic {
        SettingsTopic::General => {
          let preferences = *cx.global::<ConversationPreferences>();
          v_flex()
            .gap_4()
            .child("Configure omp and the default model on the gateway server.")
            .child("Conversation")
            .children(
              [
                (
                  "expand-mutating-tools",
                  "Expand potentially mutating tools",
                  preferences.expand_mutating_tools,
                ),
                ("expand-readonly-tools", "Expand readonly tools", preferences.expand_readonly_tools),
                ("expand-thinking", "Expand thinking", preferences.expand_thinking),
              ]
              .into_iter()
              .enumerate()
              .map(|(field, (id, label, checked))| {
                Switch::new(id)
                  .label(label)
                  .checked(checked)
                  .on_change(cx.listener(move |this, checked, _, cx| this.save_conversation_preference(field, *checked, cx)))
              }),
            )
            .into_any_element()
        }
        SettingsTopic::Appearance => v_flex()
          .gap_4()
          .child(v_flex().gap_1().child("Mode").child(selector))
          .child(
            v_flex()
              .gap_1()
              .child("Light theme")
              .child(Select::new(&self.light_theme).id("light-theme").accessibility_label("Light theme")),
          )
          .child(
            v_flex()
              .gap_1()
              .child("Dark theme")
              .child(Select::new(&self.dark_theme).id("dark-theme").accessibility_label("Dark theme")),
          )
          .child(
            v_flex().gap_1().child("Accent color").child(
              div()
                .id("accent-color")
                .test_support()
                .child(ColorSelect::new(&self.accent_color).accessibility_label("Accent color")),
            ),
          )
          .child(
            v_flex()
              .gap_1()
              .child("User message color")
              .child(
                div()
                  .id("user-message-color")
                  .test_support()
                  .child(ColorSelect::new(&self.user_color).accessibility_label("User message color")),
              )
              .child(
                Button::new("user-color-accent")
                  .label("Use accent color")
                  .selected(
                    self
                      .config
                      .as_ref()
                      .is_some_and(|config| config.config.user_message_color == ColorChoice::ACCENT),
                  )
                  .on_click(cx.listener(|this, _, window, cx| this.select_color(true, ColorChoice::ACCENT, window, cx))),
              ),
          )
          .into_any_element(),
        SettingsTopic::Hosts => v_flex()
          .gap_4()
          .child(
            div()
              .text_sm()
              .text_color(cx.theme().muted_foreground)
              .child("Configure omp on the gateway server. Press Enter to save the WebSocket URL."),
          )
          .child(field("Gateway URL", &self.endpoint, self.connecting))
          .into_any_element(),
        SettingsTopic::Terminal => v_flex()
          .gap_4()
          .child(
            v_flex().gap_1().child("Terminal font family").child(
              Select::new(&self.terminal_family)
                .id("terminal-font-family")
                .accessibility_label("Terminal font family"),
            ),
          )
          .when(
            self
              .config
              .as_ref()
              .is_some_and(|config| config.config.terminal_font_family.is_some()),
            |view| {
              view.child(
                Button::new("terminal-font-inherit")
                  .label("Use inherited font")
                  .on_click(cx.listener(|this, _, window, cx| this.save_terminal_family(None, window, cx))),
              )
            },
          )
          .child(field("Terminal font size (px)", &self.terminal_size, false))
          .child(
            div()
              .text_sm()
              .text_color(cx.theme().muted_foreground)
              .child("Press Enter to save font size. Leave it empty to use Ghostty or the default."),
          )
          .children(
            self
              .terminal_error
              .as_ref()
              .map(|error| div().text_color(cx.theme().danger).child(SharedString::from(error.clone()))),
          )
          .into_any_element(),
      };
      div()
        .flex()
        .items_stretch()
        .size_full()
        .bg(cx.theme().sidebar)
        .child(
          v_flex()
            .id("settings-sidebar")
            .test_support()
            .w_48()
            .flex_shrink_0()
            .when(cfg!(target_os = "macos"), |view| {
              view.child(gpui_kit::component::TitleBar::new().bg(cx.theme().sidebar).border_0())
            })
            .child(
              v_flex()
                .p_3()
                .gap_1()
                .child(
                  Button::new("back-to-sessions")
                    .ghost()
                    .accessibility_label("Back to sessions")
                    .child(
                      h_flex()
                        .id("settings-back-content")
                        .test_support()
                        .w_full()
                        .gap_2()
                        .child(Icon::new(gpui_kit::assets::IconName::ArrowLeft).size_4())
                        .child("Back to sessions"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                      this.open = false;
                      this.workspace.update(cx, |workspace, cx| workspace.focus(window, cx));
                      cx.notify();
                    })),
                )
                .child(div().h_2())
                .children(SettingsTopic::ALL.into_iter().map(|topic| {
                  let focus = window
                    .use_keyed_state((gpui_kit::ElementId::from(topic.id()), "focus"), cx, |_, cx| {
                      cx.focus_handle().tab_stop(true)
                    })
                    .read(cx)
                    .clone();
                  Button::new(topic.id())
                    .track_focus(&focus)
                    .ghost()
                    .accessibility_label(topic.label())
                    .child(
                      h_flex()
                        .id("settings-topic-content")
                        .test_support()
                        .w_full()
                        .gap_2()
                        .child(Icon::new(topic.icon()).size_4())
                        .child(topic.label()),
                    )
                    .selected(self.topic == topic)
                    .on_click(cx.listener(move |this, _, window, cx| {
                      focus.focus(window, cx);
                      this.topic = topic;
                      cx.notify();
                    }))
                })),
            ),
        )
        .child(
          div()
            .id("settings-scroll")
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .mt_1()
            .mr_1()
            .mb_1()
            .rounded_lg()
            .bg(cx.theme().background)
            .overflow_y_scroll()
            .child(
              div().flex().justify_center().w_full().child(
                v_flex()
                  .id("settings-content")
                  .test_support()
                  .p_6()
                  .gap_4()
                  .w_full()
                  .max_w(gpui_kit::rems(42.))
                  .child(div().text_xl().font_semibold().child(self.topic.label()))
                  .child(form)
                  .when(self.topic != SettingsTopic::Terminal, |view| {
                    view
                      .children(
                        self
                          .error
                          .as_ref()
                          .map(|error| div().text_color(cx.theme().danger).child(SharedString::from(error.clone()))),
                      )
                      .when(self.connecting, |view| view.child("Connecting…"))
                  }),
              ),
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
      .child(div().flex_1().min_h_0().child(content))
      .when(!self.open, |view| {
        view.children(
          self
            .terminal_error
            .as_ref()
            .map(|error| div().text_color(cx.theme().danger).child(SharedString::from(error.clone()))),
        )
      })
      .children(
        self
          .preferences_error
          .as_ref()
          .map(|error| div().text_color(cx.theme().danger).child(SharedString::from(error.clone()))),
      )
  }
}

#[cfg(test)]
mod tests {
  use alvin_conversation_ui::{Model, UserMessageColors, Workspace};
  use gpui_kit::component::ActiveTheme as _;
  use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
  use gpui_kit::{AppContext as _, Bounds, MouseButton, TestAppContext, WindowBounds, WindowOptions, point, px, size};

  use super::{AppView, apply_appearance, contrast};
  use crate::app::connect;
  use crate::config::{Appearance, COLOR_CHOICES, ColorChoice, Configuration};
  use crate::test_support::ServerFixture;
  use test_context::test_context;

  #[test_context(ServerFixture)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn opening_settings_cancels_pending_dictation_and_preserves_draft(fixture: &mut ServerFixture, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let client = connect(&fixture.endpoint).await.unwrap();
      let cache = fixture.project.base().join("speech-cache");
      let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        alvin_conversation_ui::init(cache, cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
          let model = cx.new(|cx| Model::new(client.clone(), cx));
          let workspace = cx.new(|cx| Workspace::new(model, client, window, cx));
          cx.new(|cx| AppView::new(workspace, None, None, Default::default(), window, cx))
        })
        .unwrap()
      });
      let workspace = cx.read(|cx| view.read(cx).workspace.clone());
      let model = cx.read(|cx| workspace.read(cx).model().clone());
      cx.update(|cx| model.update(cx, |model, cx| model.refresh_projects(cx))).await;
      let (project, checkout) = cx.read(|cx| {
        let project = model.read(cx).conversation().workspace().projects().first().unwrap();
        (project.id.clone(), project.checkouts.first().unwrap().id.clone())
      });
      cx.update(|cx| model.update(cx, |model, cx| model.new_session(project, checkout, cx)))
        .await;
      let live = cx.read(|cx| workspace.read(cx).active().unwrap());
      let button = format!("composer-dictation-{}", live.as_uuid());
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let editor = window.find_by_label("Message").bounds().center();
        window.pointer_down(editor, MouseButton::Left, cx);
        window.pointer_up(editor, MouseButton::Left, cx);
        window.input("keep this draft", cx);
        window.click(button.clone(), cx);
        window.render_frame(cx);
        assert_eq!(window.find(button.clone()).label(), Some("Cancel dictation"));
        window.click("sidebar-settings", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-general").visible());
        window.click("back-to-sessions", cx);
        window.render_frame(cx);
        assert_eq!(window.find(button).label(), Some("Start dictation"));
        assert!(window.try_find(format!("dictation-retry-{}", live.as_uuid())).is_none());
        let editor = window.find_by_label("Message").bounds().center();
        window.pointer_down(editor, MouseButton::Left, cx);
        window.pointer_up(editor, MouseButton::Left, cx);
        window.press("secondary-a", cx);
        window.press("secondary-c", cx);
        assert_eq!(
          cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
          Some("keep this draft")
        );
      })
      .unwrap();
      cx.update(alvin_conversation_ui::shutdown).await;
    });
  }

  #[test_context(ServerFixture)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn settings_commit_only_the_changed_control(fixture: &mut ServerFixture, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let client = connect(&fixture.endpoint).await.unwrap();
      let dir = fixture.project.base().join("settings");
      std::fs::create_dir_all(&dir).unwrap();
      let path = dir.join("config.toml");
      std::fs::write(&path, format!("appearance = 'system'\nendpoint = '{}'\n", fixture.endpoint)).unwrap();
      let config = Configuration::for_test(path.clone()).unwrap();
      let (handle, view) = cx.update(|cx| {
        gpui_kit::init(cx);
        alvin_conversation_ui::init(fixture.project.base().join("speech-cache"), cx);
        gpui_kit::open_window(
          WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
              point(px(0.), px(0.)),
              size(px(1000.), px(1200.)),
            ))),
            ..Default::default()
          },
          cx,
          |window, cx| {
            let model = cx.new(|cx| Model::new(client.clone(), cx));
            let workspace = cx.new(|cx| Workspace::new(model, client, window, cx));
            workspace.update(cx, |workspace, cx| workspace.set_user_name("Ada".to_owned(), cx));
            cx.new(|cx| AppView::new(workspace, Some(config), None, Default::default(), window, cx))
          },
        )
        .unwrap()
      });
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.click("sidebar-settings", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        let sidebar = window.find("settings-sidebar").bounds();
        let back = window.find("back-to-sessions").bounds();
        let general = window.find("settings-general").bounds();
        let scroll = window.find("settings-scroll").bounds();
        let content = window.find("settings-content").bounds();
        let back_content = window.find("settings-back-content").bounds();
        let topic_content = window.within("settings-general").find("settings-topic-content").bounds();
        assert!(back.left() >= sidebar.left() && back.right() <= sidebar.right());
        assert!(back.bottom() <= general.top());
        assert_eq!(back_content.left(), topic_content.left());
        assert!(sidebar.right() <= scroll.left());
        assert!(scroll.top() > sidebar.top());
        assert!(content.left() > scroll.left() && content.right() < scroll.right());
        let left_gap = content.left() - scroll.left();
        let right_gap = scroll.right() - content.right();
        assert!(left_gap <= right_gap + px(2.) && right_gap <= left_gap + px(2.));
        assert_eq!(window.find("expand-mutating-tools").checked(), Some(true));
        assert_eq!(window.find("expand-readonly-tools").checked(), Some(false));
        assert_eq!(window.find("expand-thinking").checked(), Some(false));
        window.click("expand-mutating-tools", cx);
        window.click("expand-readonly-tools", cx);
        window.click("expand-thinking", cx);
        window.click("settings-hosts", cx);
        window.click("Gateway URL", cx);
        window.press("secondary-a", cx);
        window.input("unfinished/model", cx);
        window.click("settings-appearance", cx);
        window.click("appearance-dark", cx);
      })
      .unwrap();
      cx.run_until_parked();
      let saved = std::fs::read_to_string(&path).unwrap();
      assert!(saved.contains("dark"));
      assert!(!saved.contains("unfinished/model"));
      let reloaded = Configuration::for_test(path.clone()).unwrap();
      assert!(!reloaded.config.expand_mutating_tools);
      assert!(reloaded.config.expand_readonly_tools);
      assert!(reloaded.config.expand_thinking);
      cx.update_window(handle, |_, window, cx| {
        window.within("dark-theme").click("input", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.input("one::cool", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.press("enter", cx);
      })
      .unwrap();
      cx.run_until_parked();
      let accent_id = cx.read(|cx| view.read(cx).accent_color.entity_id());
      let user_id = cx.read(|cx| view.read(cx).user_color.entity_id());
      let accent_input_id = cx.read(|cx| view.read(cx).accent_color.read(cx).hex_input().entity_id());
      let user_input_id = cx.read(|cx| view.read(cx).user_color.read(cx).hex_input().entity_id());
      cx.update_window(handle, |_, window, cx| {
        window.click("accent-color", cx);
        let input = window
          .within(("color-select", accent_id))
          .find(("input", accent_input_id))
          .bounds()
          .center();
        window.pointer_down(input, MouseButton::Left, cx);
        window.pointer_up(input, MouseButton::Left, cx);
        window.press("secondary-a", cx);
        window.input("#34567880", cx);
        window.press("enter", cx);
      })
      .unwrap();
      cx.run_until_parked();
      let accent = ColorChoice::from_color(
        gpui_kit::Rgba {
          r: 52. / 255.,
          g: 86. / 255.,
          b: 120. / 255.,
          a: 128. / 255.,
        }
        .into(),
      );
      cx.update(|cx| {
        let config = Configuration::for_test(path.clone()).unwrap();
        assert_eq!(config.config.accent_color, accent);
        assert_eq!(ColorChoice::from_color(cx.theme().primary), accent);
      });
      cx.update_window(handle, |_, window, cx| {
        window.click("accent-color", cx);
        assert!(view.read(cx).accent_color.read(cx).is_open());
        window.press("escape", cx);
        assert!(!view.read(cx).accent_color.read(cx).is_open());
        window.click("user-message-color", cx);
        let input = window
          .within(("color-select", user_id))
          .find(("input", user_input_id))
          .bounds()
          .center();
        window.pointer_down(input, MouseButton::Left, cx);
        window.pointer_up(input, MouseButton::Left, cx);
        window.press("secondary-a", cx);
        window.input("#abcdef80", cx);
        window.press("enter", cx);
      })
      .unwrap();
      cx.run_until_parked();
      let user = ColorChoice::from_color(
        gpui_kit::Rgba {
          r: 171. / 255.,
          g: 205. / 255.,
          b: 239. / 255.,
          a: 128. / 255.,
        }
        .into(),
      );
      cx.update(|cx| {
        let config = Configuration::for_test(path.clone()).unwrap();
        assert_eq!(config.config.user_message_color, user);
        assert_eq!(ColorChoice::from_color(cx.global::<UserMessageColors>().background()), user);
      });
      cx.update_window(handle, |_, window, cx| {
        assert!(!view.read(cx).user_color.read(cx).is_open());
        window.click("user-color-accent", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update(|cx| {
        let config = Configuration::for_test(path.clone()).unwrap();
        assert_eq!(config.config.user_message_color, ColorChoice::ACCENT);
        assert_eq!(ColorChoice::from_color(cx.global::<UserMessageColors>().background()), accent);
      });
      let old_workspace = cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        view.workspace.entity_id()
      });
      let (finished, completion) = tokio::sync::oneshot::channel();
      let mut finished = Some(finished);
      let subscription = cx.update(|cx| {
        cx.observe(&view, move |view, cx| {
          let view = view.read(cx);
          if (view.workspace.entity_id() != old_workspace || view.error.is_some())
            && let Some(finished) = finished.take()
          {
            finished.send(()).unwrap();
          }
        })
      });
      cx.update_window(handle, |_, window, cx| {
        window.click("settings-hosts", cx);
        window.click("Gateway URL", cx);
        window.press("secondary-a", cx);
        window.input(
          &format!("ws://localhost:{}", fixture.server.as_ref().unwrap().local_addr().port()),
          cx,
        );
        window.press("enter", cx);
      })
      .unwrap();
      completion.await.unwrap();
      drop(subscription);
      cx.update(|cx| {
        let view = view.read(cx);
        assert!(view.error.is_none(), "{:?}", view.error);
        assert_ne!(view.workspace.entity_id(), old_workspace);
        assert_eq!(view.workspace.read(cx).user_name(), "Ada");
      });
      cx.update_window(handle, |_, window, cx| {
        window.click("back-to-sessions", cx);
        assert_eq!(window.find("user-menu").label(), Some("Ada"));
        window.click("user-menu", cx);
        window.render_frame(cx);
        let settings = window.find_by_label("Settings…").bounds().center();
        window.pointer_down(settings, MouseButton::Left, cx);
        window.pointer_up(settings, MouseButton::Left, cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        assert!(window.find_by_label("Gateway URL").visible());
        window.click("back-to-sessions", cx);
        window.click("sidebar-settings", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, _| {
        assert!(window.find_by_label("Gateway URL").visible());
      })
      .unwrap();
      cx.update_window(handle, |_, window, cx| {
        window.click("Gateway URL", cx);
        window.press("secondary-a", cx);
        window.input("not a url", cx);
        window.press("enter", cx);
      })
      .unwrap();
      cx.run_until_parked();
      let saved = std::fs::read_to_string(&path).unwrap();
      assert!(saved.contains("one::cool"));
      assert!(!saved.contains("unfinished/model"));
      assert!(!saved.contains("not a url"));
      cx.update(|cx| assert!(view.read(cx).error.as_deref().unwrap().contains("Invalid gateway URL")));
      let connected_workspace = cx.read(|cx| view.read(cx).workspace.entity_id());
      let (finished, completion) = tokio::sync::oneshot::channel();
      let mut finished = Some(finished);
      let subscription = cx.update(|cx| {
        cx.observe(&view, move |view, cx| {
          let error = view.read(cx).error.as_deref();
          if error.is_some_and(|error| error.starts_with("Couldn’t connect to gateway:") || error.starts_with("Couldn’t save settings:"))
            && let Some(finished) = finished.take()
          {
            finished.send(()).unwrap();
          }
        })
      });
      cx.update_window(handle, |_, window, cx| {
        window.click("Gateway URL", cx);
        window.press("secondary-a", cx);
        window.input("ws://127.0.0.1:1", cx);
        window.press("enter", cx);
      })
      .unwrap();
      completion.await.unwrap();
      drop(subscription);
      cx.update(|cx| {
        let view = view.read(cx);
        assert_eq!(view.workspace.entity_id(), connected_workspace);
        assert!(
          view
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("Couldn’t connect to gateway:")),
          "{:?}",
          view.error
        );
      });
      let workspace = cx.read(|cx| view.read(cx).workspace.clone());
      let model = cx.read(|cx| workspace.read(cx).model().clone());
      cx.update(|cx| model.update(cx, |model, cx| model.refresh_projects(cx))).await;
      let (project, checkout) = cx.read(|cx| {
        let project = model.read(cx).conversation().workspace().projects().first().unwrap();
        (project.id.clone(), project.checkouts.first().unwrap().id.clone())
      });
      assert!(cx.read(|cx| workspace.read(cx).active().is_none()));
      cx.update(|cx| model.update(cx, |model, cx| model.new_session(project, checkout, cx)))
        .await;
      let live = cx.read(|cx| workspace.read(cx).active().unwrap());
      let probe = connect(&fixture.endpoint).await.unwrap();
      assert!(
        alvin_protocol::LiveApiClient::list(&probe)
          .await
          .unwrap()
          .iter()
          .any(|session| session.live == live)
      );
      let saved = std::fs::read_to_string(&path).unwrap();
      assert!(saved.contains("ws://127.0.0.1:1"));
      assert!(!saved.contains("unfinished/model"));
      std::fs::remove_dir_all(dir).unwrap();
    });
  }

  #[test_context(ServerFixture)]
  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn settings_and_sidebar_controls_remain_accessible(fixture: &mut ServerFixture, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let client = connect(&fixture.endpoint).await.unwrap();
      let handle = cx.update(|cx| {
        gpui_kit::init(cx);
        alvin_conversation_ui::init(fixture.project.base().join("speech-cache"), cx);
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
          let model = cx.new(|cx| Model::new(client.clone(), cx));
          let workspace = cx.new(|cx| Workspace::new(model, client, window, cx));
          cx.new(|cx| AppView::new(workspace, None, None, Default::default(), window, cx))
        })
        .unwrap()
        .0
      });
      cx.update_window(handle, |_, window, cx| {
        apply_appearance(
          Appearance::Dark,
          "github::light",
          "one::cool",
          ColorChoice::MODULE,
          ColorChoice::OPERATOR,
          window,
          cx,
        );
        let background = verdant_themes::from_str("one::cool").unwrap().bg().unwrap();
        let expected = gpui_kit::Hsla::from(gpui_kit::rgb(
          (u32::from(background.red) << 16) | (u32::from(background.green) << 8) | u32::from(background.blue),
        ));
        assert_eq!(cx.theme().background, expected);
        assert_eq!(cx.theme().highlight_theme.name, "one::cool");
        let mut first_background = None;
        for (appearance, light, dark, name) in [
          (Appearance::Light, "catppuccin::latte", "catppuccin::mocha", "catppuccin::latte"),
          (Appearance::Dark, "catppuccin::latte", "catppuccin::mocha", "catppuccin::mocha"),
          (Appearance::Light, "catppuccin::latte", "catppuccin::mocha", "catppuccin::latte"),
          (Appearance::Dark, "github::light", "one::cool", "one::cool"),
        ] {
          apply_appearance(appearance, light, dark, ColorChoice::MODULE, ColorChoice::OPERATOR, window, cx);
          let theme = verdant_themes::from_str(name).unwrap();
          macro_rules! convert {
            ($color:expr) => {{
              let color = $color;
              gpui_kit::Hsla::from(gpui_kit::rgb(
                (u32::from(color.red) << 16) | (u32::from(color.green) << 8) | u32::from(color.blue),
              ))
            }};
          }
          let background = convert!(theme.bg().unwrap());
          let foreground = convert!(theme.fg().unwrap());
          assert_eq!(cx.theme().background, background);
          assert_eq!(cx.theme().foreground, foreground);
          assert_eq!(cx.theme().primary, convert!(theme.find_style("module").unwrap().color()));
          assert_eq!(cx.theme().link, convert!(theme.find_style("function").unwrap().color()));
          for &choice in COLOR_CHOICES {
            apply_appearance(appearance, light, dark, choice, ColorChoice::ACCENT, window, cx);
            let user = cx.global::<UserMessageColors>();
            assert_eq!(user.background(), cx.theme().primary);
            assert!(contrast(user.background(), user.foreground()) >= 4.5, "{name}: {choice}");
          }
          for color in [
            gpui_kit::Hsla::from(gpui_kit::rgb(0)).alpha(0.),
            gpui_kit::Hsla::from(gpui_kit::rgb(0xffffff)).alpha(0.),
            gpui_kit::Hsla::from(gpui_kit::rgb(0x123abc)).alpha(0.25),
          ] {
            let choice = ColorChoice::from_color(color);
            apply_appearance(appearance, light, dark, choice, ColorChoice::ACCENT, window, cx);
            let user = cx.global::<UserMessageColors>();
            let surface = cx.theme().background.blend(user.background());
            assert!(contrast(surface, user.foreground()) >= 4.5, "{name}: {choice}");
            assert!(contrast(cx.theme().background.blend(cx.theme().primary), cx.theme().primary_foreground) >= 4.5);
          }
          if name.starts_with("catppuccin::") {
            let colors = cx.theme();
            assert_ne!(colors.sidebar, colors.background);
            assert_ne!(colors.muted, colors.background);
            assert_ne!(colors.border, colors.background);
            assert!(contrast(colors.foreground, colors.background) >= 4.5);
            assert!(contrast(colors.muted_foreground, colors.muted) >= 4.5);
            for (color, foreground) in [
              (colors.danger, colors.danger_foreground),
              (colors.success, colors.success_foreground),
              (colors.warning, colors.warning_foreground),
              (colors.info, colors.info_foreground),
            ] {
              assert!(contrast(color, foreground) >= 4.5, "{name}");
            }
            assert_ne!(colors.info, colors.success);
            assert_ne!(colors.success, colors.danger);
          } else {
            if let Some(style) = theme.get("diff.plus") {
              assert_eq!(cx.theme().success, convert!(style.color()));
            }
            if let Some(style) = theme.get("diff.minus") {
              assert_eq!(cx.theme().danger, convert!(style.color()));
            }
            for (key, actual) in [("comment.warning", cx.theme().warning), ("comment.note", cx.theme().info)] {
              if let Some(style) = theme.get(key) {
                assert_eq!(actual, convert!(style.bg().unwrap_or_else(|| style.color())));
              }
            }
          }
          if name == "catppuccin::latte" {
            if let Some(previous) = first_background {
              assert_eq!(background, previous);
            } else {
              first_background = Some(background);
            }
          } else {
            assert_ne!(Some(background), first_background);
          }
          assert_eq!(cx.theme().highlight_theme.name, name);
        }
        window.render_frame(cx);
        assert!(window.find("titlebar-toggle-sidebar").visible());
        window.click("sidebar-settings", cx);
      })
      .unwrap();
      cx.run_until_parked();
      cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.find("settings-general").visible());
        window.click("settings-general", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-general").focused(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-appearance").focused(), Some(true));
        window.press("shift-tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-general").focused(), Some(true));
        window.press("tab", cx);
        window.render_frame(cx);
        assert_eq!(window.find("settings-appearance").focused(), Some(true));
        window.press("space", cx);
        window.render_frame(cx);
        assert!(window.find_by_label("Light theme").visible());
        window.click("settings-terminal", cx);
        assert!(window.find_by_label("Terminal font family").visible());
        window.click("settings-hosts", cx);
        assert!(window.find_by_label("Gateway URL").visible());
        window.click("back-to-sessions", cx);
        window.render_frame(cx);
        assert!(window.try_find_by_label("Gateway URL").is_none());
        window.click("titlebar-toggle-sidebar", cx);
        window.render_frame(cx);
        assert_eq!(window.find("titlebar-toggle-sidebar").label(), Some("Show sidebar"));
        window.click("titlebar-toggle-sidebar", cx);
        window.render_frame(cx);
        assert_eq!(window.find("titlebar-toggle-sidebar").label(), Some("Hide sidebar"));
      })
      .unwrap();
    });
  }
}
