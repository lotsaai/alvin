//! The dialog an extension or a tool is waiting on: `select`, `confirm`, `input`, `editor` and `ask`.
//!
//! omp can have several open; the session shows the oldest and the rest wait their turn. The host holds only what the user is
//! in the middle of choosing or typing; whether a dialog is open is the model's.

use std::collections::BTreeMap;
use std::rc::Rc;

use alvin_conversation::request_id;
use alvin_protocol::{
  AnswersUiResponse, AskAnswer, AskUiRequest, CancelUiResponse, ConfirmUiResponse, ExtensionUiRequest, ExtensionUiResponse, LitTrue,
  LiveSessionId, ValueUiResponse,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, AppContext as _, Div, ElementId, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
  SharedString, Stateful, Styled as _, Window, div,
};

use crate::model::Model;

type Respond = Rc<dyn Fn(ExtensionUiResponse, &mut App)>;

fn element(prefix: &str, index: usize) -> ElementId {
  ElementId::from(SharedString::from(format!("{prefix}-{index}")))
}

fn title(text: &str) -> Div {
  div().font_weight(FontWeight::SEMIBOLD).child(SharedString::from(text.to_owned()))
}

/// What the user is choosing or typing in the dialog on show.
pub(crate) struct DialogHost {
  /// The request on show, so a new one starts from a clean slate.
  shown: Option<String>,
  /// The text of an `input` or `editor` dialog.
  text: Entity<TextareaState>,
  /// The free-text box of each `ask` question, by question id.
  custom: BTreeMap<String, Entity<TextareaState>>,
  /// The options chosen for each `ask` question, by question id.
  chosen: BTreeMap<String, Vec<String>>,
}

impl DialogHost {
  pub(crate) fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
    let text = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 8));
    cx.new(|_| Self {
      shown: None,
      text,
      custom: BTreeMap::new(),
      chosen: BTreeMap::new(),
    })
  }

  /// Starts over when the dialog on show changed.
  pub(crate) fn sync(host: &Entity<Self>, model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
    let current = model
      .read(cx)
      .conversation()
      .live(live)
      .and_then(|view| view.dialogs.pending().first().cloned());
    let current_id = current.as_ref().map(|request| request_id(request).to_owned());
    if current_id == host.read(cx).shown {
      return;
    }
    let prefill = match &current {
      Some(ExtensionUiRequest::Editor(request)) => request.prefill.clone().unwrap_or_default(),
      _ => String::new(),
    };
    let text = host.read(cx).text.clone();
    text.update(cx, |text, cx| text.set_value(prefill, window, cx));
    let mut custom = BTreeMap::new();
    let mut chosen = BTreeMap::new();
    if let Some(ExtensionUiRequest::Ask(ask)) = &current {
      for question in &ask.questions {
        custom.insert(
          question.id.clone(),
          cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 4).placeholder("Something else")),
        );
        let recommended = question
          .recommended
          .and_then(|index| usize::try_from(index).ok())
          .and_then(|index| question.options.get(index));
        if let Some(option) = recommended {
          chosen.insert(question.id.clone(), vec![option.label.clone()]);
        }
      }
    }
    host.update(cx, |host, cx| {
      host.shown = current_id;
      host.custom = custom;
      host.chosen = chosen;
      cx.notify();
    });
  }

  /// Chooses an option of an `ask` question. A question that takes one answer replaces the choice; one that takes several
  /// toggles the option.
  fn choose(&mut self, question: &str, label: &str, multi: bool) {
    let chosen = self.chosen.entry(question.to_owned()).or_default();
    if !multi {
      *chosen = vec![label.to_owned()];
    } else if let Some(position) = chosen.iter().position(|option| option == label) {
      chosen.remove(position);
    } else {
      chosen.push(label.to_owned());
    }
  }

  /// The answers of an `ask`, once every question has an option or some text.
  fn answers(&self, ask: &AskUiRequest, cx: &App) -> Option<Vec<AskAnswer>> {
    ask
      .questions
      .iter()
      .map(|question| {
        let selected_options = self.chosen.get(&question.id).cloned().unwrap_or_default();
        let custom = self
          .custom
          .get(&question.id)
          .map(|text| text.read(cx).value().trim().to_owned())
          .filter(|text| !text.is_empty());
        (!selected_options.is_empty() || custom.is_some()).then(|| AskAnswer {
          id: question.id.clone(),
          selected_options,
          custom_input: custom,
        })
      })
      .collect()
  }
}

