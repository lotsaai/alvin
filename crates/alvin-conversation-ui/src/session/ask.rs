//! The `ask` wizard: one omp question per step, then a review of every answer before anything is sent.
//!
//! Picks are kept by option position, so the state never depends on labels being distinct. The answer sent back names
//! options by label, as omp requires. omp itself rejects an `ask` with repeated question ids or option labels before it
//! reaches a client.

use alvin_protocol::{AnswersUiResponse, AskAnswer, AskQuestion, AskUiRequest, ExtensionUiResponse};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::radio::{Radio, RadioGroup};
use gpui_kit::component::tag::Tag;
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
  AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
  Subscription, Window, div, rems,
};

use super::dialogs::Reply;

/// What omp appends to the label of the option it recommends.
const RECOMMENDED: &str = " (Recommended)";

/// Opens the wizard for an `ask` request.
pub(crate) fn open(reply: Reply, ask: AskUiRequest, window: &mut Window, cx: &mut App) {
  let title = SharedString::from(if ask.questions.len() == 1 {
    "omp has a question"
  } else {
    "omp has questions"
  });
  let id = ask.id.clone();
  let body = cx.new(|cx| Wizard::new(reply.clone(), ask, window, cx));
  window.open_dialog(cx, move |dialog, _, _| {
    let (reply, id) = (reply.clone(), id.clone());
    dialog.title(title.clone()).child(body.clone()).on_cancel(move |_, _, cx| {
      reply.cancel(&id, cx);
      true
    })
  });
}

/// The wizard's state: the step on screen, and what the user chose and typed for each question.
struct Wizard {
  reply: Reply,
  ask: AskUiRequest,
  /// The question on screen, or one past the last question for the review.
  step: usize,
  /// Option positions picked for each question, in the order they were picked.
  picks: Vec<Vec<usize>>,
  /// The free-text box of each question.
  custom: Vec<Entity<InputState>>,
  _subscriptions: Vec<Subscription>,
}

