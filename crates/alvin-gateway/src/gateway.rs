//! The gateway: shared state behind every protocol method.

use std::sync::Arc;

use alvin_protocol::{ListSessions, ProjectSummary, SessionPage};

use crate::{Error, GatewayConfig, Result};

/// The application boundary. Cheap to clone; every transport serves the same methods from one instance.
#[derive(Debug, Clone)]
pub struct Gateway {
  config: Arc<GatewayConfig>,
}

impl Gateway {
  /// Creates a gateway from its configuration.
  pub fn new(config: GatewayConfig) -> Self {
    Self { config: Arc::new(config) }
  }

  /// Every project with sessions. A session file that cannot be read fails the listing.
  pub(crate) async fn projects(&self) -> Result<Vec<ProjectSummary>> {
    let config = Arc::clone(&self.config);
    tokio::task::spawn_blocking(move || {
      let heads = alvin_session_index::scan(&config.sessions_dir)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
      Ok(alvin_session_index::group(heads.into_iter().map(|head| head.cwd))?)
    })
    .await
    .map_err(Error::from)?
  }

  /// One page of a project's sessions. Validates the request here, once: the page is not empty and the project (and
  /// checkout) exist.
  pub(crate) async fn sessions(&self, request: ListSessions) -> Result<SessionPage> {
    let limit = usize::try_from(request.limit)
      .ok()
      .filter(|limit| *limit > 0)
      .ok_or(Error::EmptyPage)?;
    let config = Arc::clone(&self.config);
    tokio::task::spawn_blocking(move || {
      let heads = alvin_session_index::scan(&config.sessions_dir)?
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
      let selected = alvin_session_index::select(heads, &request.project, request.checkout.as_ref())?;
      if selected.is_empty() {
        return Err(Error::UnknownProject {
          project: request.project,
          checkout: request.checkout,
        });
      }
      Ok(alvin_session_index::list_page(selected, request.after.as_ref(), limit)?)
    })
    .await
    .map_err(Error::from)?
  }
}
