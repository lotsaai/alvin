//! The model and thinking-level pickers of a session, with a cycle action for each.
//!
//! The options come from the model's catalog and the current values from `live/state` and its events. The pickers hold no
//! domain state: they mirror it, and a choice goes to the model as a command.

use alvin_conversation::LiveView;
use alvin_protocol::{LiveSessionId, ModelInfo, ThinkingLevel};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::{
  App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Subscription, Window, div, rems,
};

use crate::model::{Catalog, Model};
use crate::session::SessionView;

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

/// A model is identified by provider and id together: two providers can offer the same id.
fn model_key(model: &ModelInfo) -> String {
  format!("{}/{}", model.provider, model.id)
}

fn model_choice(model: &ModelInfo) -> Choice {
  Choice {
    title: SharedString::from(format!("{} ({})", model.name, model.provider)),
    key: model_key(model),
  }
}

fn level_choice(level: ThinkingLevel) -> Choice {
  Choice {
    title: SharedString::from(level.as_str()),
    key: level.as_str().to_owned(),
  }
}

/// The two selects of one session and what they were last given.
pub(crate) struct Pickers {
  models: Entity<SelectState<Vec<Choice>>>,
  levels: Entity<SelectState<Vec<Choice>>>,
  offered_models: Vec<String>,
  offered_levels: Vec<String>,
  _subscriptions: Vec<Subscription>,
}

impl Pickers {
  pub(crate) fn new(model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<SessionView>) -> Self {
    let models = cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx));
    let levels = cx.new(|cx| SelectState::new(Vec::<Choice>::new(), None, window, cx));
    let subscriptions = vec![
      cx.subscribe_in(&models, window, {
        let model = model.clone();
        move |_, _, event: &SelectEvent<Vec<Choice>>, _, cx| {
          let SelectEvent::Confirm(Some(key)) = event else { return };
          let chosen = model
            .read(cx)
            .catalog(live)
            .and_then(|catalog| catalog.models.iter().find(|candidate| &model_key(candidate) == key))
            .cloned();
          let current = current_model(model.read(cx), live);
          if let Some(chosen) = chosen
            && current.as_ref() != Some(key)
          {
            model
              .update(cx, |model, cx| model.set_model(live, chosen.provider, chosen.id, cx))
              .detach();
          }
        }
      }),
      cx.subscribe_in(&levels, window, {
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
      }),
    ];
    Self {
      models,
      levels,
      offered_models: Vec::new(),
      offered_levels: Vec::new(),
      _subscriptions: subscriptions,
    }
  }

  /// Makes the selects show what the model holds: the options of the catalog, and the model and level in use.
  pub(crate) fn sync(&mut self, model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
    let (catalog, current_model, current_level) = {
      let model = model.read(cx);
      let Some(view) = model.conversation().live(live) else { return };
      (model.catalog(live).cloned(), view.model.as_ref().map(model_key), view.thinking)
    };
    if let Some(Catalog { models, levels }) = &catalog {
      let keys: Vec<String> = models.iter().map(model_key).collect();
      if keys != self.offered_models {
        let choices: Vec<Choice> = models.iter().map(model_choice).collect();
        self.models.update(cx, |select, cx| select.set_items(choices, window, cx));
        self.offered_models = keys;
      }
      let keys: Vec<String> = levels.iter().map(|level| level.as_str().to_owned()).collect();
      if keys != self.offered_levels {
        let choices: Vec<Choice> = levels.iter().copied().map(level_choice).collect();
        self.levels.update(cx, |select, cx| select.set_items(choices, window, cx));
        self.offered_levels = keys;
      }
    }
    if let Some(key) = current_model {
      self.models.update(cx, |select, cx| select.set_selected_value(&key, window, cx));
    }
    if let Some(level) = current_level {
      self
        .levels
        .update(cx, |select, cx| select.set_selected_value(&level.as_str().to_owned(), window, cx));
    }
  }

  /// Compact model and thinking controls for the composer toolbar.
  pub(crate) fn view(&self) -> impl IntoElement {
    h_flex()
      .flex_1()
      .min_w(rems(10.))
      .gap_1()
      .child(
        div().flex_1().min_w_0().max_w(rems(16.)).child(
          Select::new(&self.models)
            .id("model-picker")
            .accessibility_label("Model")
            .xsmall()
            .appearance(false)
            .menu_width(rems(20.)),
        ),
      )
      .child(
        div().w(rems(7.)).min_w_0().child(
          Select::new(&self.levels)
            .id("thinking-picker")
            .accessibility_label("Thinking level")
            .xsmall()
            .appearance(false),
        ),
      )
  }
}

fn current_model(model: &Model, live: LiveSessionId) -> Option<String> {
  model
    .conversation()
    .live(live)
    .and_then(|view: &LiveView| view.model.as_ref())
    .map(model_key)
}
