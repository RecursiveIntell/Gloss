//! Incremental SSE framing. Network chunks are bytes, not UTF-8 or event boundaries.
//! JSON interpretation belongs to each provider. Malformed/oversized frames fail
//! visibly rather than losing tokens. No reconnect or retry is performed here.

const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Default)]
pub(super) struct SseDecoder {
    line: Vec<u8>,
    data: String,
    event_bytes: usize,
    skip_lf: bool,
    started: bool,
    failed: bool,
}

impl SseDecoder {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, &'static str> {
        if self.failed {
            return Err("SSE decoder already failed");
        }
        let result = self.decode(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn decode(&mut self, bytes: &[u8]) -> Result<Vec<String>, &'static str> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.finish_line(&mut events)?;
                self.skip_lf = byte == b'\r';
            } else {
                self.event_bytes += 1;
                if self.event_bytes > MAX_EVENT_BYTES {
                    return Err("SSE event exceeds 1 MiB limit");
                }
                self.line.push(byte);
            }
        }
        Ok(events)
    }

    fn finish_line(&mut self, events: &mut Vec<String>) -> Result<(), &'static str> {
        let line = std::str::from_utf8(&self.line).map_err(|_| "Invalid UTF-8 in SSE frame")?;
        let line = if self.started {
            line
        } else {
            self.started = true;
            line.strip_prefix('\u{feff}').unwrap_or(line)
        };
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop(); // The SSE algorithm strips the final added newline.
                events.push(std::mem::take(&mut self.data));
            }
            self.event_bytes = 0;
        } else if !line.starts_with(':') {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            if field == "data" {
                self.data.push_str(value);
                self.data.push('\n');
            }
        }
        self.line.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chunk_boundary_preserves_utf8_and_framing() {
        let wire = "\u{feff}: hello\r\ndata:{\"text\":\"你好 🦀\"}\r\n\r\ndata: [DONE]\n\n";
        for split in 0..=wire.len() {
            let mut parser = SseDecoder::new();
            let mut events = parser.push(&wire.as_bytes()[..split]).unwrap();
            events.extend(parser.push(&wire.as_bytes()[split..]).unwrap());
            assert_eq!(events, ["{\"text\":\"你好 🦀\"}", "[DONE]"]);
        }
    }

    #[test]
    fn supports_multiline_data_cr_and_optional_space() {
        let mut parser = SseDecoder::new();
        assert_eq!(
            parser
                .push(b"event: delta\rdata:one\rdata: two\r\r")
                .unwrap(),
            ["one\ntwo"]
        );
        assert_eq!(
            parser.push(b"data:  keep leading space\n\n").unwrap(),
            [" keep leading space"]
        );
    }

    #[test]
    fn incomplete_event_is_not_dispatched() {
        let mut parser = SseDecoder::new();
        assert!(parser.push(b"data: pending\n").unwrap().is_empty());
        assert_eq!(parser.push(b"\n").unwrap(), ["pending"]);
    }

    #[test]
    fn rejects_invalid_utf8_and_poisoned_reuse() {
        let mut parser = SseDecoder::new();
        assert!(parser
            .push(b"data: \xff\n\n")
            .unwrap_err()
            .contains("UTF-8"));
        assert!(parser.push(b"data: x\n\n").is_err());
    }

    #[test]
    fn bounds_unterminated_lines_and_multiline_frames() {
        let mut parser = SseDecoder::new();
        assert!(parser.push(&vec![b'x'; MAX_EVENT_BYTES + 1]).is_err());
        let mut parser = SseDecoder::new();
        for _ in 0..MAX_EVENT_BYTES / 1024 {
            parser.push(b"data:").unwrap();
            if parser.push(&vec![b'x'; 1019]).is_err() {
                return;
            }
            parser.push(b"\n").unwrap();
        }
        assert!(parser.push(b"data:x\n").is_err());
    }

    #[test]
    fn empty_data_and_unknown_fields_follow_sse_rules() {
        let mut parser = SseDecoder::new();
        assert_eq!(
            parser
                .push(b"id: 1\nretry: 100\n:heartbeat\ndata\n\n")
                .unwrap(),
            [""]
        );
        assert!(parser.push(b"event: ping\n\n").unwrap().is_empty());
    }
}
