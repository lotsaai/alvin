//! One retained markdown editor, compact for chat or expanded for long drafts.
//!
//! What the user types goes to omp unchanged, except a leading `!`, which runs the rest as a shell command. omp expands `/`
//! commands, skills, templates and `^model` itself; nothing here parses them. The text is edited as markdown.

mod history;

use std::ops::Range as ByteRange;
use std::rc::Rc;

use alvin_protocol::{AvailableSlashCommand, LiveSessionId};
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
#[cfg(target_os = "macos")]
use gpui_kit::component::input::Paste;
use gpui_kit::component::input::{
  CompletionProvider, Delete, DeleteToBeginningOfLine, DeleteToEndOfLine, DeleteToPreviousWordStart, Editor, EditorState, Enter, Escape,
  InputEvent, MoveDown, MoveUp, OutdentInline, Rope,
};
#[cfg(not(target_os = "macos"))]
use gpui_kit::component::input::{MoveEnd, MoveHome, MoveToNextWord, MoveToPreviousWord};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::speech::SpeechWaveform;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
  Anchor, AnyElement, App, AppContext as _, ClipboardItem, Context, ElementId, Entity, EntityInputHandler as _, InteractiveElement as _,
  IntoElement, KeyBinding, ParentElement as _, Pixels, Render, Result, SharedString, StatefulInteractiveElement as _, Styled as _,
  Subscription, Task, Window, actions, div, relative, rems,
};
use lsp_types::{CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Position, Range, TextEdit};

use super::dictation::{Dictation, Phase};
use super::{markdown, pickers::Pickers};
use crate::model::{Model, ModelChange, ModelEvent};

/// How many rows the composer shows at least, and at most before its text scrolls.
const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 8;

/// An editor leads its rows at this multiple of its font size.
const LINE_HEIGHT: f32 = 1.5;

actions!(
  composer,
  [ClearPrompt, CopyLine, CopyPrompt, SearchHistory, CycleThinking, OpenModel]
);

pub(crate) fn init(cx: &mut App) {
  let context = Some("Composer > Input");
  cx.bind_keys([
    KeyBinding::new("ctrl-w", DeleteToPreviousWordStart, context),
    KeyBinding::new("ctrl-u", DeleteToBeginningOfLine, context),
    KeyBinding::new("ctrl-k", DeleteToEndOfLine, context),
    KeyBinding::new(
      "alt-enter",
      Enter {
        secondary: false,
        shift: true,
      },
      context,
    ),
    KeyBinding::new("ctrl-c", ClearPrompt, context),
    KeyBinding::new("ctrl-d", Delete, context),
    KeyBinding::new("alt-shift-l", CopyLine, context),
    KeyBinding::new("alt-shift-c", CopyPrompt, context),
    KeyBinding::new("ctrl-r", SearchHistory, context),
    #[cfg(target_os = "macos")]
    KeyBinding::new("ctrl-v", Paste, context),
    KeyBinding::new("shift-tab", CycleThinking, context),
    KeyBinding::new("alt-p", OpenModel, context),
    #[cfg(not(target_os = "macos"))]
    KeyBinding::new("alt-backspace", DeleteToPreviousWordStart, context),
    #[cfg(not(target_os = "macos"))]
    KeyBinding::new("alt-left", MoveToPreviousWord, context),
    #[cfg(not(target_os = "macos"))]
    KeyBinding::new("alt-right", MoveToNextWord, context),
    #[cfg(not(target_os = "macos"))]
    KeyBinding::new("ctrl-a", MoveHome, context),
    #[cfg(not(target_os = "macos"))]
    KeyBinding::new("ctrl-e", MoveEnd, context),
  ]);
}

/// The editor's local layout. Changing it never replaces its editing state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Layout {
  #[default]
  Compact,
  Expanded,
}

#[derive(Default)]
pub(super) struct DictationEdits {
  pending: String,
  inserted: bool,
  selection: Option<(ByteRange<usize>, bool, usize)>,
}

impl DictationEdits {
  pub(super) fn push(&mut self, text: &str) {
    self.pending.push_str(text);
  }

