//! Black-box tests of the gateway's wire contract, exercised through `alvin-client` over both transports.

use std::path::{Path, PathBuf};

use alvin_client::{ChannelPair, Client, Endpoint, Error};
use alvin_gateway::{Gateway, GatewayConfig, WsServer};
use alvin_protocol::{ErrorCode, ErrorData, EventId, ProjectSummary, ProjectsApiClient, SessionId};

/// Session files written by real omp (the first two records of one run).
fn fixture_sessions() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/sessions")
}

fn gateway(sessions_dir: impl Into<PathBuf>) -> Gateway {
  Gateway::new(GatewayConfig::builder().sessions_dir(sessions_dir.into()).build())
}

async fn channel_client(gateway: &Gateway) -> Client {
  Client::connect(Endpoint::Channel(ChannelPair::from(gateway.connect_channel(16))))
    .await
    .unwrap()
}

async fn ws_client(gateway: &Gateway) -> (Client, WsServer) {
  let server = gateway.serve_ws(([127, 0, 0, 1], 0).into()).await.unwrap();
  let endpoint = format!("ws://{}", server.local_addr()).parse::<Endpoint>().unwrap();
  (Client::connect(endpoint).await.unwrap(), server)
}

#[test]
fn the_declared_method_list_is_pinned() {
  let mut methods: Vec<_> = gateway(fixture_sessions()).methods().method_names().collect();
  methods.sort_unstable();
  assert_eq!(methods, ["projects/list"]);
}

#[tokio::test]
async fn projects_list_returns_the_same_projects_over_channel_and_websocket() {
  let gateway = gateway(fixture_sessions());
  let over_channel = channel_client(&gateway).await.list().await.unwrap();
  let (client, server) = ws_client(&gateway).await;
  let over_ws = client.list().await.unwrap();
  drop(client);
  server.shutdown().await.unwrap();

  assert_eq!(over_channel, over_ws);
  // The recorded cwd was a temporary directory that no longer exists, so the session is an unresolved group.
  assert!(
    matches!(
      over_ws.as_slice(),
      [ProjectSummary::Unresolved { sessions, .. }]
        if *sessions == [SessionId::from("01a18ff1-ab74-73b8-a3a2-79905565c49b".to_owned())]
    ),
    "{over_ws:?}"
  );
}

#[tokio::test]
async fn a_gateway_error_reaches_the_client_typed_over_both_transports() {
  let missing = std::env::temp_dir().join(format!("alvin-missing-{}", EventId::new().as_uuid()));
  let gateway = gateway(&missing);
  let (ws, server) = ws_client(&gateway).await;
  let channel = channel_client(&gateway).await;

  let results = [
    channel.list().await.map_err(Error::from).unwrap_err(),
    ws.list().await.map_err(Error::from).unwrap_err(),
  ];
  drop(ws);
  server.shutdown().await.unwrap();

  for error in results {
    assert!(
      matches!(
        &error,
        Error::Gateway { code: ErrorCode::SessionIndexFailed, data: Some(ErrorData::SessionIndex { path }), .. }
          if *path == missing
      ),
      "{error:?}"
    );
  }
}