impl Wizard {
  fn new(reply: Reply, ask: AskUiRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
    let mut picks = Vec::new();
    let mut custom = Vec::new();
    let mut subscriptions = Vec::new();
    for (index, question) in ask.questions.iter().enumerate() {
      let input = cx.new(|cx| InputState::new(window, cx).placeholder("Other (type your own)"));
      let multi = question.multi;
      // A single-select answer is an option or text, never both: typing drops the option.
      subscriptions.push(cx.subscribe(&input, move |this, input, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change)
          && !multi
          && !input.read(cx).value().trim().is_empty()
          && let Some(picked) = this.picks.get_mut(index)
        {
          picked.clear();
          cx.notify();
        }
      }));
      let recommended = question
        .recommended
        .and_then(|position| usize::try_from(position).ok())
        .filter(|position| *position < question.options.len());
      picks.push(recommended.into_iter().collect());
      custom.push(input);
    }
    Self {
      reply,
      ask,
      step: 0,
      picks,
      custom,
      _subscriptions: subscriptions,
    }
  }

  /// A review is worth a step only when there is more than one answer to look over.
  fn has_review(&self) -> bool {
    self.ask.questions.len() > 1
  }

  fn reviewing(&self) -> bool {
    self.has_review() && self.step >= self.ask.questions.len()
  }

  /// Whether the question on screen is the last one, so the next step is the review or the send.
  fn on_last_question(&self) -> bool {
    self.step + 1 >= self.ask.questions.len()
  }

  fn answer(&self, index: usize, cx: &App) -> Option<AskAnswer> {
    let question = self.ask.questions.get(index)?;
    let picked = self.picks.get(index)?;
    let text = self.custom.get(index)?.read(cx).value();
    answer(question, picked, &text)
  }

  /// Every answer, once each question has one.
  fn answers(&self, cx: &App) -> Option<Vec<AskAnswer>> {
    (0..self.ask.questions.len()).map(|index| self.answer(index, cx)).collect()
  }

  /// Picks an option; a single-select question also clears what was typed, since omp takes one or the other.
  fn pick(&mut self, index: usize, position: usize, window: &mut Window, cx: &mut Context<Self>) {
    let Some(question) = self.ask.questions.get(index) else {
      return;
    };
    let multi = question.multi;
    if let Some(picked) = self.picks.get_mut(index) {
      toggle(picked, position, multi);
    }
    if !multi && let Some(input) = self.custom.get(index) {
      input.update(cx, |input, cx| input.set_value("", window, cx));
    }
    cx.notify();
  }

  fn question_view(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
    let (Some(question), Some(picked)) = (self.ask.questions.get(index), self.picks.get(index)) else {
      return div().into_any_element();
    };
    let theme = cx.theme();
    let (muted, border, radius) = (theme.muted_foreground, theme.border, theme.radius);
    let key = format!("ask-{}-{}", self.ask.id, question.id);
    let description = move |text: &Option<String>| {
      text
        .as_ref()
        .map(|text| div().text_sm().text_color(muted).child(SharedString::from(text.clone())))
    };

    let options: AnyElement = if question.multi {
      v_flex()
        .gap_2()
        .children(question.options.iter().enumerate().map(|(position, option)| {
          v_flex()
            .child(
              Checkbox::new(SharedString::from(format!("{key}-{}", option.label)))
                .label(SharedString::from(shown_label(question, position)))
                .checked(picked.contains(&position))
                .on_change(cx.listener(move |this, _: &bool, window, cx| this.pick(index, position, window, cx))),
            )
            .children(description(&option.description).map(|text| text.pl(rems(1.75))))
        }))
        .into_any_element()
    } else {
      let mut group = RadioGroup::vertical(SharedString::from(key.clone()));
      for (position, option) in question.options.iter().enumerate() {
        group = group.child(
          Radio::new(SharedString::from(format!("{key}-{}", option.label)))
            .label(SharedString::from(shown_label(question, position)))
            .children(description(&option.description)),
        );
      }
      group
        .selected_index(picked.first().copied())
        .on_change(cx.listener(move |this, position: &usize, window, cx| this.pick(index, *position, window, cx)))
        .into_any_element()
    };

    let previews = picked.iter().filter_map(|position| {
      let option = question.options.get(*position)?;
      let preview = option.preview.clone()?;
      Some(
        div()
          .p_2()
          .border_1()
          .border_color(border)
          .rounded(radius)
          .child(TextView::markdown(
            SharedString::from(format!("{key}-preview-{}", option.label)),
            preview,
          )),
      )
    });

    v_flex()
      .gap_3()
      .children(self.has_review().then(|| {
        div().text_sm().text_color(muted).child(SharedString::from(format!(
          "Question {} of {}",
          index + 1,
          self.ask.questions.len()
        )))
      }))
      .children(
        question
          .header
          .as_ref()
          .map(|header| Tag::secondary().child(SharedString::from(header.clone()))),
      )
      .child(
        div()
          .font_weight(FontWeight::SEMIBOLD)
          .child(SharedString::from(question.question.clone())),
      )
      .child(options)
      .children(previews)
      .children(
        self
          .custom
          .get(index)
          .map(|input| Input::new(input).aria_label("Other (type your own)")),
      )
      .into_any_element()
  }

  fn review_view(&self, cx: &mut Context<Self>) -> AnyElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
      .gap_3()
      .child(div().font_weight(FontWeight::SEMIBOLD).child("Review your answers"))
      .children(self.ask.questions.iter().enumerate().map(|(index, question)| {
        let text = self.answer(index, cx).map(|answer| summary(&answer)).unwrap_or_default();
        h_flex()
          .gap_3()
          .items_start()
          .justify_between()
          .child(
            v_flex()
              .flex_1()
              .min_w_0()
              .child(SharedString::from(question.question.clone()))
              .child(div().text_sm().text_color(muted).child(SharedString::from(text))),
          )
          .child(
            Button::new(SharedString::from(format!("ask-{}-edit-{}", self.ask.id, question.id)))
              .ghost()
              .small()
              .label("Edit")
              .on_click(cx.listener(move |this, _, _, cx| {
                this.step = index;
                cx.notify();
              })),
          )
      }))
      .into_any_element()
  }
}