  pub(super) fn flush(&mut self, editor: &mut EditorState, window: &mut Window, cx: &mut Context<EditorState>) {
    if self.pending.is_empty() || editor.marked_text_range(window, cx).is_some() {
      return;
    }
    let text = std::mem::take(&mut self.pending);
    let selection = editor.selected_range();
    if !self.inserted || selection.is_empty() {
      editor.replace(text, window, cx);
      self.selection = None;
    } else {
      let reversed = editor.is_selection_reversed();
      let offset = match &self.selection {
        Some((previous, direction, offset)) if *previous == selection && *direction == reversed => *offset,
        _ => selection.end,
      };
      editor.set_selected_range(offset..offset, cx);
      editor.replace(text, window, cx);
      let end = editor.selected_range().end;
      editor.set_selected_range_with_direction(selection.clone(), reversed, cx);
      self.selection = Some((selection, reversed, end));
    }
    self.inserted = true;
  }
}

/// The composer's editor: soft-wrapped markdown without a gutter, where Enter submits and a leading `/` offers the session's
/// slash commands.
pub(crate) fn state<T: 'static>(
  model: &Entity<Model>,
  live: LiveSessionId,
  window: &mut Window,
  cx: &mut Context<T>,
) -> Entity<EditorState> {
  let commands = Rc::new(SlashCommands {
    model: model.clone(),
    live,
  });
  cx.new(|cx| {
    let mut state = EditorState::new(window, cx)
      .language(markdown::LANGUAGE)
      .line_number(false)
      .folding(false)
      .placeholder("Message omp, or !command")
      .submit_on_enter(true);
    state.set_highlighter_factory(markdown::factory(), cx);
    state.lsp_mut().completion_provider = Some(commands);
    state
  })
}

/// Offers omp's slash commands for the word at the start of the text. omp expands them; this only completes the name.
struct SlashCommands {
  model: Entity<Model>,
  live: LiveSessionId,
}

impl CompletionProvider for SlashCommands {
  fn completions(
    &self,
    text: &Rope,
    offset: usize,
    _: CompletionContext,
    _: &mut Window,
    cx: &mut App,
  ) -> Task<Result<CompletionResponse>> {
    let items = self
      .model
      .read(cx)
      .conversation()
      .live(self.live)
      .map_or_else(Vec::new, |view| command_items(&view.commands, &text.to_string(), offset));
    Task::ready(Ok(CompletionResponse::Array(items)))
  }

  /// Any typed word: the menu refines as the name grows, and `completions` has nothing to offer outside a leading `/word`.
  fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
    !new_text.is_empty() && !new_text.contains(char::is_whitespace)
  }
}

/// The commands whose name starts with the `/word` before `offset`, as edits replacing that word. Nothing when the caret is
/// not at the end of a first word that starts with `/`.
fn command_items(commands: &[AvailableSlashCommand], text: &str, offset: usize) -> Vec<CompletionItem> {
  let Some(typed) = text.get(..offset).and_then(|before| before.strip_prefix('/')) else {
    return Vec::new();
  };
  if typed.contains(char::is_whitespace) {
    return Vec::new();
  }
  let end = Position::new(0, text[..offset].encode_utf16().count() as u32);
  commands
    .iter()
    .filter(|command| command.name.starts_with(typed))
    .map(|command| CompletionItem {
      label: format!("/{}", command.name),
      detail: command.description.clone(),
      text_edit: Some(CompletionTextEdit::Edit(TextEdit {
        range: Range::new(Position::new(0, 0), end),
        new_text: format!("/{} ", command.name),
      })),
      ..Default::default()
    })
    .collect()
}

/// What the user sent: the text, and whether they sent it with the secondary key (Cmd or Ctrl), which queues it as a
/// follow-up instead of steering the running turn.
pub(crate) struct Submission {
  pub(crate) text: String,
  pub(crate) follow_up: bool,
}

/// Expanded editing sends only with the secondary key. Shift+Enter never sends.
pub(crate) fn submitted(event: &InputEvent, state: &Entity<EditorState>, layout: Layout, cx: &App) -> Option<Submission> {
  match event {
    InputEvent::PressEnter { shift: false, secondary } if layout == Layout::Compact || *secondary => {
      let text = state.read(cx).value();
      let text = text.trim_end();
      (!text.trim().is_empty()).then(|| Submission {
        text: text.to_owned(),
        follow_up: *secondary,
      })
    }
    _ => None,
  }
}

