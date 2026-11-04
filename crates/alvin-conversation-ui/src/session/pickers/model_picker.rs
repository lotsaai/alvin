//! Search and choose a model without changing the session until confirmation.

use std::cell::Cell;

use alvin_protocol::{LiveSessionId, ModelInfo};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogFooter, DialogHeader, DialogTitle};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  App, AppContext as _, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
  Task, Window, div, rems,
};

use crate::model::{Model, ModelChange};

#[derive(Clone, PartialEq, Eq)]
struct Key {
  provider: String,
  id: String,
}

impl From<&ModelInfo> for Key {
  fn from(model: &ModelInfo) -> Self {
    Self {
      provider: model.provider.clone(),
      id: model.id.clone(),
    }
  }
}

impl Key {
  fn matches(&self, model: &ModelInfo) -> bool {
    self.provider == model.provider && self.id == model.id
  }
}

struct Models {
  models: Vec<ModelInfo>,
  filtered: Vec<usize>,
  query: String,
  pending: Option<Key>,
  current: Option<Key>,
  loaded: bool,
}

impl Models {
  fn refresh(&mut self, model: &Model, live: LiveSessionId) -> bool {
    let current = model.conversation().live(live).and_then(|view| view.model.as_ref());
    let catalog = model.catalog(live);
    let current_changed = match (&self.current, current) {
      (Some(key), Some(model)) => !key.matches(model),
      (None, None) => false,
      _ => true,
    };
    let catalog_changed = self.loaded != catalog.is_some() || catalog.is_some_and(|catalog| catalog.models != self.models);
    if !current_changed && !catalog_changed {
      return false;
    }
    if current_changed {
      self.current = current.map(Key::from);
    }
    if catalog_changed {
      self.loaded = catalog.is_some();
      self.models = catalog.map_or_else(Vec::new, |catalog| catalog.models.clone());
      if self
        .pending
        .as_ref()
        .is_some_and(|key| !self.models.iter().any(|model| key.matches(model)))
      {
        self.pending = None;
      }
      self.filter();
    }
    true
  }

  fn filter(&mut self) {
    let query = self.query.to_lowercase();
    self.filtered = self
      .models
      .iter()
      .enumerate()
      .filter_map(|(index, model)| {
        (model.provider.to_lowercase().contains(&query)
          || model.id.to_lowercase().contains(&query)
          || model.name.to_lowercase().contains(&query))
        .then_some(index)
      })
      .collect();
  }

  fn selected(&self) -> Option<&ModelInfo> {
    let key = self.pending.as_ref()?;
    self.models.iter().find(|model| key.matches(model))
  }
}

impl ListDelegate for Models {
  type Item = ListItem;

  fn items_count(&self, _: usize, _: &App) -> usize {
    self.filtered.len()
  }

  fn perform_search(&mut self, query: &str, _: &mut Window, cx: &mut Context<ListState<Self>>) -> Task<()> {
    self.query = query.to_owned();
    self.filter();
    cx.notify();
    Task::ready(())
  }

  fn set_selected_index(&mut self, ix: Option<IndexPath>, _: &mut Window, cx: &mut Context<ListState<Self>>) {
    self.pending = ix
      .and_then(|ix| self.filtered.get(ix.row))
      .and_then(|index| self.models.get(*index))
      .map(Key::from);
    cx.notify();
  }

