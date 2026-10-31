//! The protocol server traits, implemented over the gateway, and the merged method table.

use std::time::Instant;

use alvin_protocol::{ListSessions, ProjectSummary, ProjectsApiServer, SessionPage, SessionsApiServer};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::server::Methods;
use jsonrpsee::types::ErrorObjectOwned;
use tracing::Instrument;

use crate::{Gateway, Result};

#[async_trait]
impl ProjectsApiServer for Gateway {
  async fn list(&self) -> RpcResult<Vec<ProjectSummary>> {
    self.traced("projects/list", self.projects()).await
  }
}

#[async_trait]
impl SessionsApiServer for Gateway {
  async fn list(&self, request: ListSessions) -> RpcResult<SessionPage> {
    self.traced("sessions/list", self.sessions(request)).await
  }
}

impl Gateway {
  /// Every method the gateway serves. Both transports serve exactly this table.
  pub(crate) fn methods(&self) -> Result<Methods> {
    let mut methods = Methods::from(ProjectsApiServer::into_rpc(self.clone()));
    methods.merge(SessionsApiServer::into_rpc(self.clone()))?;
    Ok(methods)
  }

  /// Runs one request inside a span named after its method and records `alvin.rpc.duration` with the outcome: `ok`, or
  /// the wire error code. Wrapping the implementations rather than a transport middleware keeps both transports covered.
  async fn traced<T>(&self, method: &'static str, call: impl Future<Output = Result<T>>) -> RpcResult<T> {
    let span = tracing::info_span!("rpc", rpc.method = method, error.code = tracing::field::Empty);
    let started = Instant::now();
    let result = call.instrument(span.clone()).await.map_err(ErrorObjectOwned::from);
    let outcome = match &result {
      Ok(_) => "ok".to_owned(),
      Err(error) => {
        span.record("error.code", error.code());
        error.code().to_string()
      }
    };
    self.metrics.record_rpc(method, &outcome, started.elapsed());
    result
  }
}

#[cfg(test)]
mod tests {
  use crate::GatewayConfig;

  use super::*;

  #[test]
  fn the_declared_method_list_is_pinned() {
    let gateway = Gateway::new(GatewayConfig::builder().sessions_dir("sessions").build());
    let mut methods: Vec<_> = gateway.methods().unwrap().method_names().collect();
    methods.sort_unstable();
    assert_eq!(methods, ["projects/list", "sessions/list"]);
  }
}