/// The same editor grows to the content viewport or follows its wrapped rows in compact mode.
pub(crate) fn view(state: &Entity<EditorState>, layout: Layout, window: &Window, cx: &App) -> Editor {
  let editor = Editor::new(state);
  let editor = if layout == Layout::Expanded {
    editor.h(relative(1.))
  } else {
    let rows = state.read(cx).display_map().wrap_row_count().clamp(MIN_ROWS, MAX_ROWS);
    let row: Pixels = cx.theme().mono_font_size * LINE_HEIGHT;
    // Match the native editor's vertical padding.
    let padding = rems(1.).to_pixels(window.rem_size());
    editor.h(row * rows as f32 + padding)
  };
  editor.appearance(false).bordered(false).aria_label("Message")
}

pub(crate) struct Composer {
  live: LiveSessionId,
  model: Entity<Model>,
  composer: Entity<EditorState>,
  dictation: Entity<Dictation>,
  composer_layout: Layout,
  pickers: Pickers,
  history: Option<history::Browse>,
  recalling_history: bool,
  _subscriptions: Vec<Subscription>,
}

impl Composer {
  pub(crate) fn new(
    model: Entity<Model>,
    live: LiveSessionId,
    dictation: Entity<Dictation>,
    window: &mut Window,
    cx: &mut Context<Self>,
  ) -> Self {
    let composer = state(&model, live, window, cx);
    let mut pickers = Pickers::new(&model, live, window, cx);
    pickers.sync(&model, live, window, cx);
    let subscriptions = vec![
      cx.observe(&dictation, |_, _, cx| cx.notify()),
      cx.subscribe_in(&model, window, |this, model, change: &ModelChange, window, cx| {
        if matches!(change, ModelChange::Session(id) if *id == this.live) {
          this.pickers.sync(model, this.live, window, cx);
          cx.notify();
        }
      }),
      cx.subscribe_in(&model, window, |this, _, event: &ModelEvent, window, cx| {
        if let ModelEvent::ComposerText { live, text } = event
          && *live == this.live
        {
          this.dictation.update(cx, |dictation, cx| dictation.cancel(cx));
          this.composer.update(cx, |editor, cx| editor.set_value(text.clone(), window, cx));
        }
      }),
      cx.subscribe_in(&composer, window, |this, state, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::Change) {
          cx.notify();
          if !std::mem::take(&mut this.recalling_history) {
            this.history = None;
          }
        }
        if let Some(submission) = submitted(event, state, this.composer_layout, cx) {
          this.dictation.update(cx, |dictation, cx| dictation.cancel(cx));
          this
            .model
            .update(cx, |model, cx| model.send(this.live, &submission.text, submission.follow_up, cx))
            .detach();
          state.update(cx, |state, cx| state.set_value("", window, cx));
        }
      }),
    ];
    Self {
      live,
      model,
      composer,
      dictation,
      composer_layout: Layout::Compact,
      history: None,
      recalling_history: false,
      pickers,
      _subscriptions: subscriptions,
    }
  }

  fn recall_history(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
    if self.composer.read(cx).value().as_ref() != text {
      self.recalling_history = true;
      self.composer.update(cx, |editor, cx| {
        let end = text.len();
        editor.replace_all(text, window, cx);
        editor.set_selected_range(end..end, cx);
      });
    }
  }

  pub(crate) fn editor(&self) -> &Entity<EditorState> {
    &self.composer
  }
  pub(crate) fn expanded(&self) -> bool {
    self.composer_layout == Layout::Expanded
  }

  fn id(&self, part: &str) -> ElementId {
    ElementId::from(SharedString::from(format!("{part}-{}", self.live.as_uuid())))
  }

  pub(crate) fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    self.composer_layout = if self.expanded() { Layout::Compact } else { Layout::Expanded };
    self.composer.update(cx, |state, cx| {
      state.set_submit_on_enter(self.composer_layout == Layout::Compact, cx);
      state.focus(window, cx);
    });
    cx.notify();
  }
  fn dictation_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
    let dictation = self.dictation.read(cx);
    let phase = dictation.phase();
    let label = match phase {
      Phase::Idle | Phase::Failed(_) => "Start dictation",
      Phase::Capturing => "Stop dictation",
      Phase::Preparing(_) | Phase::Transcribing => "Cancel dictation",
    };
    let button = Button::new(self.id("composer-dictation"))
      .small()
      .ghost()
      .icon(if matches!(phase, Phase::Capturing) {
        gpui_kit::assets::IconName::MicOff
      } else {
        gpui_kit::assets::IconName::Mic
      })
      .tooltip(label)
      .accessibility_label(label)
      .on_click(cx.listener(|this, _, _, cx| {
        if matches!(this.dictation.read(cx).phase(), Phase::Transcribing) {
          this.dictation.update(cx, |dictation, cx| dictation.cancel(cx));
        } else {
          this.dictation.update(cx, |dictation, cx| dictation.toggle(cx));
        }
      }));
    let status = match phase {
      Phase::Preparing(Some(alvin_stt::PreparationProgress::Downloading { .. })) => Some(match dictation.remaining(cx) {
        Some(remaining) if remaining.as_secs() >= 60 => {
          format!("About {} min left", remaining.as_secs().div_ceil(60))
        }
        Some(remaining) => format!("About {} sec left", remaining.as_secs().max(1)),
        None => "Estimating…".to_owned(),
      }),
      Phase::Preparing(Some(alvin_stt::PreparationProgress::Loading)) => Some("Loading speech model".to_owned()),
      Phase::Preparing(None) => Some("Preparing speech model".to_owned()),
      Phase::Transcribing => Some("Transcribing".to_owned()),
      Phase::Failed(error) => Some(error.clone()),
      Phase::Idle | Phase::Capturing => None,
    };
    h_flex()
      .flex_1()
      .min_w_0()
      .justify_end()
      .gap_2()
      .when_some(
        match phase {
          Phase::Preparing(Some(alvin_stt::PreparationProgress::Downloading {
            completed_bytes,
            total_bytes,
          }))
            if *total_bytes > 0 =>
          {
            Some((*completed_bytes as f64 / *total_bytes as f64 * 100.) as f32)
          }
          _ => None,
        },
        |row, percent| {
          row.child(
            gpui_kit::component::progress::Progress::new(self.id("dictation-download-progress"))
              .small()
              .w(gpui_kit::rems(6.))
              .value(percent)
              .accessibility_label("Downloading speech model"),
          )
        },
      )
      .when(matches!(phase, Phase::Capturing), |row| {
        row.child(SpeechWaveform::new(dictation.speech()).small().w(gpui_kit::rems(4.)))
      })
      .when(matches!(phase, Phase::Transcribing), |row| row.child(Spinner::new().xsmall()))
      .when_some(status, |row, status| {
        row.child(div().text_xs().text_color(cx.theme().muted_foreground).child(status))
      })
      .when(matches!(phase, Phase::Failed(_)), |row| {
        row.child(
          Button::new(self.id("dictation-retry"))
            .small()
            .ghost()
            .label("Retry")
            .on_click(cx.listener(|this, _, _, cx| this.dictation.update(cx, |dictation, cx| dictation.toggle(cx)))),
        )
      })
      .child(button)
  }

  fn interrupt_button(&self, cx: &App) -> Option<AnyElement> {
    let live = self.live;
    let running = self
      .model
      .read(cx)
      .conversation()
      .live(live)
      .is_some_and(|view| view.run == alvin_conversation::Run::Running);
    if !running || self.composer.read(cx).value().trim().is_empty() {
      return None;
    }
    let (model, composer) = (self.model.clone(), self.composer.clone());
    let dictation = self.dictation.clone();
    Some(
      Button::new(self.id("session-interrupt"))
        .small()
        .ghost()
        .label("Interrupt and send")
        .on_click(move |_, window, cx| {
          let text = composer.read(cx).value().trim().to_owned();
          if text.is_empty() {
            return;
          }
          dictation.update(cx, |dictation, cx| dictation.cancel(cx));
          composer.update(cx, |composer, cx| composer.set_value("", window, cx));
          model.update(cx, |model, cx| model.interrupt_and_send(live, text, cx)).detach();
        })
        .into_any_element(),
    )
  }

  fn send_button(&self, cx: &App) -> AnyElement {
    let (model, composer, live) = (self.model.clone(), self.composer.clone(), self.live);
    let dictation = self.dictation.clone();
    let running = model.read(cx).bash_running(live)
      || model
        .read(cx)
        .conversation()
        .live(live)
        .is_some_and(|view| view.run == alvin_conversation::Run::Running);
    let empty = composer.read(cx).value().trim().is_empty();
    if running && empty {
      let queued = !model.read(cx).bash_running(live)
        && model
          .read(cx)
          .conversation()
          .live(live)
          .is_some_and(|view| !view.queue.steering.is_empty() || !view.queue.follow_up.is_empty());
      return Button::new(self.id("session-stop"))
        .small()
        .label(if queued { "Restore queue" } else { "Stop" })
        .on_click(move |_, _, cx| model.update(cx, |model, cx| model.abort(live, cx)).detach())
        .into_any_element();
    }
    Button::new(self.id("session-send"))
      .small()
      .primary()
      .icon(IconName::ArrowUp)
      .accessibility_label("Send")
      .disabled(empty)
      .on_click(move |_, window, cx| {
        let text = composer.read(cx).value().trim().to_owned();
        if text.is_empty() {
          return;
        }
        dictation.update(cx, |dictation, cx| dictation.cancel(cx));
        composer.update(cx, |composer, cx| composer.set_value("", window, cx));
        model.update(cx, |model, cx| model.send(live, &text, false, cx)).detach();
      })
      .into_any_element()
  }

  fn context_panel(&self, cx: &App) -> impl IntoElement + use<> {
    let model = self.model.read(cx);
    let mut panel = v_flex()
      .id(self.id("session-context"))
      .test_support()
      .w(rems(18.))
      .max_h(rems(24.))
      .overflow_y_scroll()
      .min_w_0()
      .gap_4()
      .child(div().font_semibold().child("Session"));
    if let Some(view) = model.conversation().live(self.live) {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Working directory"))
          .child(div().child(SharedString::from(view.cwd.display().to_string()))),
      );
      if let Some(model) = &view.model {
        panel = panel.child(
          v_flex()
            .gap_1()
            .child(div().text_color(cx.theme().muted_foreground).child("Model"))
            .child(div().child(SharedString::from(model.name.clone())))
            .child(
              div()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(model.provider.clone())),
            ),
        );
      }
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Messages"))
          .child(div().child(SharedString::from(view.total_messages.to_string()))),
      );
    }
    if let Some(usage) = model.stats(self.live).and_then(|stats| stats.context_usage.as_ref()) {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Last reported context"))
          .child(div().child(SharedString::from(format!(
            "{} of {} tokens · {:.1}%",
            usage.tokens, usage.context_window, usage.percent
          )))),
      );
    } else {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Last reported context"))
          .child(div().child("Unavailable")),
      );
    }
    if let Some(stats) = model.stats(self.live) {
      panel = panel.child(
        v_flex()
          .gap_1()
          .child(div().text_color(cx.theme().muted_foreground).child("Usage"))
          .child(div().child(SharedString::from(format!("{} tool calls", stats.tool_calls))))
          .child(div().child(SharedString::from(format!("{} tokens", stats.tokens.total)))),
      );
    }
    panel
  }
}

