//! Markdown highlighting for the composer, parsed by verdant's tree-sitter grammars.
//!
//! gpui-kit installs its own highlighter only into an editor that has none, so the factory here takes precedence. Colors
//! still come from gpui-kit's theme: verdant's `markup.*` keys are translated to names that theme knows, so the composer
//! follows light and dark mode with the rest of the app.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::base::TextViewDefaults;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
  EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use gpui_kit::{App, Context, HighlightStyle, SharedString, Window};
use verdant::Processor;
use verdant::language_set::SupportedLanguage as _;
use verdant_parsers_git::{Lang, LanguageSetImpl};

/// The language name the composer's editor is configured with.
pub(crate) const LANGUAGE: &str = "markdown";

/// Non-overlapping highlighted byte ranges in source order, each with its verdant theme key.
type Highlights = Vec<(Range<usize>, &'static str)>;

/// The factory that gives an editor configured for [`LANGUAGE`] a verdant-backed highlighter.
pub(crate) fn factory() -> InputHighlighterFactory {
  Rc::new(|language| (language == LANGUAGE).then(|| Box::new(MarkdownHighlighter::default()) as Box<dyn InputHighlighter>))
}

/// Installs code-fence highlighting using the active syntax theme.
pub fn refresh_markdown_theme(cx: &mut App) {
  let theme = cx.theme().highlight_theme.clone();
  TextViewDefaults::global(cx)
    .with_code_block_highlighter(move |block| {
      let Some(language) = block.lang() else {
        return Vec::new();
      };
      let code = block.code();
      match highlight_language(&code, &language) {
        Ok(highlights) => highlights
          .into_iter()
          .filter_map(|(range, key)| theme.style(theme_name(key)).map(|style| (range, style)))
          .collect(),
        Err(verdant::Error::UnsupportedLanguage(_)) => Vec::new(),
        Err(error) => {
          tracing::warn!(%error, %language, "code highlighting failed");
          Vec::new()
        }
      }
    })
    .install(cx);
}

/// Parses the whole text on every change. Composer text is a few lines, so an incremental parse buys nothing.
#[derive(Default)]
struct MarkdownHighlighter {
  highlights: Highlights,
}

/// The highlighted ranges of `text`. Inline markup and fenced code are injected into the block grammar by verdant; a fence in
/// a language this build has no grammar for stays unhighlighted.
fn highlight(text: &str) -> verdant::Result<Highlights> {
  highlight_language(text, LANGUAGE)
}

fn highlight_language(text: &str, language: &str) -> verdant::Result<Highlights> {
  thread_local! {
    // Compiled queries are cached per set, so one set per thread keeps every later parse cheap.
    static SET: LanguageSetImpl = LanguageSetImpl::new();
  }
  SET.with(|set| {
    let language = Lang::for_name(language, set)?;
    let lines = Processor::process_once(text, language, set)?;
    let base = text.as_ptr() as usize;
    Ok(
      lines
        .into_iter()
        .flatten()
        .filter_map(|(piece, key)| {
          // Every piece is a slice of `text`, so its address is never below the text's.
          let start = (piece.as_ptr() as usize).checked_sub(base)?;
          Some((start..start + piece.len(), key?))
        })
        .collect(),
    )
  })
}

/// The name gpui-kit's syntax theme knows for a verdant key. Keys it has no markup entry for fall through unchanged and are
/// resolved by their leading segment.
fn theme_name(key: &str) -> &str {
  match key {
    key if key.starts_with("markup.heading") => "title",
    "markup.strong" => "emphasis.strong",
    "markup.italic" => "emphasis",
    key if key.starts_with("markup.raw") => "text.literal",
    "markup.link.url" => "link_uri",
    key if key.starts_with("markup.link") => "link_text",
    key if key.starts_with("markup.list") => "punctuation.list_marker",
    "markup.quote" => "comment",
    key => key,
  }
}

impl InputHighlighter for MarkdownHighlighter {
  fn language(&self) -> SharedString {
    SharedString::new_static(LANGUAGE)
  }

  fn update(&mut self, _: Option<InputEdit>, text: &Rope, _: bool, _: &mut Window, _: &mut Context<EditorState>) {
    self.highlights = highlight(&text.to_string()).unwrap_or_else(|error| {
      tracing::warn!(%error, "markdown highlighting failed; the composer text is shown unstyled");
      Vec::new()
    });
  }

  fn styles(&self, range: &Range<usize>, resolver: &dyn HighlightStyleResolver) -> Vec<(Range<usize>, HighlightStyle)> {
    let first = self.highlights.partition_point(|(highlight, _)| highlight.end <= range.start);
    let mut runs = Vec::new();
    let mut cursor = range.start;
    for (highlight, key) in &self.highlights[first..] {
      if highlight.start >= range.end {
        break;
      }
      let (start, end) = (highlight.start.max(range.start), highlight.end.min(range.end));
      if start >= end || end <= cursor {
        continue;
      }
      if cursor < start {
        runs.push((cursor..start, HighlightStyle::default()));
      }
      runs.push((start..end, resolver.style(theme_name(key)).unwrap_or_default()));
      cursor = end;
    }
    if cursor < range.end {
      runs.push((cursor..range.end, HighlightStyle::default()));
    }
    runs
  }

  fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
    Vec::new()
  }
}