impl Render for Wizard {
  fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let id = self.ask.id.clone();
    let reviewing = self.reviewing();
    let sending = reviewing || (self.on_last_question() && !self.has_review());
    let step_answered = self.answer(self.step, cx).is_some();
    let all_answered = self.answers(cx).is_some();
    let body = if reviewing {
      self.review_view(cx)
    } else {
      self.question_view(self.step, cx)
    };
    let footer = DialogFooter::new()
      .child(
        Button::new(SharedString::from(format!("ask-{id}-cancel")))
          .outline()
          .label("Cancel")
          .on_click(cx.listener(|this, _, window, cx| {
            this.reply.cancel(&this.ask.id, cx);
            window.close_dialog(cx);
          })),
      )
      .children((self.step > 0).then(|| {
        Button::new(SharedString::from(format!("ask-{id}-back")))
          .outline()
          .label("Back")
          .on_click(cx.listener(|this, _, _, cx| {
            this.step = this.step.saturating_sub(1);
            cx.notify();
          }))
      }))
      .children((!sending).then(|| {
        Button::new(SharedString::from(format!("ask-{id}-next")))
          .primary()
          .label("Next")
          .disabled(!step_answered)
          .on_click(cx.listener(|this, _, _, cx| {
            this.step = this.step.saturating_add(1);
            cx.notify();
          }))
      }))
      .children(sending.then(|| {
        Button::new(SharedString::from(format!("ask-{id}-submit")))
          .primary()
          .label("Send answers")
          .disabled(!all_answered)
          .on_click(cx.listener(|this, _, window, cx| {
            if let Some(answers) = this.answers(cx) {
              let response = ExtensionUiResponse::AnswersUiResponse(AnswersUiResponse {
                id: this.ask.id.clone(),
                answers,
              });
              this.reply.send(response, cx);
              window.close_dialog(cx);
            }
          }))
      }));
    v_flex().gap_4().child(body).child(footer)
  }
}

/// The label omp shows for an option: the recommended one is marked.
fn shown_label(question: &AskQuestion, position: usize) -> String {
  let label = question
    .options
    .get(position)
    .map(|option| option.label.as_str())
    .unwrap_or_default();
  let recommended = question.recommended.and_then(|index| usize::try_from(index).ok()) == Some(position);
  if recommended {
    format!("{label}{RECOMMENDED}")
  } else {
    label.to_owned()
  }
}

/// Picks or unpicks an option: a single-select question holds the one picked, a multi-select one toggles it.
fn toggle(picked: &mut Vec<usize>, position: usize, multi: bool) {
  if !multi {
    *picked = vec![position];
  } else if let Some(at) = picked.iter().position(|picked| *picked == position) {
    picked.remove(at);
  } else {
    picked.push(position);
  }
}

/// The answer to a question, once there is one. Options go by label, once each. A single-select answer is options or
/// text, which is what omp accepts; a multi-select one can carry both.
fn answer(question: &AskQuestion, picked: &[usize], text: &str) -> Option<AskAnswer> {
  let mut selected_options: Vec<String> = Vec::new();
  for option in picked.iter().filter_map(|position| question.options.get(*position)) {
    if !selected_options.contains(&option.label) {
      selected_options.push(option.label.clone());
    }
  }
  let text = text.trim();
  let custom_input = (!text.is_empty() && (question.multi || selected_options.is_empty())).then(|| text.to_owned());
  (!selected_options.is_empty() || custom_input.is_some()).then(|| AskAnswer {
    id: question.id.clone(),
    selected_options,
    custom_input,
  })
}

/// An answer as one line for the review.
fn summary(answer: &AskAnswer) -> String {
  answer
    .selected_options
    .iter()
    .cloned()
    .chain(answer.custom_input.clone())
    .collect::<Vec<_>>()
    .join(", ")
}

#[cfg(test)]
mod tests {
  use alvin_protocol::AskQuestion;

  use super::{answer, shown_label, summary, toggle};

  fn question(multi: bool, labels: &[&str]) -> AskQuestion {
    serde_json::from_value(serde_json::json!({
      "id": "q",
      "question": "Which?",
      "multi": multi,
      "recommended": 1,
      "options": labels.iter().map(|label| serde_json::json!({ "label": label })).collect::<Vec<_>>(),
    }))
    .unwrap()
  }

  #[test]
  fn a_single_select_answer_is_an_option_or_text_never_both() {
    let single = question(false, &["A", "B"]);
    let picked = answer(&single, &[1], "typed").unwrap();
    assert_eq!((picked.selected_options, picked.custom_input), (vec!["B".to_owned()], None));
    let typed = answer(&single, &[], "  mine ").unwrap();
    assert_eq!((typed.selected_options, typed.custom_input), (vec![], Some("mine".to_owned())));
  }

  #[test]
  fn a_multi_select_answer_keeps_every_option_and_the_text() {
    let multi = question(true, &["A", "B", "C"]);
    let both = answer(&multi, &[2, 0], "also this").unwrap();
    assert_eq!(both.selected_options, ["C", "A"]);
    assert_eq!(both.custom_input.as_deref(), Some("also this"));
  }

  #[test]
  fn nothing_chosen_or_typed_is_no_answer() {
    assert!(answer(&question(true, &["A"]), &[], "  ").is_none());
    assert!(answer(&question(false, &["A"]), &[], "").is_none());
  }

