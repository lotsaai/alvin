//! The protocol server traits, implemented over the gateway, and the merged method table.

use alvin_protocol::{ListSessions, ProjectSummary, ProjectsApiServer, SessionPage, SessionsApiServer};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::server::Methods;

use crate::{Gateway, Result};

#[async_trait]
impl ProjectsApiServer for Gateway {
  async fn list(&self) -> RpcResult<Vec<ProjectSummary>> {
    Ok(self.projects().await?)
  }
}

#[async_trait]
impl SessionsApiServer for Gateway {
  async fn list(&self, request: ListSessions) -> RpcResult<SessionPage> {
    Ok(self.sessions(request).await?)
  }
}

impl Gateway {
  /// Every method the gateway serves. Both transports serve exactly this table.
  pub fn methods(&self) -> Result<Methods> {
    let mut methods = Methods::from(ProjectsApiServer::into_rpc(self.clone()));
    methods.merge(SessionsApiServer::into_rpc(self.clone()))?;
    Ok(methods)
  }
}