  fn render_item(&mut self, ix: IndexPath, _: &mut Window, cx: &mut Context<ListState<Self>>) -> Option<ListItem> {
    let model = self.filtered.get(ix.row).and_then(|index| self.models.get(*index))?;
    let current = self.current.as_ref().is_some_and(|key| key.matches(model));
    let label = format!(
      "{} · {}/{}{}",
      model.name,
      model.provider,
      model.id,
      if current { " · Current" } else { "" }
    );
    Some(
      ListItem::new(SharedString::from(format!(
        "{}:{}{}",
        model.provider.len(),
        model.provider,
        model.id
      )))
      .accessibility_label(label)
      .child(
        h_flex()
          .w_full()
          .min_w_0()
          .gap_2()
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .child(div().min_w_0().truncate().child(model.name.clone()))
              .child(
                div()
                  .min_w_0()
                  .truncate()
                  .text_xs()
                  .text_color(cx.theme().muted_foreground)
                  .child(format!("{}/{}", model.provider, model.id)),
              ),
          )
          .when(current, |row| {
            row.child(
              div()
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Current"),
            )
          }),
      ),
    )
  }

  fn render_empty(&mut self, _: &mut Window, cx: &mut Context<ListState<Self>>) -> impl IntoElement {
    let message = if !self.loaded {
      "Loading models…"
    } else if self.models.is_empty() {
      "No models available"
    } else {
      "No models match. Clear search to see all models."
    };
    div().p_4().text_color(cx.theme().muted_foreground).child(message)
  }
}

struct ModelPicker {
  model: Entity<Model>,
  live: LiveSessionId,
  list: Entity<ListState<Models>>,
  _subscriptions: Vec<Subscription>,
}

impl ModelPicker {
  fn new(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let mut models = Models {
      models: Vec::new(),
      filtered: Vec::new(),
      query: String::new(),
      pending: None,
      current: None,
      loaded: false,
    };
    models.refresh(model.read(cx), live);
    models.pending = models
      .current
      .clone()
      .filter(|key| models.models.iter().any(|candidate| key.matches(candidate)));
    let selected = models.pending.as_ref().and_then(|key| {
      models
        .filtered
        .iter()
        .position(|index| models.models.get(*index).is_some_and(|candidate| key.matches(candidate)))
    });
    let list = cx.new(|cx| ListState::new(models, window, cx).searchable(true));
    if let Some(row) = selected {
      list.update(cx, |list, cx| {
        list.set_selected_index(Some(IndexPath::default().row(row)), window, cx)
      });
    }
    let subscriptions = vec![
      cx.observe(&list, |_, _, cx| cx.notify()),
      cx.subscribe_in(&model, window, |this, model, change: &ModelChange, window, cx| {
        if !matches!(change, ModelChange::Session(id) if *id == this.live) {
          return;
        }
        this.list.update(cx, |list, cx| {
          if !list.delegate_mut().refresh(model.read(cx), this.live) {
            return;
          }
          let pending = list.delegate().pending.clone();
          let row = pending.as_ref().and_then(|key| {
            list
              .delegate()
              .filtered
              .iter()
              .position(|index| list.delegate().models.get(*index).is_some_and(|candidate| key.matches(candidate)))
          });
          list.set_selected_index(row.map(|row| IndexPath::default().row(row)), window, cx);
          list.delegate_mut().pending = pending;
          cx.notify();
        });
      }),
    ];
    Self {
      model,
      live,
      list,
      _subscriptions: subscriptions,
    }
  }
}