/// The oldest pending dialog of the session, or nothing.
pub(crate) fn view(host: &Entity<DialogHost>, model: &Entity<Model>, live: LiveSessionId, cx: &App) -> Option<AnyElement> {
  let request = model.read(cx).conversation().live(live)?.dialogs.pending().first()?.clone();
  let respond: Respond = {
    let model = model.clone();
    Rc::new(move |response, cx| model.update(cx, |model, cx| model.respond(live, response, cx)))
  };
  let cancel = |request_id: &str| {
    let (request_id, respond) = (request_id.to_owned(), respond.clone());
    Button::new(ElementId::from("dialog-cancel"))
      .small()
      .ghost()
      .label("Cancel")
      .on_click(move |_, _, cx| {
        respond(
          ExtensionUiResponse::CancelUiResponse(CancelUiResponse {
            id: request_id.clone(),
            cancelled: LitTrue,
            timed_out: None,
          }),
          cx,
        );
      })
  };
  let panel = v_flex().id("dialog").p_3().gap_2().border_t_1().border_color(cx.theme().border);
  let dialog = match &request {
    ExtensionUiRequest::Confirm(confirm) => {
      let answer = |name: &'static str, label: &'static str, confirmed: bool| {
        let (request_id, respond) = (confirm.id.clone(), respond.clone());
        Button::new(ElementId::from(name)).small().label(label).on_click(move |_, _, cx| {
          respond(
            ExtensionUiResponse::ConfirmUiResponse(ConfirmUiResponse {
              id: request_id.clone(),
              confirmed,
            }),
            cx,
          );
        })
      };
      panel
        .child(title(&confirm.title))
        .child(SharedString::from(confirm.message.clone()))
        .child(
          h_flex()
            .gap_2()
            .child(answer("dialog-confirm-yes", "Yes", true))
            .child(answer("dialog-confirm-no", "No", false))
            .child(cancel(&confirm.id)),
        )
    }
    ExtensionUiRequest::Select(select) => {
      let mut options = h_flex().gap_2().flex_wrap();
      for (index, option) in select.options.iter().enumerate() {
        let (request_id, value, respond) = (select.id.clone(), option.clone(), respond.clone());
        options = options.child(
          Button::new(element("dialog-option", index))
            .small()
            .label(SharedString::from(option.clone()))
            .on_click(move |_, _, cx| {
              respond(
                ExtensionUiResponse::ValueUiResponse(ValueUiResponse {
                  id: request_id.clone(),
                  value: value.clone(),
                }),
                cx,
              );
            }),
        );
      }
      panel.child(title(&select.title)).child(options).child(cancel(&select.id))
    }
    ExtensionUiRequest::Input(input) => text_dialog(panel, host, &input.title, &input.id, &respond, cancel(&input.id), cx),
    ExtensionUiRequest::Editor(editor) => text_dialog(panel, host, &editor.title, &editor.id, &respond, cancel(&editor.id), cx),
    ExtensionUiRequest::Ask(ask) => ask_dialog(panel, host, ask, &respond, cancel(&ask.id), cx),
    // Requests that are not dialogs never wait in the model.
    ExtensionUiRequest::Cancel(_)
    | ExtensionUiRequest::Notify(_)
    | ExtensionUiRequest::SetStatus(_)
    | ExtensionUiRequest::SetWidget(_)
    | ExtensionUiRequest::SetTitle(_)
    | ExtensionUiRequest::SetEditorText(_)
    | ExtensionUiRequest::OpenUrl(_) => return None,
  };
  Some(dialog.into_any_element())
}

fn text_dialog(
  panel: Stateful<Div>,
  host: &Entity<DialogHost>,
  heading: &str,
  request_id: &str,
  respond: &Respond,
  cancel: Button,
  cx: &App,
) -> Stateful<Div> {
  let text = host.read(cx).text.clone();
  let (request_id, respond, answer) = (request_id.to_owned(), respond.clone(), text.clone());
  panel
    .child(title(heading))
    .child(Textarea::new(&text).accessibility_id("dialog-text").aria_label("Answer"))
    .child(
      h_flex()
        .gap_2()
        .child(
          Button::new(ElementId::from("dialog-submit"))
            .small()
            .label("Submit")
            .on_click(move |_, _, cx| {
              let value = answer.read(cx).value().to_string();
              respond(
                ExtensionUiResponse::ValueUiResponse(ValueUiResponse {
                  id: request_id.clone(),
                  value,
                }),
                cx,
              );
            }),
        )
        .child(cancel),
    )
}

fn ask_dialog(
  panel: Stateful<Div>,
  host: &Entity<DialogHost>,
  ask: &AskUiRequest,
  respond: &Respond,
  cancel: Button,
  cx: &App,
) -> Stateful<Div> {
  let mut panel = panel;
  for (q, question) in ask.questions.iter().enumerate() {
    let mut options = h_flex().gap_2().flex_wrap();
    for (o, option) in question.options.iter().enumerate() {
      let selected = host
        .read(cx)
        .chosen
        .get(&question.id)
        .is_some_and(|chosen| chosen.contains(&option.label));
      let (host, question_id, label, multi) = (host.clone(), question.id.clone(), option.label.clone(), question.multi);
      options = options.child(
        Button::new(ElementId::from(SharedString::from(format!("dialog-ask-{q}-{o}"))))
          .small()
          .selected(selected)
          .label(SharedString::from(option.label.clone()))
          .on_click(move |_, _, cx| {
            host.update(cx, |host, cx| {
              host.choose(&question_id, &label, multi);
              cx.notify();
            });
          }),
      );
    }
    panel = panel.child(title(&question.question)).child(options);
    if let Some(custom) = host.read(cx).custom.get(&question.id) {
      panel = panel.child(
        Textarea::new(custom)
          .accessibility_id(SharedString::from(format!("dialog-ask-text-{q}")))
          .aria_label("Something else"),
      );
    }
  }
  let ready = host.read(cx).answers(ask, cx).is_some();
  let (host, respond, ask) = (host.clone(), respond.clone(), ask.clone());
  panel.child(
    h_flex()
      .gap_2()
      .child(
        Button::new(ElementId::from("dialog-ask-submit"))
          .small()
          .label("Submit")
          .disabled(!ready)
          .on_click(move |_, _, cx| {
            if let Some(answers) = host.read(cx).answers(&ask, cx) {
              respond(
                ExtensionUiResponse::AnswersUiResponse(AnswersUiResponse {
                  id: ask.id.clone(),
                  answers,
                }),
                cx,
              );
            }
          }),
      )
      .child(cancel),
  )
}
