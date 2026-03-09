//! Buffered streaming decoder for newline-delimited JSON streams.
//!
//! Handles the case where JSON objects are split across TCP chunk boundaries,
//! which is a common issue with Ollama's streaming API.

use serde_json::Value;

use crate::output_parser::streaming::auto_complete_json;

/// Buffered decoder for newline-delimited JSON streams (NDJSON).
///
/// Accumulates raw bytes, splits on newline boundaries, and yields
/// complete JSON lines. Handles the common case where a single JSON
/// object is split across multiple network chunks.
///
/// # Example
///
/// ```
/// use llm_pipeline::StreamingDecoder;
///
/// let mut decoder = StreamingDecoder::new();
///
/// // First chunk: partial JSON
/// let values = decoder.decode(b"{\"response\":");
/// assert!(values.is_empty());
///
/// // Second chunk: completes the line
/// let values = decoder.decode(b"\"hello\"}\n");
/// assert_eq!(values.len(), 1);
/// assert_eq!(values[0]["response"], "hello");
/// ```
pub struct StreamingDecoder {
    buffer: String,
}

impl StreamingDecoder {
    /// Create a new empty decoder.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed a raw chunk into the decoder and return any complete JSON lines.
    ///
    /// Each returned value is a parsed JSON `Value` from one complete line.
    /// Incomplete lines are buffered until the next chunk arrives.
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
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                values.push(val);
            }
        }

        values
    }

    /// Flush remaining buffer content, attempting to parse it as JSON.
    ///
    /// Call this after the stream ends to handle any trailing data
    /// not terminated by a newline. If direct parsing fails, attempts
    /// auto-completion of truncated JSON (closing unclosed strings,
    /// brackets, and braces).
    pub fn flush(&mut self) -> Option<Value> {
        let remaining = self.buffer.trim().to_string();
        self.buffer.clear();
        if remaining.is_empty() {
            return None;
        }
        // Try direct parse first
        if let Ok(val) = serde_json::from_str::<Value>(&remaining) {
            return Some(val);
        }
        // Try auto-completing truncated JSON
        if let Some(completed) = auto_complete_json(&remaining) {
            return serde_json::from_str::<Value>(&completed).ok();
        }
        None
    }
}

impl Default for StreamingDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_complete_lines() {
        let mut decoder = StreamingDecoder::new();
        let chunk = b"{\"response\":\"hello\"}\n{\"response\":\"world\"}\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["response"], "hello");
        assert_eq!(values[1]["response"], "world");
    }

    #[test]
    fn test_split_across_chunks() {
        let mut decoder = StreamingDecoder::new();

        // First chunk: partial JSON line
        let values1 = decoder.decode(b"{\"response\":");
        assert!(values1.is_empty());

        // Second chunk: completes the line
        let values2 = decoder.decode(b"\"hello\"}\n");
        assert_eq!(values2.len(), 1);
        assert_eq!(values2[0]["response"], "hello");
    }

    #[test]
    fn test_split_mid_value() {
        let mut decoder = StreamingDecoder::new();

        let v1 = decoder.decode(b"{\"response\":\"hel");
        assert!(v1.is_empty());

        let v2 = decoder.decode(b"lo wor");
        assert!(v2.is_empty());

        let v3 = decoder.decode(b"ld\"}\n");
        assert_eq!(v3.len(), 1);
        assert_eq!(v3[0]["response"], "hello world");
    }

    #[test]
    fn test_multiple_chunks_multiple_lines() {
        let mut decoder = StreamingDecoder::new();

        // Chunk contains end of one line and start of another
        let v1 = decoder.decode(b"{\"a\":1}\n{\"b\":");
        assert_eq!(v1.len(), 1);
        assert_eq!(v1[0]["a"], 1);

        let v2 = decoder.decode(b"2}\n");
        assert_eq!(v2.len(), 1);
        assert_eq!(v2[0]["b"], 2);
    }

    #[test]
    fn test_empty_chunks() {
        let mut decoder = StreamingDecoder::new();
        let v = decoder.decode(b"");
        assert!(v.is_empty());
        let v = decoder.decode(b"\n\n");
        assert!(v.is_empty());
    }

    #[test]
    fn test_flush_remaining() {
        let mut decoder = StreamingDecoder::new();
        decoder.decode(b"{\"done\":true}");
        // No newline, so nothing returned yet
        let flushed = decoder.flush();
        assert!(flushed.is_some());
        assert_eq!(flushed.unwrap()["done"], json!(true));
    }

    #[test]
    fn test_flush_empty() {
        let mut decoder = StreamingDecoder::new();
        assert!(decoder.flush().is_none());
    }

    #[test]
    fn test_ollama_streaming_simulation() {
        let mut decoder = StreamingDecoder::new();

        let full_stream = concat!(
            "{\"model\":\"llama3\",\"response\":\"Hello\"}\n",
            "{\"model\":\"llama3\",\"response\":\" world\"}\n",
            "{\"model\":\"llama3\",\"response\":\"!\",\"done\":true}\n",
        );
        let bytes = full_stream.as_bytes();

        // Split at awkward positions that cross JSON boundaries
        let mut all_values = Vec::new();
        let splits = [15, 37, 60, bytes.len()];
        let mut start = 0;
        for &end in &splits {
            let end = end.min(bytes.len());
            let chunk = &bytes[start..end];
            all_values.extend(decoder.decode(chunk));
            start = end;
        }

        assert_eq!(all_values.len(), 3);
        assert_eq!(all_values[0]["response"], "Hello");
        assert_eq!(all_values[1]["response"], " world");
        assert_eq!(all_values[2]["response"], "!");
        assert_eq!(all_values[2]["done"], json!(true));
    }

    #[test]
    fn test_streaming_decoder_flush_recovers_truncated() {
        let mut decoder = StreamingDecoder::new();
        // Feed truncated JSON (no newline, unclosed brace)
        decoder.decode(b"{\"name\": \"Alice\", \"age\": 30");
        let flushed = decoder.flush();
        assert!(flushed.is_some());
        let val = flushed.unwrap();
        assert_eq!(val["name"], "Alice");
        assert_eq!(val["age"], 30);
    }

    #[test]
    fn test_non_json_lines_skipped() {
        let mut decoder = StreamingDecoder::new();
        let chunk = b"not json\n{\"ok\":true}\ngarbage\n";
        let values = decoder.decode(chunk);
        assert_eq!(values.len(), 1);
        assert_eq!(values[0]["ok"], json!(true));
    }
}
