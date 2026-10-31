//! The protocol server traits, implemented over the gateway, and the merged method table.

use std::time::Instant;

use alvin_protocol::{
  AbortAndRestoreQueueResult, BashRequest, BashResult, BranchMessage, BranchResult, CancellationResult, DialogApiServer, EntriesRequest,
  EntryRequest, EventsApiServer, ForkRequest, HistoryEntries, LabelRequest, ListSessions, LiveApiServer, LiveSessionId, LiveSessionSummary,
  MessagesPage, MessagesRequest, ModelApiServer, ModelCycleResult, ModelInfo, NavigateRequest, NavigateTreeResult, OpenRequest,
  ProjectSummary, ProjectsApiServer, PromoteQueuedMessageResult, PromoteQueuedRequest, PromptRequest, QueueApiServer,
  RemoveQueuedMessageResult, RemoveQueuedRequest, RespondRequest, SessionPage, SessionState, SessionsApiServer, SetModelRequest,
  SetThinkingRequest, SubscribeRequest, ThinkingLevel, ThinkingLevelCycleResult,
};
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::server::{Methods, PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use tokio::sync::broadcast::error::RecvError;
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

#[async_trait]
impl LiveApiServer for Gateway {
  async fn open(&self, request: OpenRequest) -> RpcResult<LiveSessionSummary> {
    self.traced("live/open", Gateway::open(self, request)).await
  }

  async fn close(&self, live: LiveSessionId) -> RpcResult<()> {
    self.traced("live/close", Gateway::close(self, live)).await
  }

  async fn list(&self) -> RpcResult<Vec<LiveSessionSummary>> {
    self.traced("live/list", async { Ok(self.list_live()) }).await
  }

  async fn state(&self, live: LiveSessionId) -> RpcResult<SessionState> {
    self.traced("live/state", Gateway::state(self, live)).await
  }

  async fn messages(&self, request: MessagesRequest) -> RpcResult<MessagesPage> {
    self.traced("live/messages", Gateway::messages(self, request)).await
  }

  async fn prompt(&self, request: PromptRequest) -> RpcResult<()> {
    self.traced("live/prompt", Gateway::prompt(self, request)).await
  }

  async fn steer(&self, request: PromptRequest) -> RpcResult<()> {
    self.traced("live/steer", Gateway::steer(self, request)).await
  }

  async fn follow_up(&self, request: PromptRequest) -> RpcResult<()> {
    self.traced("live/followUp", Gateway::follow_up(self, request)).await
  }

  async fn abort_and_prompt(&self, request: PromptRequest) -> RpcResult<()> {
    self.traced("live/abortAndPrompt", Gateway::abort_and_prompt(self, request)).await
  }

  async fn abort(&self, live: LiveSessionId) -> RpcResult<()> {
    self.traced("live/abort", Gateway::abort(self, live)).await
  }

  async fn bash(&self, request: BashRequest) -> RpcResult<BashResult> {
    self.traced("live/bash", Gateway::bash(self, request)).await
  }

  async fn abort_bash(&self, live: LiveSessionId) -> RpcResult<()> {
    self.traced("live/abortBash", Gateway::abort_bash(self, live)).await
  }

  async fn entries(&self, request: EntriesRequest) -> RpcResult<HistoryEntries> {
    self.traced("live/entries", Gateway::entries(self, request)).await
  }

  async fn branch_messages(&self, live: LiveSessionId) -> RpcResult<Vec<BranchMessage>> {
    self.traced("live/branchMessages", Gateway::branch_messages(self, live)).await
  }

  async fn navigate_tree(&self, request: NavigateRequest) -> RpcResult<NavigateTreeResult> {
    self.traced("live/navigateTree", Gateway::navigate_tree(self, request)).await
  }

  async fn abort_branch_summary(&self, live: LiveSessionId) -> RpcResult<()> {
    self
      .traced("live/abortBranchSummary", Gateway::abort_branch_summary(self, live))
      .await
  }

  async fn branch(&self, request: EntryRequest) -> RpcResult<BranchResult> {
    self.traced("live/branch", Gateway::branch(self, request)).await
  }

  async fn fork(&self, request: ForkRequest) -> RpcResult<CancellationResult> {
    self.traced("live/fork", Gateway::fork(self, request)).await
  }

  async fn label(&self, request: LabelRequest) -> RpcResult<()> {
    self.traced("live/label", Gateway::label(self, request)).await
  }
}

#[async_trait]
impl QueueApiServer for Gateway {
  async fn remove(&self, request: RemoveQueuedRequest) -> RpcResult<RemoveQueuedMessageResult> {
    self.traced("queue/remove", self.remove_queued(request)).await
  }

  async fn promote(&self, request: PromoteQueuedRequest) -> RpcResult<PromoteQueuedMessageResult> {
    self.traced("queue/promote", self.promote_queued(request)).await
  }

  async fn restore(&self, live: LiveSessionId) -> RpcResult<AbortAndRestoreQueueResult> {
    self.traced("queue/restore", self.restore_queue(live)).await
  }
}

#[async_trait]
impl DialogApiServer for Gateway {
  async fn respond(&self, request: RespondRequest) -> RpcResult<()> {
    self.traced("dialog/respond", Gateway::respond(self, request)).await
  }
}

#[async_trait]
impl ModelApiServer for Gateway {
  async fn models(&self, live: LiveSessionId) -> RpcResult<Vec<ModelInfo>> {
    self.traced("model/list", Gateway::models(self, live)).await
  }

  async fn set_model(&self, request: SetModelRequest) -> RpcResult<ModelInfo> {
    self.traced("model/set", Gateway::set_model(self, request)).await
  }

  async fn cycle_model(&self, live: LiveSessionId) -> RpcResult<Option<ModelCycleResult>> {
    self.traced("model/cycle", Gateway::cycle_model(self, live)).await
  }

  async fn thinking_levels(&self, live: LiveSessionId) -> RpcResult<Vec<ThinkingLevel>> {
    self.traced("thinking/levels", Gateway::thinking_levels(self, live)).await
  }

  async fn set_thinking(&self, request: SetThinkingRequest) -> RpcResult<()> {
    self.traced("thinking/set", Gateway::set_thinking(self, request)).await
  }

  async fn cycle_thinking(&self, live: LiveSessionId) -> RpcResult<Option<ThinkingLevelCycleResult>> {
    self.traced("thinking/cycle", Gateway::cycle_thinking(self, live)).await
  }
}

#[async_trait]
impl EventsApiServer for Gateway {
  async fn events(&self, pending: PendingSubscriptionSink, request: SubscribeRequest) -> SubscriptionResult {
    let (replay, mut receiver, log) = match self.subscribe(&request) {
      Ok(subscription) => subscription,
      Err(error) => {
        pending.reject(ErrorObjectOwned::from(error)).await;
        return Ok(());
      }
    };
    let sink = pending.accept().await?;
    for event in replay {
      if sink
        .send(SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &event)?)
        .await
        .is_err()
      {
        return Ok(());
      }
    }
    loop {
      tokio::select! {
        () = sink.closed() => return Ok(()),
        received = receiver.recv() => match received {
          Ok(event) => {
            if sink.send(SubscriptionMessage::new(sink.method_name(), sink.subscription_id(), &event)?).await.is_err() {
              return Ok(());
            }
          }
          // The subscriber fell behind the broadcast buffer. The subscription ends; the client resubscribes with its
          // last event id and the log covers the gap while the events are still retained.
          Err(RecvError::Lagged(skipped)) => {
            tracing::debug!(skipped, "ending a subscription that fell behind");
            log.metrics().subscription_stale();
            return Ok(());
          }
          Err(RecvError::Closed) => return Ok(()),
        },
      }
    }
  }
}

