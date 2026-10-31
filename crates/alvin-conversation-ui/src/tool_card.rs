//! One card per tool call, keyed by omp's tool call id.
//!
//! Tool arguments and results are open JSON in omp's schema, so a tool this crate does not know is the normal case: the
//! card shows its name, what the model said it was doing, and its arguments and result as formatted JSON.

use alvin_conversation::ToolExecution;
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::{
  App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, SharedString, Styled as _, div, prelude::FluentBuilder as _, px,
};
use serde_json::Value;

/// What a card shows. Built from a tool call in an assistant message, and from the execution tracked while it runs.
pub(crate) struct ToolCard {
  pub(crate) id: String,
  pub(crate) name: String,
  pub(crate) intent: Option<String>,
  pub(crate) args: Option<Value>,
  pub(crate) result: Option<Value>,
  pub(crate) running: bool,
  pub(crate) failed: bool,
}

impl ToolCard {
  /// The card of a tool call: its arguments from the message, and its progress when an execution is being tracked.
  pub(crate) fn of_call(id: &str, name: &str, intent: Option<&str>, args: Option<Value>, execution: Option<&ToolExecution>) -> Self {
    Self {
      id: id.to_owned(),
      name: name.to_owned(),
      intent: intent.map(str::to_owned),
      args: execution.and_then(|execution| execution.args.clone()).or(args),
      result: execution.and_then(|execution| {
        execution
          .outcome
          .as_ref()
          .and_then(|outcome| outcome.result.clone())
          .or_else(|| execution.partial.clone())
      }),
      running: execution.is_some_and(|execution| execution.outcome.is_none()),
      failed: execution.is_some_and(|execution| execution.outcome.as_ref().is_some_and(|outcome| outcome.is_error)),
    }
  }

  pub(crate) fn render(self, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let status = if self.running {
      "running"
    } else if self.failed {
      "failed"
    } else {
      ""
    };
    v_flex()
      .id(ElementId::from(SharedString::from(format!("tool-{}", self.id))))
      .gap_1()
      .p_2()
      .border_1()
      .border_color(theme.border)
      .rounded(theme.radius)
      .child(
        h_flex()
          .gap_2()
          .child(
            div()
              .font_weight(gpui_kit::FontWeight::SEMIBOLD)
              .child(SharedString::from(self.name)),
          )
          .when_some(self.intent, |this, intent| {
            this.child(div().text_color(theme.muted_foreground).child(SharedString::from(intent)))
          })
          .when(!status.is_empty(), |this| {
            this.child(div().text_color(theme.muted_foreground).child(status))
          }),
      )
      .when_some(self.args, |this, args| this.child(json_block(&args, cx)))
      .when_some(self.result, |this, result| this.child(json_block(&result, cx)))
  }
}

/// Formatted JSON in a monospaced block.
fn json_block(value: &Value, cx: &App) -> impl IntoElement {
  let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
  div()
    .p_1()
    .max_h(px(240.))
    .overflow_hidden()
    .bg(cx.theme().muted)
    .rounded(cx.theme().radius)
    .font_family(cx.theme().mono_font_family.clone())
    .text_sm()
    .child(SharedString::from(text))
}