impl Render for Composer {
  fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let expanded = self.expanded();
    let usage = self.model.read(cx).stats(self.live).and_then(|stats| stats.context_usage.as_ref());
    let label: SharedString = usage.map_or_else(
      || "Context usage unavailable".into(),
      |usage| format!("Context usage {:.1}%", usage.percent).into(),
    );
    let indicator = usage.map(|usage| usage.percent as f32);
    let model = self.model.clone();
    let live = self.live;
    let owner = cx.entity();
    let context = Popover::new(self.id("context-popover"))
      .anchor(Anchor::BottomLeft)
      .trigger(
        Button::new(self.id("context-toggle"))
          .xsmall()
          .ghost()
          .accessibility_label(label.clone())
          .tooltip(label.clone())
          .child(match indicator {
            Some(percent) => ProgressCircle::new(self.id("context-progress"))
              .xsmall()
              .value(percent)
              .accessibility_label(label)
              .into_any_element(),
            None => Icon::new(IconName::Info).into_any_element(),
          }),
      )
      .on_open_change(move |open, _, cx| {
        if *open {
          model.update(cx, |model, cx| model.refresh_stats(live, cx)).detach();
        }
      })
      .content(move |_, _, cx| owner.read(cx).context_panel(cx).into_any_element());
    v_flex()
      .mt_2()
      .mb_3()
      .min_w_0()
      .min_h_0()
      .gap_2()
      .when(!expanded, |editor| editor.flex_shrink_0())
      .when(expanded, |editor| editor.flex_1())
      .rounded(cx.theme().radius_lg)
      .border_1()
      .border_color(cx.theme().border)
      .bg(cx.theme().muted)
      .shadow(cx.theme().shadow_tokens().md)
      .child(
        div()
          .when(!expanded, |editor| editor.flex_shrink_0())
          .min_h_0()
          .when(expanded, |editor| editor.flex_1())
          .key_context("Composer")
          .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
            let editor = this.composer.read(cx);
            if editor.completion_menu_state().open || (editor.value().contains('\n') && editor.cursor() > 0) {
              return;
            }
            if this.history.is_none() && editor.value().is_empty() {
              this.history = history::Browse::new(history::prompts(this.model.read(cx), this.live), editor.value().to_string());
            }
            if let Some(text) = this.history.as_mut().and_then(history::Browse::previous) {
              this.recall_history(text, window, cx);
              cx.stop_propagation();
            }
          }))
          .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
            let editor = this.composer.read(cx);
            if editor.completion_menu_state().open || (editor.value().contains('\n') && editor.cursor() < editor.value().len()) {
              return;
            }
            if let Some(text) = this.history.as_mut().and_then(history::Browse::next) {
              this.recall_history(text, window, cx);
              cx.stop_propagation();
            }
          }))
          .on_action(cx.listener(|this, _: &ClearPrompt, window, cx| {
            if this.composer.read(cx).value().is_empty() {
              cx.propagate();
            } else {
              this.history = None;
              this.composer.update(cx, |editor, cx| editor.replace_all("", window, cx));
            }
          }))
          .on_action(cx.listener(|this, _: &CopyPrompt, _, cx| {
            let text = this.composer.read(cx).value().to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
          }))
          .on_action(cx.listener(|this, _: &CopyLine, _, cx| {
            let editor = this.composer.read(cx);
            let text = editor.value();
            let cursor = editor.cursor();
            if let (Some(before), Some(after)) = (text.get(..cursor), text.get(cursor..)) {
              let start = before.rfind('\n').map_or(0, |offset| offset + 1);
              let end = cursor + after.find('\n').unwrap_or(after.len());
              if let Some(line) = text.get(start..end) {
                cx.write_to_clipboard(ClipboardItem::new_string(line.to_owned()));
              }
            }
          }))
          .on_action(cx.listener(|this, _: &SearchHistory, window, cx| {
            history::open(history::prompts(this.model.read(cx), this.live), this.composer.clone(), window, cx);
          }))
          .on_action(cx.listener(|this, _: &CycleThinking, window, cx| {
            if this.composer.read(cx).completion_menu_state().open {
              window.dispatch_action(Box::new(OutdentInline), cx);
            } else {
              this.model.update(cx, |model, cx| model.cycle_thinking(this.live, cx)).detach();
            }
          }))
          .on_action(cx.listener(|this, _: &OpenModel, window, cx| {
            this.pickers.open_model(window, cx);
          }))
          .capture_action(cx.listener(|this, enter: &Enter, _, cx| {
            this.composer.update(cx, |state, cx| {
              state.set_submit_on_enter(this.composer_layout == Layout::Compact || enter.secondary, cx);
            });
            cx.propagate();
          }))
          .on_action(cx.listener(|this, _: &Escape, window, cx| {
            if this.expanded() {
              this.toggle(window, cx);
            } else {
              cx.propagate();
            }
          }))
          .child(view(&self.composer, self.composer_layout, window, cx)),
      )
      .child(
        h_flex()
          .flex_wrap()
          .px_2()
          .pb_2()
          .gap_2()
          .flex_shrink_0()
          .min_w_0()
          .child(self.pickers.view())
          .children(self.interrupt_button(cx))
          .when(expanded, |toolbar| {
            toolbar.child(
              div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if cfg!(target_os = "macos") {
                  "⌘Enter to send"
                } else {
                  "Ctrl+Enter to send"
                }),
            )
          })
          .child(context)
          .child(self.dictation_toolbar(cx))
          .child(
            Button::new(self.id("composer-expand"))
              .small()
              .ghost()
              .icon(if expanded { IconName::Minimize } else { IconName::Maximize })
              .accessibility_label(if expanded { "Collapse editor" } else { "Expand editor" })
              .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
          )
          .child(self.send_button(cx)),
      )
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::AvailableSlashCommand;
  use gpui_kit::component::input::{Editor, EditorState};
  use gpui_kit::{
    App, AppContext as _, Context, Entity, EntityInputHandler as _, IntoElement as _, Render, TestAppContext, Window, WindowOptions,
  };
  use lsp_types::CompletionTextEdit;

  use super::{DictationEdits, command_items};

  struct EditorFixture {
    editor: Entity<EditorState>,
  }

  impl Render for EditorFixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui_kit::IntoElement {
      Editor::new(&self.editor).into_any_element()
    }
  }

  fn editor_fixture(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<EditorState>) {
    cx.update(gpui_kit::init);
    let (window, fixture) = cx.update(|cx: &mut App| {
      gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        cx.new(|cx| EditorFixture {
          editor: cx.new(|cx| EditorState::new(window, cx)),
        })
      })
      .unwrap()
    });
    let editor = cx.update(|cx| fixture.read(cx).editor.clone());
    (window, editor)
  }

  #[gpui_kit::test]
  fn dictation_keeps_chunk_order_and_selection(cx: &mut TestAppContext) {
    let (window, editor) = editor_fixture(cx);
    cx.update_window(window, |_, window, cx| {
      editor.update(cx, |editor, cx| {
        let mut edits = DictationEdits::default();
        edits.push("hello");
        edits.flush(editor, window, cx);
        editor.set_selected_range_with_direction(0..5, true, cx);
        edits.push(" world");
        edits.flush(editor, window, cx);
        edits.push("!");
        edits.flush(editor, window, cx);
        assert_eq!(editor.value().as_ref(), "hello world!");
        assert_eq!(editor.selected_range(), 0..5);
        assert!(editor.is_selection_reversed());
      });
    })
    .unwrap();
  }

  #[gpui_kit::test]
  fn dictation_waits_for_ime_commit(cx: &mut TestAppContext) {
    let (window, editor) = editor_fixture(cx);
    cx.update_window(window, |_, window, cx| {
      editor.update(cx, |editor, cx| {
        editor.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
        let mut edits = DictationEdits::default();
        edits.push(" spoken");
        edits.flush(editor, window, cx);
        assert_eq!(editor.value().as_ref(), "ni");
        assert!(editor.marked_text_range(window, cx).is_some());
        editor.replace_text_in_range(None, "你", window, cx);
        edits.flush(editor, window, cx);
        assert_eq!(editor.value().as_ref(), "你 spoken");
        assert!(editor.marked_text_range(window, cx).is_none());
      });
    })
    .unwrap();
  }

  fn command(name: &str) -> AvailableSlashCommand {
    serde_json::from_value(serde_json::json!({ "name": name, "source": "builtin", "description": "does it" })).unwrap()
  }

  fn edits(text: &str, offset: usize) -> Vec<(String, String)> {
    let commands = [command("compact"), command("model"), command("move")];
    command_items(&commands, text, offset)
      .into_iter()
      .map(|item| match item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => (item.label, edit.new_text),
        other => panic!("a plain edit, got {other:?}"),
      })
      .collect()
  }

  #[test]
  fn a_leading_slash_word_completes_to_the_commands_it_prefixes() {
    assert_eq!(
      edits("/mo", 3),
      [("/model".into(), "/model ".into()), ("/move".into(), "/move ".into())]
    );
    assert_eq!(edits("/", 1).len(), 3);
  }

  #[test]
  fn nothing_completes_outside_a_leading_slash_word() {
    assert!(edits("hello /mo", 9).is_empty());
    assert!(edits("/model arg", 10).is_empty());
    assert!(edits("!ls", 3).is_empty());
    assert!(edits("/zzz", 4).is_empty());
  }
}