impl Gateway {
  /// Every method the gateway serves. Both transports serve exactly this table.
  pub(crate) fn methods(&self) -> Result<Methods> {
    let mut methods = Methods::from(ProjectsApiServer::into_rpc(self.clone()));
    methods.merge(SessionsApiServer::into_rpc(self.clone()))?;
    methods.merge(LiveApiServer::into_rpc(self.clone()))?;
    methods.merge(QueueApiServer::into_rpc(self.clone()))?;
    methods.merge(ModelApiServer::into_rpc(self.clone()))?;
    methods.merge(DialogApiServer::into_rpc(self.clone()))?;
    methods.merge(EventsApiServer::into_rpc(self.clone()))?;
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
  use alvin_omp_host::HostConfig;

  use super::*;
  use crate::GatewayConfig;

  #[test]
  fn the_declared_method_list_is_pinned() {
    let config = GatewayConfig::builder()
      .sessions_dir("sessions")
      .omp(HostConfig::builder().omp("omp".into()).build())
      .build();
    let mut methods: Vec<_> = Gateway::new(config).methods().unwrap().method_names().collect();
    methods.sort_unstable();
    assert_eq!(
      methods,
      [
        "dialog/respond",
        "events/subscribe",
        "events/unsubscribe",
        "live/abort",
        "live/abortAndPrompt",
        "live/abortBash",
        "live/abortBranchSummary",
        "live/bash",
        "live/branch",
        "live/branchMessages",
        "live/close",
        "live/entries",
        "live/followUp",
        "live/fork",
        "live/label",
        "live/list",
        "live/messages",
        "live/navigateTree",
        "live/open",
        "live/prompt",
        "live/state",
        "live/steer",
        "model/cycle",
        "model/list",
        "model/set",
        "projects/list",
        "queue/promote",
        "queue/remove",
        "queue/restore",
        "sessions/list",
        "thinking/cycle",
        "thinking/levels",
        "thinking/set",
      ]
    );
  }
}
