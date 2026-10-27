//! Length-prefixed JSON framing shared by the control API and the PTY broker.
//!
//! A frame is a 4-byte big-endian length followed by that many bytes. Limits
//! are enforced before allocating, so a peer cannot make Oxterm allocate an
//! arbitrary amount of memory merely by announcing a huge frame.

use std::io::{Read, Write};

use serde_json::Value;

/// Largest request accepted by the control API. Captures can be large, but a
/// request is just a command plus some text; 8 MiB is far more than enough.
pub const MAX_REQUEST_FRAME: usize = 8 * 1024 * 1024;

/// Largest reply sent by the control API: `capture --all` may return a few
/// megabytes of scrollback.
pub const MAX_RESPONSE_FRAME: usize = 64 * 1024 * 1024;

/// Frame limit of the PTY broker protocol.
pub const MAX_BROKER_FRAME: usize = 1024 * 1024;

/// Output buffered for a slow broker client before it is disconnected.
pub const MAX_BROKER_PENDING: usize = 4 * 1024 * 1024;

/// Concurrent broker clients.
pub const MAX_BROKER_CLIENTS: usize = 32;

/// Concurrent control API connections. Each one is handled by a thread, so this
/// bounds the threads a local process can make Oxterm spawn.
pub const MAX_CONTROL_CLIENTS: usize = 32;

/// A client that connects and sends nothing is dropped after this long.
pub const CONTROL_READ_TIMEOUT_SECS: u64 = 5;

pub fn write_json(stream: &mut impl Write, value: &Value, limit: usize) -> Result<(), String> {
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if payload.len() > limit {
        return Err(format!(
            "control frame is too large ({} bytes, limit {})",
            payload.len(),
            limit
        ));
    }
    stream
        .write_all(&(payload.len() as u32).to_be_bytes())
        .and_then(|_| stream.write_all(&payload))
        .map_err(|error| error.to_string())
}

pub fn read_json(stream: &mut impl Read, limit: usize) -> Result<Value, String> {
    let mut length = [0u8; 4];
    stream
        .read_exact(&mut length)
        .map_err(|error| error.to_string())?;
    let length = u32::from_be_bytes(length) as usize;
    if length > limit {
        return Err(format!(
            "control frame is too large ({length} bytes, limit {limit})"
        ));
    }
    let mut payload = vec![0u8; length];
    stream
        .read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&payload).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_a_frame() {
        let mut buffer = Vec::new();
        write_json(&mut buffer, &json!({"op": "ping"}), 1024).unwrap();
        let value = read_json(&mut buffer.as_slice(), 1024).unwrap();
        assert_eq!(value, json!({"op": "ping"}));
    }

    #[test]
    fn refuses_to_allocate_or_send_oversized_frames() {
        let mut buffer = Vec::new();
        let error = write_json(&mut buffer, &json!("x".repeat(4096)), 1024).unwrap_err();
        assert!(error.contains("too large"));

        // Announce a frame larger than the limit without sending its body.
        let mut frame = Vec::new();
        frame.extend_from_slice(&(64u32 * 1024 * 1024).to_be_bytes());
        let error = read_json(&mut frame.as_slice(), 1024).unwrap_err();
        assert!(error.contains("too large"));
    }

    #[test]
    fn rejects_invalid_json_and_truncated_frames() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&4u32.to_be_bytes());
        frame.extend_from_slice(b"{oops");
        assert!(read_json(&mut frame.as_slice(), 1024).is_err());

        let mut truncated = Vec::new();
        truncated.extend_from_slice(&16u32.to_be_bytes());
        truncated.extend_from_slice(b"{}");
        assert!(read_json(&mut truncated.as_slice(), 1024).is_err());
    }
}
