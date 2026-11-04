//! The model and thinking-level pickers of a session, with a cycle action for each.
//!
//! The options come from the model's catalog and the current values from `live/state` and its events. The pickers hold no
//! domain state: they mirror it, and a choice goes to the model as a command.

mod model_picker;

use alvin_protocol::{ConfiguredThinkingLevel, LiveSessionId, ThinkingLevel};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::{
  App, AppContext as _, Context, Entity, FocusHandle, IntoElement, ParentElement as _, SharedString, Styled as _, Subscription, Window,
  div, rems,
};

use crate::model::{Catalog, Model};
use crate::session::composer::Composer;

/// One option of a picker: what the user reads, and the key that identifies it.
#[derive(Debug, Clone)]
pub(crate) struct Choice {
  title: SharedString,
  key: String,
}

impl SearchableListItem for Choice {
  type Value = String;

  fn title(&self) -> SharedString {
    self.title.clone()
  }

  fn value(&self) -> &String {
    &self.key
  }
}

fn level_title(level: ThinkingLevel) -> &'static str {
  match level {
    ThinkingLevel::Inherit => "inherit",
    ThinkingLevel::Off => "off",
    ThinkingLevel::Minimal => "○ minimal",
    ThinkingLevel::Low => "◔ low",
    ThinkingLevel::Medium => "◑ medium",
    ThinkingLevel::High => "◒ high",
    ThinkingLevel::Xhigh => "◕ xhigh",
    ThinkingLevel::Max => "◉ max",
  }
}

fn level_choice(level: ThinkingLevel) -> Choice {
  Choice {
    title: SharedString::from(level_title(level)),
    key: level.as_str().to_owned(),
  }
}

/// The model and thinking controls for one session.
pub(crate) struct Pickers {
  model: Entity<Model>,
  live: LiveSessionId,
  current: SharedString,
  thinking_label: SharedString,
  trigger: FocusHandle,
  levels: Entity<SelectState<Vec<Choice>>>,
  offered_levels: Vec<String>,
  _subscriptions: Vec<Subscription>,
}

impl Pickers {
  pub(crate) fn new(model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Composer>) -> Self {
    let levels = cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx));
    let subscriptions = vec![cx.subscribe_in(&levels, window, {
      let model = model.clone();
      move |_, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
        let SelectEvent::Confirm(Some(key)) = event else { return };
        let chosen = model
          .read(cx)
          .catalog(live)
          .and_then(|catalog| catalog.levels.iter().find(|level| level.as_str() == key))
          .copied();
        let current = model.read(cx).conversation().live(live).and_then(|view| view.thinking);
        if let Some(chosen) = chosen
          && current != Some(chosen)
        {
          model.update(cx, |model, cx| model.set_thinking(live, chosen, cx)).detach();
        }
      }
    })];
    Self {
      model: model.clone(),
      live,
      current: "Model".into(),
      thinking_label: "Thinking".into(),
      trigger: cx.focus_handle().tab_stop(true),
      levels,
      offered_levels: Vec::new(),
      _subscriptions: subscriptions,
    }
  }

  /// Mirrors the session's current model and thinking level.
  pub(crate) fn sync(&mut self, model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
    let (catalog, current, current_level, automatic) = {
      let model = model.read(cx);
      let Some(view) = model.conversation().live(live) else { return };
      (
        model.catalog(live).cloned(),
        view.model.as_ref().map(|model| format!("{} ({})", model.name, model.provider)),
        view.thinking,
        view.thinking_selector == Some(ConfiguredThinkingLevel::Auto),
      )
    };
    self.current = current.map_or_else(|| "Model".into(), SharedString::from);
    self.thinking_label = if automatic {
      "⟳ auto"
    } else {
      current_level.map_or("Thinking", level_title)
    }
    .into();
    if let Some(Catalog { levels, .. }) = &catalog {
      let keys: Vec<String> = levels.iter().map(|level| level.as_str().to_owned()).collect();
      if keys != self.offered_levels {
        let choices: Vec<Choice> = levels.iter().copied().map(level_choice).collect();
        self.levels.update(cx, |select, cx| select.set_items(choices, window, cx));
        self.offered_levels = keys;
      }
    }
    if automatic {
      self.levels.update(cx, |select, cx| select.set_selected_index(None, window, cx));
    } else if let Some(level) = current_level {
      self
        .levels
        .update(cx, |select, cx| select.set_selected_value(&level.as_str().to_owned(), window, cx));
    }
  }

  pub(crate) fn open_model(&self, window: &mut Window, cx: &mut App) {
    model_picker::open(self.model.clone(), self.live, window, cx);
  }

  /// Compact model and thinking controls for the composer toolbar.
  pub(crate) fn view(&self) -> impl IntoElement {
    let model = self.model.clone();
    let live = self.live;
    let trigger = self.trigger.clone();
    h_flex()
      .flex_1()
      .min_w(rems(12.))
      .gap_1()
      .child(
        div().flex_1().min_w_0().max_w(rems(16.)).child(
          Button::new("model-picker")
            .ghost()
            .xsmall()
            .min_w_0()
            .w_full()
            .child(div().min_w_0().truncate().child(self.current.clone()))
            .accessibility_label("Model")
            .tooltip("Model")
            .track_focus(&self.trigger)
            .on_click(move |_, window, cx| {
              trigger.focus(window, cx);
              model_picker::open(model.clone(), live, window, cx);
            }),
        ),
      )
      .child(
        div().w(rems(6.)).flex_shrink_0().child(
          Select::new(&self.levels)
            .id("thinking-picker")
            .accessibility_label("Thinking level")
            .placeholder(self.thinking_label.clone())
            .xsmall()
            .appearance(false),
        ),
      )
  }
}
