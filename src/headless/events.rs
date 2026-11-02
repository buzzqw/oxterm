//! Event stream published by a running Oxterm.
//!
//! The GUI thread publishes one JSON object per event. A control-socket client
//! subscribes with [`EventBus::subscribe`] and receives them through a bounded
//! channel; `src/control.rs` turns that channel into length-prefixed frames.
//!
//! Everything here is display-free so the filtering and queueing rules can be
//! tested without a window. A slow subscriber is disconnected instead of
//! blocking the GUI or growing memory without bound.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;

use serde_json::Value;

/// Concurrent event subscriptions. The control socket already limits clients;
/// this is the tighter limit that keeps a script from flooding the bus.
pub const MAX_SUBSCRIBERS: usize = 16;

/// Events buffered for one subscriber before it is considered too slow.
pub const SUBSCRIBER_QUEUE: usize = 256;

/// Every event name Oxterm can publish. `output_line` is opt-in per
/// subscription because it can contain secrets.
pub const EVENT_NAMES: &[&str] = &[
    "command_started",
    "command_finished",
    "prompt",
    "bell",
    "title_changed",
    "cwd_changed",
    "tab_created",
    "tab_closed",
    "pane_focused",
    "selection_changed",
    "theme_changed",
    "output_line",
];

/// What a subscriber wants to receive.
#[derive(Clone, Debug, Default)]
pub struct EventFilter {
    /// Only events for this pane id.
    pub pane: Option<String>,
    /// Only these event names; `None` means every name except `output_line`.
    pub events: Option<Vec<String>>,
    /// Receive `output_line` too. Off by default: it can contain secrets.
    pub include_output: bool,
}

impl EventFilter {
    fn accepts(&self, name: &str, pane: Option<&str>) -> bool {
        if name == "output_line" && !self.include_output {
            return false;
        }
        if let Some(wanted) = &self.events {
            if !wanted.iter().any(|entry| entry == name) {
                return false;
            }
        }
        if let Some(wanted) = &self.pane {
            if pane != Some(wanted.as_str()) {
                return false;
            }
        }
        true
    }
}

struct Subscriber {
    id: u64,
    filter: EventFilter,
    sender: SyncSender<Value>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    subscribers: Vec<Subscriber>,
}

/// Fan-out of events to every interested subscriber.
pub struct EventBus {
    state: Mutex<State>,
}

impl EventBus {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                next_id: 1,
                subscribers: Vec::new(),
            }),
        }
    }

    /// Register a subscriber. `None` means the subscriber limit was reached.
    pub fn subscribe(&self, filter: EventFilter) -> Option<(u64, Receiver<Value>)> {
        let mut state = self.state.lock().ok()?;
        if state.subscribers.len() >= MAX_SUBSCRIBERS {
            return None;
        }
        let (sender, receiver) = sync_channel(SUBSCRIBER_QUEUE);
        let id = state.next_id;
        state.next_id += 1;
        state.subscribers.push(Subscriber { id, filter, sender });
        Some((id, receiver))
    }

    pub fn unsubscribe(&self, id: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.subscribers.retain(|subscriber| subscriber.id != id);
        }
    }

    /// Deliver one event. Events without a string `event` field are ignored.
    ///
    /// A subscriber whose queue is full is dropped: a client that cannot keep
    /// up must not stall the GUI or make Oxterm allocate without bound.
    pub fn publish(&self, event: &Value) {
        let Some(name) = event.get("event").and_then(Value::as_str) else {
            return;
        };
        let pane = event.get("pane").and_then(Value::as_str);
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.subscribers.retain(|subscriber| {
            if !subscriber.filter.accepts(name, pane) {
                return true;
            }
            match subscriber.sender.try_send(event.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    pub fn subscriber_count(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.subscribers.len())
            .unwrap_or(0)
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

static BUS: once_cell::sync::Lazy<EventBus> = once_cell::sync::Lazy::new(EventBus::new);

/// The process-wide bus shared by the GUI and the control server.
pub fn bus() -> &'static EventBus {
    &BUS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    fn event(name: &str, pane: Option<&str>) -> Value {
        let mut value = json!({ "event": name });
        if let Some(pane) = pane {
            value["pane"] = json!(pane);
        }
        value
    }

    #[test]
    fn delivers_matching_events_and_filters_names() {
        let bus = EventBus::new();
        let (id, receiver) = bus
            .subscribe(EventFilter {
                events: Some(vec!["bell".to_string()]),
                ..EventFilter::default()
            })
            .unwrap();
        bus.publish(&event("prompt", Some("1")));
        bus.publish(&event("bell", Some("1")));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap()["event"],
            "bell"
        );
        assert_eq!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        );
        bus.unsubscribe(id);
        assert_eq!(bus.subscriber_count(), 0);
    }

    #[test]
    fn filters_by_pane() {
        let bus = EventBus::new();
        let (_id, receiver) = bus
            .subscribe(EventFilter {
                pane: Some("pane-2".to_string()),
                ..EventFilter::default()
            })
            .unwrap();
        bus.publish(&event("bell", Some("pane-1")));
        bus.publish(&event("bell", Some("pane-2")));
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(1)).unwrap()["pane"],
            "pane-2"
        );
    }

    #[test]
    fn output_is_opt_in() {
        let bus = EventBus::new();
        let (_quiet_id, quiet) = bus.subscribe(EventFilter::default()).unwrap();
        let (_loud_id, loud) = bus
            .subscribe(EventFilter {
                include_output: true,
                ..EventFilter::default()
            })
            .unwrap();
        bus.publish(&event("output_line", Some("1")));
        assert_eq!(
            loud.recv_timeout(Duration::from_secs(1)).unwrap()["event"],
            "output_line"
        );
        assert_eq!(
            quiet.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn rejects_events_without_a_name_and_enforces_the_limit() {
        let bus = EventBus::new();
        bus.publish(&json!({ "pane": "1" }));
        for _ in 0..MAX_SUBSCRIBERS {
            assert!(bus.subscribe(EventFilter::default()).is_some());
        }
        assert!(bus.subscribe(EventFilter::default()).is_none());
        assert_eq!(bus.subscriber_count(), MAX_SUBSCRIBERS);
    }

    #[test]
    fn drops_a_subscriber_that_does_not_keep_up() {
        let bus = EventBus::new();
        let (_id, _receiver) = bus.subscribe(EventFilter::default()).unwrap();
        for _ in 0..=SUBSCRIBER_QUEUE {
            bus.publish(&event("bell", Some("1")));
        }
        assert_eq!(bus.subscriber_count(), 0);
    }
}
