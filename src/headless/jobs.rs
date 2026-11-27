//! Honest receipts for text handed to a terminal.
//!
//! `oxterm send` only proves that the GUI accepted the bytes. A script that
//! chains on that acknowledgement can wrongly believe a command ran. A receipt
//! names what was actually observed: acceptance by the terminal, expected
//! output seen afterwards, or no proof either way. Nothing here touches GTK or
//! the terminal: the caller decides what evidence is available.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// How much of a send we can actually stand behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptState {
    /// The GUI accepted the bytes; no output was checked.
    HandedToTerminal,
    /// The requested output was observed after the send.
    Delivered,
    /// The send succeeded but the requested output never appeared. The command
    /// may still have run: absence of evidence is not evidence of failure.
    Unconfirmed,
    /// The send itself failed.
    NotDelivered,
}

impl ReceiptState {
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptState::HandedToTerminal => "handed_to_terminal",
            ReceiptState::Delivered => "delivered",
            ReceiptState::Unconfirmed => "unconfirmed",
            ReceiptState::NotDelivered => "not_delivered",
        }
    }
}

/// A single, machine-readable result of one `send`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Receipt {
    pub id: String,
    pub pane: String,
    pub state: ReceiptState,
    pub detail: String,
}

impl Receipt {
    pub fn new(pane: &str, state: ReceiptState, detail: impl Into<String>) -> Receipt {
        Receipt {
            id: new_id(),
            pane: pane.to_string(),
            state,
            detail: detail.into(),
        }
    }

    /// One JSON line, safe for a script to parse with `jq`.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// An id a caller can use to correlate a later `capture` or agent action with
/// this send. Unique within a process and stable across restarts in the sense
/// that it is never reused by a second send in the same run.
pub fn new_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("job-{nanos}-{sequence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_name_what_was_observed() {
        let receipt = Receipt::new("pane-1", ReceiptState::Delivered, "saw ready");
        assert!(receipt.id.starts_with("job-"));
        assert_eq!(receipt.pane, "pane-1");
        let json = receipt.to_json();
        assert!(json.contains("\"state\":\"delivered\""), "{json}");
        assert!(json.contains("\"id\":\"job-"), "{json}");
        assert_eq!(ReceiptState::Unconfirmed.as_str(), "unconfirmed");
        assert_eq!(ReceiptState::NotDelivered.as_str(), "not_delivered");
    }

    #[test]
    fn ids_are_unique_within_a_process() {
        let first = new_id();
        let second = new_id();
        assert_ne!(first, second);
    }
}
