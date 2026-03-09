//! Re-exports from the standalone [`llm-output-parser`] crate.
//!
//! The parser implementation now lives in the separate `llm-output-parser` crate.
//! This module re-exports its public API for backward compatibility so existing
//! `use llm_pipeline::output_parser::parse_json` paths continue to work.

pub use llm_output_parser::*;

/// Streaming JSON parser with auto-completion for truncated output.
///
/// These utilities are pipeline-specific and not part of the standalone
/// `llm-output-parser` crate.
pub mod streaming {
    use serde_json::Value;

    /// Auto-complete a truncated JSON string by closing unclosed delimiters.
    ///
    /// Handles:
    /// - Unclosed strings (adds `"`)
    /// - Unclosed objects (adds `}`)
    /// - Unclosed arrays (adds `]`)
    /// - Nested combinations
    /// - Trailing commas before closing
    /// - Think tags (strips them first)
    ///
    /// Returns `Some(valid_json_string)` if completion was possible,
    /// or `None` if the input doesn't look like JSON at all.
    ///
    /// # Example
    ///
    /// ```
    /// use llm_pipeline::output_parser::streaming::auto_complete_json;
    ///
    /// let completed = auto_complete_json(r#"{"name": "Alice", "age": 3"#).unwrap();
    /// let v: serde_json::Value = serde_json::from_str(&completed).unwrap();
    /// assert_eq!(v["name"], "Alice");
    /// ```
    pub fn auto_complete_json(input: &str) -> Option<String> {
        // Strip think tags first
        let cleaned = llm_output_parser::strip_think_tags(input);
        let trimmed = cleaned.trim();

        if trimmed.is_empty() {
            return None;
        }

        // Already valid JSON? Return as-is.
        if serde_json::from_str::<Value>(trimmed).is_ok() {
            return Some(trimmed.to_string());
        }

        // Must start with { or [ to be JSON
        if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
            return None;
        }

        let mut result = String::with_capacity(trimmed.len() + 16);
        let mut stack: Vec<char> = Vec::new();
        let mut in_string = false;
        let mut escaped = false;

        for ch in trimmed.chars() {
            result.push(ch);

            if escaped {
                escaped = false;
                continue;
            }

            if ch == '\\' && in_string {
                escaped = true;
                continue;
            }

            if ch == '"' {
                in_string = !in_string;
                continue;
            }

            if in_string {
                continue;
            }

            match ch {
                '{' => stack.push('}'),
                '[' => stack.push(']'),
                '}' | ']' => {
                    if let Some(expected) = stack.last() {
                        if *expected == ch {
                            stack.pop();
                        }
                    }
                }
                _ => {}
            }
        }

        // If we're inside a string, close it
        if in_string {
            result.push('"');
        }

