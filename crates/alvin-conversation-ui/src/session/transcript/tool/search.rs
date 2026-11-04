use serde_json::Value;

use super::{Body, Presentation, Summary, result_text, text};

#[derive(Clone)]
pub(super) struct FileMatch {
  pub(super) path: String,
  pub(super) count: Option<u64>,
}

#[derive(Clone)]
pub(super) struct SearchResults {
  pub(super) files: Vec<String>,
  pub(super) matches: Vec<FileMatch>,
  pub(super) match_count: Option<u64>,
  pub(super) file_count: Option<u64>,
  pub(super) truncated: Option<bool>,
  pub(super) output: String,
}

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let result = result?;
  if result.get("content").and_then(Value::as_array).is_none() && result.pointer("/details/files").and_then(Value::as_array).is_none() {
    return None;
  }
  let files: Vec<String> = result
    .pointer("/details/files")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::to_owned)
    .collect();
  let output = result_text(result);
  let output = if output.is_empty() { files.join("\n") } else { output };
  let details = result.get("details");
  let matches = details
    .and_then(|value| value.get("fileMatches"))
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|value| {
      Some(FileMatch {
        path: value.get("path")?.as_str()?.to_owned(),
        count: value.get("count").and_then(Value::as_u64),
      })
    })
    .collect();
  Some(Presentation {
    summary: Summary::Search {
      pattern: text(args, "pattern").map(str::to_owned),
      path: text(args, "path").map(str::to_owned),
    },
    body: Body::Search(SearchResults {
      files,
      matches,
      match_count: details.and_then(|value| value.get("matchCount")).and_then(Value::as_u64),
      file_count: details.and_then(|value| value.get("fileCount")).and_then(Value::as_u64),
      truncated: details.and_then(|value| value.get("truncated")).and_then(Value::as_bool),
      output,
    }),
  })
}
