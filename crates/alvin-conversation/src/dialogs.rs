//! Extension UI requests waiting for an answer.

use alvin_protocol::ExtensionUiRequest;

/// The id of a request, which an answer or a cancel names.
pub fn request_id(request: &ExtensionUiRequest) -> &str {
  match request {
    ExtensionUiRequest::Select(request) => &request.id,
    ExtensionUiRequest::Confirm(request) => &request.id,
    ExtensionUiRequest::Input(request) => &request.id,
    ExtensionUiRequest::Editor(request) => &request.id,
    ExtensionUiRequest::Ask(request) => &request.id,
    ExtensionUiRequest::Cancel(request) => &request.id,
    ExtensionUiRequest::Notify(request) => &request.id,
    ExtensionUiRequest::SetStatus(request) => &request.id,
    ExtensionUiRequest::SetWidget(request) => &request.id,
    ExtensionUiRequest::SetTitle(request) => &request.id,
    ExtensionUiRequest::SetEditorText(request) => &request.id,
    ExtensionUiRequest::OpenUrl(request) => &request.id,
  }
}

/// The dialogs a session is waiting on, oldest first: `select`, `confirm`, `input`, `editor` and `ask` requests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dialogs {
  pending: Vec<ExtensionUiRequest>,
}

impl Dialogs {
  /// The pending requests, oldest first.
  pub fn pending(&self) -> &[ExtensionUiRequest] {
    &self.pending
  }

  pub(crate) fn open(&mut self, request: ExtensionUiRequest) {
    self.pending.push(request);
  }

  /// Closes the dialog a request named, whether the user answered it elsewhere or omp cancelled it.
  pub(crate) fn close(&mut self, id: &str) {
    self.pending.retain(|request| request_id(request) != id);
  }

  pub(crate) fn clear(&mut self) {
    self.pending.clear();
  }
}
