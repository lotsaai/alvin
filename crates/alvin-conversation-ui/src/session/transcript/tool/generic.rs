use serde_json::Value;

use super::{Presentation, result_text};

pub(super) fn parse(result: Option<&Value>) -> Presentation {
  Presentation::text("Details".to_owned(), result.map(result_text).unwrap_or_default())
}
