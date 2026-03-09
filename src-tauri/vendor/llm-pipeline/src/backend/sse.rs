//! SSE (Server-Sent Events) stream decoder for OpenAI-compatible APIs.
//!
//! Handles the `data: ` prefix, `[DONE]` termination, line buffering across
//! TCP chunk boundaries, and empty keep-alive lines.

use serde_json::Value;

/// SSE stream decoder for OpenAI-compatible APIs.
///
/// Handles the `data: {...}` format with `data: [DONE]` termination.
/// Properly handles multi-line SSE events and empty keep-alive lines.
///
/// # Example
///
/// ```
/// use llm_pipeline::backend::sse::SseDecoder;
///
/// let mut decoder = SseDecoder::new();
/// let data = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\ndata: [DONE]\n\n";
/// let values = decoder.decode(data);
/// assert_eq!(values.len(), 1);
/// ```
#[derive(Debug)]
pub struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    /// Create a new empty SSE decoder.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed raw bytes into the decoder and return any complete JSON payloads.
    ///
    /// Handles `data: ` prefix lines, ignores `event:` lines and empty
    /// keep-alive lines. Returns parsed JSON for each complete `data:` line
    /// (excluding the `[DONE]` terminator).
    pub fn decode(&mut self, chunk: &[u8]) -> Vec<Value> {
        let text = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&text);

        let mut values = Vec::new();

        while let Some(pos) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=pos).collect();
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            // Ignore event: lines
            if line.starts_with("event:") {
                continue;
            }

            // Process data: lines
            if let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            {
                let data = data.trim();

                // [DONE] is the termination signal
                if data == "[DONE]" {
                    continue;
                }

                if let Ok(val) = serde_json::from_str::<Value>(data) {
                    values.push(val);
                }
            }
        }

        values
    }

    /// Flush any remaining buffer content.
    pub fn flush(&mut self) -> Vec<Value> {
        let remaining = self.buffer.trim().to_string();
        self.buffer.clear();

        if remaining.is_empty() {
            return Vec::new();
        }

        let mut values = Vec::new();
        for line in remaining.lines() {
            let line = line.trim();
            if let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            {
                let data = data.trim();
                if data != "[DONE]" {
                    if let Ok(val) = serde_json::from_str::<Value>(data) {
                        values.push(val);
                    }
                }
            }
        }

        values
    }
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_basic_decode() {
        let mut decoder = SseDecoder::new();
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 1);
        assert!(values[0]["choices"][0]["delta"]["content"]
            .as_str()
            .is_some());
    }

    #[test]
    fn test_sse_done_ignored() {
        let mut decoder = SseDecoder::new();
        let chunk = b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\ndata: [DONE]\n\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn test_sse_empty_lines_ignored() {
        let mut decoder = SseDecoder::new();
        let chunk = b"\n\n\ndata: {\"x\":1}\n\n\n\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["x"], 1);
    }

    #[test]
    fn test_sse_event_lines_ignored() {
        let mut decoder = SseDecoder::new();
        let chunk = b"event: message\ndata: {\"x\":1}\n\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn test_sse_split_across_chunks() {
        let mut decoder = SseDecoder::new();

        let v1 = decoder.decode(b"data: {\"cho");
        assert!(v1.is_empty());

        let v2 = decoder.decode(b"ices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        assert_eq!(v2.len(), 1);
    }

    #[test]
    fn test_sse_multiple_events() {
        let mut decoder = SseDecoder::new();
        let chunk = b"data: {\"a\":1}\n\ndata: {\"a\":2}\n\ndata: {\"a\":3}\n\ndata: [DONE]\n\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 3);
    }
}
