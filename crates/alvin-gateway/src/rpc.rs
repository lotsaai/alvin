//! The protocol server traits, implemented over the gateway, and the merged method table.

use alvin_protocol::{ProjectSummary, ProjectsApiServer};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::server::Methods;

use crate::Gateway;

#[async_trait]
impl ProjectsApiServer for Gateway {
  async fn list(&self) -> RpcResult<Vec<ProjectSummary>> {
    Ok(self.projects().await?)
  }
}

impl Gateway {
  /// Every method the gateway serves. Both transports serve exactly this table.
  pub fn methods(&self) -> Methods {
    ProjectsApiServer::into_rpc(self.clone()).into()
  }
}
