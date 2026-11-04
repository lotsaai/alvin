use serde_json::Value;

use super::{Presentation, text};

pub(super) fn parse(args: Option<&Value>, result: Option<&Value>) -> Option<Presentation> {
  let path = text(args, "path").unwrap_or_default();
  let diff = result?.pointer("/details/diff")?.as_str()?;
  let (mut start, mut old, mut new) = (None, 0usize, 0usize);
  let mut lines = String::new();
  for line in diff.lines() {
    let sign = line.chars().next().filter(|sign| matches!(sign, ' ' | '+' | '-'))?;
    let (number, content) = line.get(1..)?.split_once('|')?;
    start.get_or_insert(number.trim().parse::<usize>().ok()?);
    match sign {
      ' ' => {
        old = old.saturating_add(1);
        new = new.saturating_add(1);
      }
      '-' => old = old.saturating_add(1),
      _ => new = new.saturating_add(1),
    }
    lines.push(sign);
    lines.push_str(content);
    lines.push('\n');
  }
  let start = start?;
  Some(Presentation::diff(
    path.to_owned(),
    format!("--- a/{path}\n+++ b/{path}\n@@ -{start},{old} +{start},{new} @@\n{lines}"),
  ))
}