#[cfg(test)]
mod tests {
  use super::{highlight, highlight_language, theme_name};

  #[test]
  fn code_fences_use_language_grammars() {
    let code = "fn main() { let n = 42; }\n";
    let highlights = highlight_language(code, "rust").unwrap();
    assert!(key_of(code, &highlights, "fn").is_some());
    assert!(key_of(code, &highlights, "42").is_some());
    assert!(matches!(
      highlight_language(code, "not-a-language"),
      Err(verdant::Error::UnsupportedLanguage(_))
    ));
  }

  /// The key verdant assigns to the first occurrence of `needle`, whatever else covers it.
  fn key_of<'t>(text: &str, highlights: &[(std::ops::Range<usize>, &'t str)], needle: &str) -> Option<&'t str> {
    let start = text.find(needle)?;
    highlights
      .iter()
      .find(|(range, _)| range.start <= start && start < range.end)
      .map(|(_, key)| *key)
  }

  #[test]
  fn block_and_inline_markup_are_highlighted_at_their_byte_offsets() {
    let text = "# Title\n\nsome **bold** and `code`\n\n- item\n";
    let highlights = highlight(text).unwrap();

    assert!(
      highlights.windows(2).all(|pair| pair[0].0.end <= pair[1].0.start),
      "ranges stay ordered and apart: {highlights:?}"
    );
    assert!(
      key_of(text, &highlights, "Title").is_some_and(|key| key.starts_with("markup.heading")),
      "{highlights:?}"
    );
    assert_eq!(key_of(text, &highlights, "bold"), Some("markup.strong"), "{highlights:?}");
    assert!(
      key_of(text, &highlights, "- ").is_some_and(|key| key.starts_with("markup.list")),
      "{highlights:?}"
    );
  }

  #[test]
  fn multibyte_text_keeps_offsets_on_char_boundaries() {
    let text = "héllo **wörld** ✓\n";
    let highlights = highlight(text).unwrap();

    assert!(
      highlights
        .iter()
        .all(|(range, _)| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
    );
    assert_eq!(key_of(text, &highlights, "wörld"), Some("markup.strong"), "{highlights:?}");
  }

  #[test]
  fn markup_keys_map_to_names_the_app_theme_knows() {
    assert_eq!(theme_name("markup.heading.2"), "title");
    assert_eq!(theme_name("markup.strong"), "emphasis.strong");
    assert_eq!(theme_name("markup.raw.block"), "text.literal");
    assert_eq!(theme_name("markup.link.url"), "link_uri");
    assert_eq!(theme_name("string.escape"), "string.escape");
  }
}
