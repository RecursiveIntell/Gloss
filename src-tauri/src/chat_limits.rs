//! Aggregate chat-response and replay retention bounds. These are byte limits,
//! independent of per-frame transport limits and provider max_tokens promises.
use crate::error::GlossError;
use serde::Serialize;
use std::collections::VecDeque;
use std::io::Write;
use std::ops::Deref;

pub const MAX_CHAT_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
pub const CHAT_STREAM_REPLAY_BYTES: usize = 16 * 1024 * 1024;
pub const CHAT_STREAM_REPLAY_EVENTS: usize = 4096;

pub fn append_response_token(
    response: &mut String,
    token: &str,
    provider: &str,
) -> Result<(), GlossError> {
    if token.len() > MAX_CHAT_RESPONSE_BYTES.saturating_sub(response.len()) {
        return Err(GlossError::Provider {
            provider: provider.into(),
            source: anyhow::anyhow!("response_byte_limit: Chat response exceeds 8 MiB limit"),
        });
    }
    response.push_str(token);
    Ok(())
}

// Count serialized bytes without allocating a second event-sized JSON buffer.
#[derive(Default)]
struct ByteCounter(usize);
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
pub fn serialized_bytes<T: Serialize>(value: &T) -> usize {
    let mut counter = ByteCounter::default();
    if serde_json::to_writer(&mut counter, value).is_err() {
        return usize::MAX;
    }
    counter.0
}

pub struct ReplayBuffer<T> {
    events: VecDeque<T>,
    sizes: VecDeque<usize>,
    retained_bytes: usize,
}
impl<T> Default for ReplayBuffer<T> {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            sizes: VecDeque::new(),
            retained_bytes: 0,
        }
    }
}
impl<T> Deref for ReplayBuffer<T> {
    type Target = VecDeque<T>;
    fn deref(&self) -> &Self::Target {
        &self.events
    }
}
impl<T: Serialize> ReplayBuffer<T> {
    /// Returns false for an oversized single event; caller must retain a small
    /// explicit gap marker instead. Never silently retain a non-contiguous tail.
    pub fn push(&mut self, event: T) -> bool {
        let size = serialized_bytes(&event);
        if size > CHAT_STREAM_REPLAY_BYTES {
            return false;
        }
        while self.events.len() >= CHAT_STREAM_REPLAY_EVENTS
            || self.retained_bytes > CHAT_STREAM_REPLAY_BYTES - size
        {
            self.events.pop_front();
            self.retained_bytes -= self.sizes.pop_front().expect("replay byte accounting");
        }
        self.retained_bytes += size;
        self.sizes.push_back(size);
        self.events.push_back(event);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legal_frames_cannot_exceed_aggregate_response_limit() {
        let mut response = String::new();
        let frame = "x".repeat(64 * 1024);
        for _ in 0..128 {
            append_response_token(&mut response, &frame, "fixture").unwrap();
        }
        assert_eq!(response.len(), MAX_CHAT_RESPONSE_BYTES);
        assert!(append_response_token(&mut response, "é", "fixture")
            .unwrap_err()
            .to_string()
            .contains("response_byte_limit"));
        assert_eq!(
            response.len(),
            MAX_CHAT_RESPONSE_BYTES,
            "no partial token appended"
        );
        assert!(append_response_token(&mut String::new(), "next attempt", "fixture").is_ok());
    }
    #[test]
    fn replay_evicts_prefix_by_bytes_and_by_count() {
        let mut replay = ReplayBuffer::default();
        for seq in 0..32 {
            assert!(
                replay.push(serde_json::json!({"seq": seq, "payload": "x".repeat(1024 * 1024)}))
            );
        }
        assert!(replay.retained_bytes <= CHAT_STREAM_REPLAY_BYTES);
        assert!(replay.front().unwrap()["seq"].as_u64().unwrap() > 0);
        assert_eq!(replay.back().unwrap()["seq"], 31);
        assert_eq!(
            replay.retained_bytes,
            replay.iter().map(serialized_bytes).sum::<usize>()
        );
        let mut replay = ReplayBuffer::default();
        for seq in 0..CHAT_STREAM_REPLAY_EVENTS + 7 {
            assert!(replay.push(seq));
        }
        assert_eq!(replay.len(), CHAT_STREAM_REPLAY_EVENTS);
        assert_eq!(replay.front(), Some(&7));
    }
    #[test]
    fn single_oversize_event_is_rejected_and_gap_can_be_retained() {
        let mut replay = ReplayBuffer::default();
        assert!(!replay.push(serde_json::json!({"payload": "x".repeat(CHAT_STREAM_REPLAY_BYTES)})));
        assert!(replay.is_empty());
        assert!(replay.push(serde_json::json!({"kind": "gap", "seq": 1})));
        assert!(replay.retained_bytes < 100);
    }
}