impl Render for ModelPicker {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let models = self.list.read(cx).delegate();
    let count = models.filtered.len();
    let selected = models.selected();
    let preview = selected.map(|model| {
      let context = model.context_window.map_or_else(|| "Not reported".to_owned(), |value| value.to_string());
      let output = model.max_tokens.map_or_else(|| "Not reported".to_owned(), |value| value.to_string());
      format!("{}\n{}/{}\nContext: {} tokens · Output: {} tokens\nReasoning: {}\nUSD per million tokens: input {} · output {} · cache read {} · cache write {}", model.name, model.provider, model.id, context, output, if model.reasoning { "Yes" } else { "No" }, model.cost.input, model.cost.output, model.cost.cache_read, model.cost.cache_write)
    }).unwrap_or_else(|| "Select a model to see its details".to_owned());
    let enabled = selected.is_some();
    let list = self.list.clone();
    let model = self.model.clone();
    let live = self.live;
    v_flex()
      .min_w_0()
      .gap_3()
      .child(DialogHeader::new().child(DialogTitle::new().child("Choose model for session")))
      .child(
        div()
          .text_sm()
          .text_color(cx.theme().muted_foreground)
          .child(format!("{count} models · Enter selects; use the button to apply")),
      )
      .child(
        div()
          .min_w_0()
          .h(rems(14.))
          .child(List::new(&self.list).search_placeholder("Search models, providers, or IDs")),
      )
      .child(div().min_w_0().text_sm().child(preview))
      .child(
        DialogFooter::new()
          .child(
            Button::new("model-picker-close")
              .label("Close")
              .on_click(|_, window, cx| window.close_dialog(cx)),
          )
          .child(
            Button::new("model-picker-use")
              .primary()
              .label("Use for session")
              .disabled(!enabled)
              .on_click(move |_, window, cx| {
                let pending = list.read(cx).delegate().pending.clone();
                let chosen = pending.and_then(|key| {
                  model
                    .read(cx)
                    .catalog(live)
                    .and_then(|catalog| {
                      catalog
                        .models
                        .iter()
                        .find(|candidate| candidate.provider == key.provider && candidate.id == key.id)
                    })
                    .cloned()
                });
                if let Some(chosen) = chosen {
                  let unchanged = model
                    .read(cx)
                    .conversation()
                    .live(live)
                    .and_then(|view| view.model.as_ref())
                    .is_some_and(|current| current.provider == chosen.provider && current.id == chosen.id);
                  if !unchanged {
                    model.update(cx, |model, cx| model.switch_model(live, &chosen, cx)).detach();
                  }
                  window.close_dialog(cx);
                }
              }),
          ),
      )
  }
}

pub(super) fn open(model: Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
  let picker = cx.new(|cx| ModelPicker::new(model, live, window, cx));
  let initial_focus = Cell::new(true);
  window.open_dialog(cx, move |dialog, window, cx| {
    if initial_focus.replace(false) {
      picker.read(cx).list.focus_handle(cx).focus(window, cx);
    }
    dialog
      .centered()
      .w(rems(42.).to_pixels(window.rem_size()))
      .on_ok(|_, _, _| false)
      .child(picker.clone())
  });
}

#[cfg(test)]
mod tests {
  use alvin_protocol::{ModelCost, ModelInfo};

  use super::{Key, Models};

  fn model(provider: &str, id: &str, name: &str) -> ModelInfo {
    ModelInfo {
      provider: provider.into(),
      id: id.into(),
      name: name.into(),
      api: String::new(),
      base_url: String::new(),
      reasoning: false,
      input: vec!["text".into()],
      cost: ModelCost {
        input: 0.,
        output: 0.,
        cache_read: 0.,
        cache_write: 0.,
      },
      context_window: None,
      max_tokens: None,
      headers: None,
      premium_multiplier: None,
      prefer_websockets: None,
      context_promotion_target: None,
      priority: None,
      thinking: None,
      compat: None,
    }
  }

  #[test]
  fn searches_provider_name_and_id_without_collapsing_provider_collisions() {
    let mut models = Models {
      models: vec![model("Alpha", "shared", "First"), model("Beta", "shared", "Second")],
      filtered: Vec::new(),
      query: "SHARED".into(),
      pending: None,
      current: None,
      loaded: true,
    };
    models.filter();
    assert_eq!(models.filtered.len(), 2);
    models.pending = Some(Key::from(&models.models[1]));
    assert_eq!(models.selected().map(|model| model.provider.as_str()), Some("Beta"));
    models.query = "ALPHA".into();
    models.filter();
    assert_eq!(models.filtered, vec![0]);
    models.query = "second".into();
    models.filter();
    assert_eq!(models.filtered, vec![1]);
    models.query = "missing".into();
    models.filter();
    assert!(models.filtered.is_empty());
    models.query.clear();
    models.filter();
    assert_eq!(models.filtered, vec![0, 1]);
  }
}
