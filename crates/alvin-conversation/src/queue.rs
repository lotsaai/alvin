//! The steering and follow-up messages waiting for the agent.

use alvin_protocol::QueueUpdateEvent;

/// Queued messages, replaced whole by each `queue_update`. omp identifies a queued message by its text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
  /// Messages that interrupt the running turn.
  pub steering: Vec<String>,
  /// Messages that wait for the turn to finish.
  pub follow_up: Vec<String>,
  /// How many leading `steering` entries were already sent into the streaming response; they can no longer be removed.
  pub live_steered: usize,
}

impl Queue {
  pub(crate) fn replace(&mut self, event: &QueueUpdateEvent) {
    self.steering.clone_from(&event.steering);
    self.follow_up.clone_from(&event.follow_up);
    self.live_steered = usize::try_from(event.live_steered).unwrap_or(0);
  }
}
