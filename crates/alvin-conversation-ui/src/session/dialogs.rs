//! The dialog an extension tool is waiting on: `select`, `confirm`, `input`, `editor` and `ask`.
//!
//! omp can have several open; a session shows the oldest and the rest wait their turn. What the user is choosing or
//! typing lives in the dialog's own body entity; whether a dialog is open is the model's.

use std::cell::Cell;
use std::rc::Rc;

use alvin_conversation::request_id;
use alvin_protocol::{
  CancelUiResponse, ConfirmUiResponse, ExtensionUiRequest, ExtensionUiResponse, LitTrue, LiveSessionId, ValueUiResponse,
};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputContentType, InputState, Textarea, TextareaState};
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::{Disableable as _, WindowExt as _, v_flex};
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window};

use crate::model::Model;

/// Which dialog is on screen for a session, and whether the user already answered it.
pub(crate) struct Dialogs {
  shown: Option<String>,
  answered: Rc<Cell<bool>>,
}

impl Dialogs {
  pub(crate) fn new() -> Self {
    Self {
      shown: None,
      answered: Rc::new(Cell::new(false)),
    }
  }

  /// Opens the oldest pending dialog when it is not the one on screen, and closes the one on screen when omp withdrew it.
  pub(crate) fn sync(&mut self, model: &Entity<Model>, live: LiveSessionId, window: &mut Window, cx: &mut App) {
    let current = model
      .read(cx)
      .conversation()
      .live(live)
      .and_then(|view| view.dialogs.pending().first().cloned());
    let current_id = current.as_ref().map(|request| request_id(request).to_owned());
    if current_id == self.shown {
      return;
    }
    if self.shown.is_some() && !self.answered.get() {
      window.close_dialog(cx);
    }
    self.shown = current_id;
    self.answered = Rc::new(Cell::new(false));
    if let Some(request) = current {
      open(model, live, request, self.answered.clone(), window, cx);
    }
  }
}

/// What a dialog sends back, and the bookkeeping around sending it.
#[derive(Clone)]
pub(crate) struct Reply {
  model: Entity<Model>,
  live: LiveSessionId,
  answered: Rc<Cell<bool>>,
}

impl Reply {
  pub(crate) fn send(&self, response: ExtensionUiResponse, cx: &mut App) {
    self.answered.set(true);
    self.model.update(cx, |model, cx| model.respond(self.live, response, cx)).detach();
  }

  pub(crate) fn cancel(&self, id: &str, cx: &mut App) {
    self.send(
      ExtensionUiResponse::CancelUiResponse(CancelUiResponse {
        id: id.to_owned(),
        cancelled: LitTrue,
        timed_out: None,
      }),
      cx,
    );
  }
}

fn open(
  model: &Entity<Model>,
  live: LiveSessionId,
  request: ExtensionUiRequest,
  answered: Rc<Cell<bool>>,
  window: &mut Window,
  cx: &mut App,
) {
  let reply = Reply {
    model: model.clone(),
    live,
    answered,
  };
  match request {
    ExtensionUiRequest::Confirm(confirm) => {
      window.open_alert_dialog(cx, move |alert, _, _| {
        let (ok, no) = (reply.clone(), reply.clone());
        let (yes_id, no_id) = (confirm.id.clone(), confirm.id.clone());
        alert
          .title(SharedString::from(confirm.title.clone()))
          .description(SharedString::from(confirm.message.clone()))
          .confirm()
          .ok_text("Yes")
          .cancel_text("No")
          .on_ok(move |_, _, cx| {
            ok.send(
              ExtensionUiResponse::ConfirmUiResponse(ConfirmUiResponse {
                id: yes_id.clone(),
                confirmed: true,
              }),
              cx,
            );
            true
          })
          .on_cancel(move |_, _, cx| {
            no.send(
              ExtensionUiResponse::ConfirmUiResponse(ConfirmUiResponse {
                id: no_id.clone(),
                confirmed: false,
              }),
              cx,
            );
            true
          })
      });
    }
    ExtensionUiRequest::Select(_) | ExtensionUiRequest::Input(_) | ExtensionUiRequest::Editor(_) => {
      let title = SharedString::from(title(&request));
      let id = request_id(&request).to_owned();
      let body = cx.new(|cx| Body::new(reply.clone(), request, window, cx));
      window.open_dialog(cx, move |dialog, _, _| {
        let (reply, id) = (reply.clone(), id.clone());
        dialog.title(title.clone()).child(body.clone()).on_cancel(move |_, _, cx| {
          reply.cancel(&id, cx);
          true
        })
      });
    }
    ExtensionUiRequest::Ask(ask) => super::ask::open(reply, ask, window, cx),
    ExtensionUiRequest::Cancel(_)
    | ExtensionUiRequest::Notify(_)
    | ExtensionUiRequest::SetStatus(_)
    | ExtensionUiRequest::SetWidget(_)
    | ExtensionUiRequest::SetTitle(_)
    | ExtensionUiRequest::SetEditorText(_)
    | ExtensionUiRequest::OpenUrl(_) => {}
  }
}