        // Clean up trailing incomplete key-value pairs before closing.
        loop {
            let t = result.trim_end();
            if t.ends_with(',') {
                result = t.strip_suffix(',').unwrap().to_string();
            } else if let Some(before_colon) = t.strip_suffix(':') {
                let without_colon = before_colon.trim_end();
                if let Some(quote_pos) = without_colon.rfind('"') {
                    if let Some(open_pos) = without_colon[..quote_pos].rfind('"') {
                        let before_key = without_colon[..open_pos].trim_end();
                        result = if let Some(stripped) = before_key.strip_suffix(',') {
                            stripped.to_string()
                        } else {
                            before_key.to_string()
                        };
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Check for orphan key at the end
        if stack.last() == Some(&'}') {
            let t = result.trim_end();
            if t.ends_with('"') && !t.ends_with("\\\"") {
                let inner = &t[..t.len() - 1];
                if let Some(open_pos) = inner.rfind('"') {
                    let before = inner[..open_pos].trim_end();
                    if before.ends_with(',') {
                        result = before.strip_suffix(',').unwrap().to_string();
                    }
                }
            }
        }

        // Close all unclosed delimiters
        while let Some(closer) = stack.pop() {
            result.push(closer);
        }

        // Verify the result is valid JSON
        if serde_json::from_str::<Value>(&result).is_ok() {
            Some(result)
        } else {
            None
        }
    }

    /// Progressive JSON parser for streaming LLM output.
    ///
    /// Tracks the accumulating text during a streaming call and caches
    /// the last successful JSON parse. Useful for partial results displays.
    ///
    /// # Example
    ///
    /// ```
    /// use llm_pipeline::output_parser::streaming::StreamingJsonParser;
    ///
    /// let mut parser = StreamingJsonParser::new();
    ///
    /// parser.push(r#"{"name": "#);
    /// // Auto-complete may produce a partial value here
    ///
    /// parser.push(r#""Alice", "age": 30}"#);
    /// let val = parser.current_value().unwrap();
    /// assert_eq!(val["name"], "Alice");
    /// ```
    #[derive(Debug)]
    pub struct StreamingJsonParser {
        buffer: String,
        cached_value: Option<Value>,
        last_parsed_len: usize,
    }

    impl StreamingJsonParser {
        /// Create a new empty streaming parser.
        pub fn new() -> Self {
            Self {
                buffer: String::new(),
                cached_value: None,
                last_parsed_len: 0,
            }
        }

        /// Append new text to the buffer and attempt to parse.
        pub fn push(&mut self, text: &str) {
            self.buffer.push_str(text);
            self.try_parse();
        }

        /// Get the current parsed value, if any.
        pub fn current_value(&self) -> Option<&Value> {
            self.cached_value.as_ref()
        }

        /// Get the raw accumulated text.
        pub fn buffer(&self) -> &str {
            &self.buffer
        }

        /// Clear the parser state.
        pub fn clear(&mut self) {
            self.buffer.clear();
            self.cached_value = None;
            self.last_parsed_len = 0;
        }

        /// Try to parse the current buffer content.
        fn try_parse(&mut self) {
            if self.buffer.len() == self.last_parsed_len {
                return;
            }

            let trimmed = self.buffer.trim();

            if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
                self.cached_value = Some(val);
                self.last_parsed_len = self.buffer.len();
                return;
            }

            if let Some(completed) = auto_complete_json(trimmed) {
                if let Ok(val) = serde_json::from_str::<Value>(&completed) {
                    self.cached_value = Some(val);
                }
            }

            self.last_parsed_len = self.buffer.len();
        }
    }

    impl Default for StreamingJsonParser {
        fn default() -> Self {
            Self::new()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_auto_complete_closes_string() {
            let result = auto_complete_json(r#"{"name": "Alice"#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["name"], "Alice");
        }

        #[test]
        fn test_auto_complete_closes_brace() {
            let result = auto_complete_json(r#"{"key": "value""#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["key"], "value");
        }

        #[test]
        fn test_auto_complete_closes_bracket() {
            let result = auto_complete_json(r#"["a", "b", "c""#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            let arr = v.as_array().unwrap();
            assert_eq!(arr.len(), 3);
        }

        #[test]
        fn test_auto_complete_nested() {
            let result = auto_complete_json(r#"{"items": [{"name": "a"}, {"name": "b""#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert!(v["items"].is_array());
        }

        #[test]
        fn test_auto_complete_with_null_fill() {
            let result = auto_complete_json(r#"{"name": "Alice", "age": "#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["name"], "Alice");
        }

        #[test]
        fn test_auto_complete_already_valid() {
            let input = r#"{"complete": true}"#;
            let result = auto_complete_json(input).unwrap();
            assert_eq!(result, input);
        }

        #[test]
        fn test_auto_complete_not_json() {
            assert!(auto_complete_json("just plain text").is_none());
            assert!(auto_complete_json("").is_none());
        }

        #[test]
        fn test_auto_complete_with_think_tags() {
            let input = r#"<think>hmm</think>{"key": "val"#;
            let result = auto_complete_json(input).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["key"], "val");
        }

        #[test]
        fn test_streaming_parser_progressive() {
            let mut parser = StreamingJsonParser::new();

            parser.push(r#"{"name""#);

            parser.push(r#": "Alice", "age": 30}"#);
            let val = parser.current_value().unwrap();
            assert_eq!(val["name"], "Alice");
            assert_eq!(val["age"], 30);
        }

        #[test]
        fn test_streaming_parser_cache_no_reparse() {
            let mut parser = StreamingJsonParser::new();
            parser.push(r#"{"complete": true}"#);

            let val1 = parser.current_value().cloned();
            let val2 = parser.current_value().cloned();
            assert_eq!(val1, val2);
        }

        #[test]
        fn test_streaming_parser_clear_resets() {
            let mut parser = StreamingJsonParser::new();
            parser.push(r#"{"a": 1}"#);
            assert!(parser.current_value().is_some());

            parser.clear();
            assert!(parser.current_value().is_none());
            assert!(parser.buffer().is_empty());
        }

        #[test]
        fn test_json_parser_recovers_truncated_object() {
            let result =
                auto_complete_json(r#"{"title": "Matrix", "year": 1999, "rating"#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["title"], "Matrix");
            assert_eq!(v["year"], 1999);
        }

        #[test]
        fn test_json_parser_recovers_truncated_array() {
            let result = auto_complete_json(r#"[1, 2, 3, "#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            let arr = v.as_array().unwrap();
            assert!(arr.len() >= 3);
        }

        #[test]
        fn test_json_parser_truncated_in_string() {
            let result = auto_complete_json(r#"{"msg": "hello wor"#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert!(v["msg"].as_str().unwrap().starts_with("hello wor"));
        }

        #[test]
        fn test_auto_complete_trailing_comma() {
            let result = auto_complete_json(r#"{"a": 1, "b": 2,"#).unwrap();
            let v: Value = serde_json::from_str(&result).unwrap();
            assert_eq!(v["a"], 1);
            assert_eq!(v["b"], 2);
        }
    }
}
