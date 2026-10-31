//! The gateway: shared state behind every protocol method.

use std::sync::Arc;

use alvin_protocol::ProjectSummary;

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
      Ok(alvin_session_index::group(heads.into_iter().map(|head| (head.cwd, head.id)))?)
    })
    .await
    .map_err(Error::from)?
  }
}