/// The dialog's title: what the extension asked, or the first question's header.
fn title(request: &ExtensionUiRequest) -> String {
  match request {
    ExtensionUiRequest::Select(select) => select.title.clone(),
    ExtensionUiRequest::Input(input) => input.title.clone(),
    ExtensionUiRequest::Editor(editor) => editor.title.clone(),
    _ => String::new(),
  }
}

/// The body of a `select`, `input` or `editor` dialog: what to choose or type, and the buttons that send it.
struct Body {
  reply: Reply,
  request: ExtensionUiRequest,
  /// The option picked in a `select`.
  selected: Option<usize>,
  /// The text of an `input`.
  line: Entity<InputState>,
  /// The text of an `editor`.
  block: Entity<TextareaState>,
}

impl Body {
  fn new(reply: Reply, request: ExtensionUiRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let placeholder = match &request {
      ExtensionUiRequest::Input(input) => input.placeholder.clone().unwrap_or_default(),
      _ => String::new(),
    };
    let prefill = match &request {
      ExtensionUiRequest::Editor(editor) => editor.prefill.clone().unwrap_or_default(),
      _ => String::new(),
    };
    let line = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let block = cx.new(|cx| {
      let mut state = TextareaState::new(window, cx).auto_grow(3, 12);
      state.set_value(prefill, window, cx);
      state
    });
    Self {
      reply,
      request,
      selected: None,
      line,
      block,
    }
  }

  /// What the user's choices say, ready to send, once they are complete.
  fn response(&self, cx: &App) -> Option<ExtensionUiResponse> {
    match &self.request {
      ExtensionUiRequest::Select(select) => {
        let value = select.options.get(self.selected?)?.clone();
        Some(ExtensionUiResponse::ValueUiResponse(ValueUiResponse {
          id: select.id.clone(),
          value,
        }))
      }
      ExtensionUiRequest::Input(input) => Some(ExtensionUiResponse::ValueUiResponse(ValueUiResponse {
        id: input.id.clone(),
        value: self.line.read(cx).value().to_string(),
      })),
      ExtensionUiRequest::Editor(editor) => Some(ExtensionUiResponse::ValueUiResponse(ValueUiResponse {
        id: editor.id.clone(),
        value: self.block.read(cx).value().to_string(),
      })),
      _ => None,
    }
  }

  fn fields(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
    match &self.request {
      ExtensionUiRequest::Select(select) => RadioGroup::new(SharedString::from(format!("select-{}", select.id)))
        .children(select.options.iter().map(|option| SharedString::from(option.clone())))
        .selected_index(self.selected)
        .on_change(cx.listener(|this, index: &usize, _, cx| {
          this.selected = Some(*index);
          cx.notify();
        }))
        .into_any_element(),
      ExtensionUiRequest::Input(input) => {
        let line = Input::new(&self.line).aria_label(SharedString::from(input.title.clone()));
        match input.secret {
          Some(true) => line.content_type(InputContentType::Password).mask_toggle().into_any_element(),
          _ => line.into_any_element(),
        }
      }
      ExtensionUiRequest::Editor(editor) => Textarea::new(&self.block)
        .aria_label(SharedString::from(editor.title.clone()))
        .into_any_element(),
      _ => v_flex().into_any_element(),
    }
  }
}

impl Render for Body {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let id = request_id(&self.request).to_owned();
    let response = self.response(cx);
    let submit = match &self.request {
      ExtensionUiRequest::Select(_) => "Select",
      _ => "Submit",
    };
    v_flex().gap_4().child(self.fields(cx)).child(
      DialogFooter::new()
        .child(
          Button::new(SharedString::from(format!("dialog-cancel-{id}")))
            .outline()
            .label("Cancel")
            .on_click(cx.listener({
              let id = id.clone();
              move |this, _, window, cx| {
                this.reply.cancel(&id, cx);
                window.close_dialog(cx);
              }
            })),
        )
        .child(
          Button::new(SharedString::from(format!("dialog-submit-{id}")))
            .primary()
            .label(submit)
            .disabled(response.is_none())
            .on_click(cx.listener(move |this, _, window, cx| {
              if let Some(response) = this.response(cx) {
                this.reply.send(response, cx);
                window.close_dialog(cx);
              }
            })),
        ),
    )
  }
}
