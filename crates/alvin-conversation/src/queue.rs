//! The steering and follow-up messages waiting for the agent.

use alvin_protocol::{QueueUpdateEvent, QueuedMessageId};

/// A message waiting in a queue. omp names it by its text; the id is minted here so a view can tell two entries apart and
/// keep one entry's identity while others come and go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
  /// Stable for as long as this text stays queued in this queue.
  pub id: QueuedMessageId,
  /// What omp knows the message by.
  pub text: String,
}

/// Queued messages, replaced whole by each `queue_update`. omp identifies a queued message by its text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Queue {
  /// Messages that interrupt the running turn.
  pub steering: Vec<QueuedMessage>,
  /// Messages that wait for the turn to finish.
  pub follow_up: Vec<QueuedMessage>,
  /// How many leading `steering` entries were already sent into the streaming response; they can no longer be removed.
  pub live_steered: usize,
}

/// The messages of `texts`, each keeping the id of an equal-text entry of `previous` that no earlier message took, and
/// getting a fresh id otherwise.
fn track(previous: &[QueuedMessage], texts: &[String]) -> Vec<QueuedMessage> {
  let mut taken = vec![false; previous.len()];
  texts
    .iter()
    .map(|text| {
      let reused = previous
        .iter()
        .enumerate()
        .find(|(index, message)| !taken.get(*index).copied().unwrap_or(true) && message.text == *text);
      let id = match reused {
        Some((index, message)) => {
          if let Some(slot) = taken.get_mut(index) {
            *slot = true;
          }
          message.id
        }
        None => QueuedMessageId::new(),
      };
      QueuedMessage { id, text: text.clone() }
    })
    .collect()
}

impl Queue {
  pub(crate) fn replace(&mut self, event: &QueueUpdateEvent) {
    self.steering = track(&self.steering, &event.steering);
    self.follow_up = track(&self.follow_up, &event.follow_up);
    self.live_steered = usize::try_from(event.live_steered).unwrap_or(0);
  }
}

#[cfg(test)]
mod tests {
  use alvin_protocol::QueueUpdateEvent;

  use super::Queue;

  fn update(steering: &[&str], follow_up: &[&str]) -> QueueUpdateEvent {
    QueueUpdateEvent {
      steering: steering.iter().map(|text| (*text).to_owned()).collect(),
      follow_up: follow_up.iter().map(|text| (*text).to_owned()).collect(),
      live_steered: 0,
    }
  }

  #[test]
  fn a_message_keeps_its_id_while_others_come_and_go() {
    let mut queue = Queue::default();
    queue.replace(&update(&[], &["a", "b"]));
    let (a, b) = (queue.follow_up[0].id, queue.follow_up[1].id);
    assert_ne!(a, b);

    queue.replace(&update(&[], &["b", "c"]));
    assert_eq!(queue.follow_up[0].id, b, "b stayed queued");
    assert_ne!(queue.follow_up[1].id, a, "c is a new message");
  }

  #[test]
  fn equal_texts_get_distinct_ids_and_keep_them() {
    let mut queue = Queue::default();
    queue.replace(&update(&[], &["same", "same"]));
    let ids = [queue.follow_up[0].id, queue.follow_up[1].id];
    assert_ne!(ids[0], ids[1]);

    queue.replace(&update(&[], &["same", "same"]));
    assert_eq!([queue.follow_up[0].id, queue.follow_up[1].id], ids);
  }
}
