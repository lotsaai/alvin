use serde_json::Value;

use super::{Presentation, text};

pub(super) fn parse(args: Option<&Value>) -> Option<Presentation> {
  let path = text(args, "path")?;
  let content = text(args, "content")?;
  Some(Presentation::file(path.to_owned(), content.to_owned()))
}