  #[test]
  fn equal_labels_are_sent_once() {
    let twin = question(true, &["Same", "Same"]);
    assert_eq!(answer(&twin, &[0, 1], "").unwrap().selected_options, ["Same"]);
  }

  #[test]
  fn a_single_select_pick_replaces_and_a_multi_select_pick_toggles() {
    let mut single = vec![0];
    toggle(&mut single, 2, false);
    assert_eq!(single, [2]);
    let mut multi = vec![0];
    toggle(&mut multi, 2, true);
    toggle(&mut multi, 0, true);
    assert_eq!(multi, [2]);
  }

  #[test]
  fn the_recommended_option_is_marked_and_the_review_lists_options_then_text() {
    let multi = question(true, &["A", "B"]);
    assert_eq!(shown_label(&multi, 0), "A");
    assert_eq!(shown_label(&multi, 1), "B (Recommended)");
    assert_eq!(summary(&answer(&multi, &[0, 1], "x").unwrap()), "A, B, x");
  }
}

#[cfg(test)]
mod window_tests {
  use std::time::Duration;

  use alvin_protocol::{Event, EventId};
  use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
  use gpui_kit::{AnyWindowHandle, AppContext as _, TestAppContext};

  use crate::test_support::{Opened, live_session, open_workspace};

  const BUDGET: Duration = Duration::from_secs(1);

  /// An `ask` of two questions: a single-select one that recommends its first option, and a multi-select one.
  fn ask_event(live: alvin_protocol::LiveSessionId) -> Event {
    serde_json::from_value(serde_json::json!({
      "id": EventId::new(),
      "live": live,
      "at": "2026-11-01T00:00:00Z",
      "body": { "kind": "extensionUiRequest", "frame": {
        "type": "extension_ui_request",
        "method": "ask",
        "id": "ask1",
        "questions": [
          { "id": "q1", "question": "Database?", "options": [{ "label": "SQLite" }, { "label": "Postgres" }], "multi": false, "recommended": 0 },
          { "id": "q2", "question": "Tools?", "options": [{ "label": "Editor" }, { "label": "Terminal" }], "multi": true }
        ]
      } }
    }))
    .unwrap()
  }

  /// Answering sends a `live/respond` call over the tokio client, whose completion wakes a GPUI task from a tokio thread, so
  /// the scheduler has to allow parking (the documented pattern for external I/O).
  async fn asked(cx: &mut TestAppContext) -> (Opened, AnyWindowHandle) {
    cx.executor().allow_parking();
    let opened = open_workspace(cx).await;
    let session = live_session("/work/api");
    cx.update(|cx| {
      opened.model.update(cx, |model, cx| {
        model.seed_open(&session, cx);
        model.seed_event(&ask_event(session.live), cx);
      });
    });
    let handle = opened.handle;
    cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-next").is_some())
      .await;
    (opened, handle)
  }

  fn click(cx: &mut TestAppContext, handle: AnyWindowHandle, id: &'static str) {
    cx.update_window(handle, |_, window, cx| window.click(id, cx)).unwrap();
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn the_wizard_steps_through_the_questions_reviews_and_sends(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (_opened, handle) = asked(cx).await;
      cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("ask-ask1-back").is_none(), "the first step has no Back")
      })
      .unwrap();

      // The recommended option is preselected, so the first question can go on.
      click(cx, handle, "ask-ask1-next");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-back").is_some())
        .await;

      // The multi-select question has no answer yet; choosing an option lets it go on to the review.
      click(cx, handle, "ask-ask1-q2-Terminal");
      click(cx, handle, "ask-ask1-next");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-submit").is_some())
        .await;

      // Edit takes the user back to that question; going on again returns to the review.
      click(cx, handle, "ask-ask1-edit-q1");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-submit").is_none())
        .await;
      click(cx, handle, "ask-ask1-next");
      click(cx, handle, "ask-ask1-next");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-submit").is_some())
        .await;

      // Sending closes the dialog.
      click(cx, handle, "ask-ask1-submit");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-submit").is_none())
        .await;
    });
  }

  #[tokio::test(flavor = "multi_thread")]
  #[gpui_kit::test]
  async fn cancel_closes_the_wizard(cx: &mut TestAppContext) {
    let executor = cx.foreground_executor().clone();
    executor.block_test(async {
      let (_opened, handle) = asked(cx).await;
      click(cx, handle, "ask-ask1-cancel");
      cx.wait_for(handle, BUDGET, |window, _| window.try_find("ask-ask1-next").is_none())
        .await;
    });
  }
}
