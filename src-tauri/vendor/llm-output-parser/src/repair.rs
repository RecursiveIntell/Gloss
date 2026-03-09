//! Deterministic JSON repair for common LLM output errors.
//!
//! Applies a sequence of heuristic fixes to malformed JSON strings produced
//! by LLMs, then validates the result with `serde_json`. No regex is used —
//! all repairs are manual string operations.

/// Attempt to repair common LLM JSON mistakes without calling the model again.
///
/// Returns the repaired string if any fixes were applied and the result
/// is valid JSON. Returns `None` if repair was not possible or the input
/// was already valid.
///
/// Repairs applied (in order):
/// 1. Strip inline comments (`//` and `/* */`)
/// 2. Replace Python booleans/None: `True`->`true`, `False`->`false`, `None`->`null`
/// 3. Remove trailing commas before `}` or `]`
/// 4. Replace single-quoted strings with double-quoted
/// 5. Quote unquoted object keys
/// 6. Append missing closing brackets/braces
/// 7. Escape raw newlines inside string values
pub fn try_repair_json(broken: &str) -> Option<String> {
    // If already valid, no repair needed
    if serde_json::from_str::<serde_json::Value>(broken).is_ok() {
        return None;
    }

    let mut s = broken.to_string();
    s = strip_comments(&s);
    s = replace_python_literals(&s);
    s = remove_trailing_commas(&s);
    s = replace_single_quotes(&s);
    s = quote_unquoted_keys(&s);
    s = close_missing_brackets(&s);
    s = escape_raw_newlines(&s);

    // Validate the result
    if serde_json::from_str::<serde_json::Value>(&s).is_ok() {
        Some(s)
    } else {
        None
    }
}

/// Strip `// ...` and `/* ... */` comments, avoiding strings.
fn strip_comments(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_string {
            if chars[i] == '\\' {
                escape_next = true;
                result.push(chars[i]);
            } else if chars[i] == '"' {
                in_string = false;
                result.push(chars[i]);
            } else {
                result.push(chars[i]);
            }
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Check for //
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '/' {
            // Skip to end of line
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }

        // Check for /* */
        if i + 1 < len && chars[i] == '/' && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2; // skip */
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Replace Python-style `True`, `False`, `None` with JSON equivalents.
/// Only replaces when not inside a quoted string.
fn replace_python_literals(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_string {
            if chars[i] == '\\' {
                escape_next = true;
            } else if chars[i] == '"' {
                in_string = false;
            }
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // Check word boundaries for replacement
        if let Some((replacement, skip)) = try_replace_word(&chars, i, "True", "true")
            .or_else(|| try_replace_word(&chars, i, "False", "false"))
            .or_else(|| try_replace_word(&chars, i, "None", "null"))
        {
            result.push_str(replacement);
            i += skip;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Try to match and replace a word at position `i` with word-boundary checking.
fn try_replace_word<'a>(
    chars: &[char],
    i: usize,
    from: &str,
    to: &'a str,
) -> Option<(&'a str, usize)> {
    let from_chars: Vec<char> = from.chars().collect();
    let len = from_chars.len();

    if i + len > chars.len() {
        return None;
    }

    // Check that the characters match
    for (j, &fc) in from_chars.iter().enumerate() {
        if chars[i + j] != fc {
            return None;
        }
    }

    // Check word boundary before
    if i > 0 && chars[i - 1].is_alphanumeric() {
        return None;
    }

    // Check word boundary after
    if i + len < chars.len() && chars[i + len].is_alphanumeric() {
        return None;
    }

    Some((to, len))
}

/// Remove trailing commas before `}` or `]`.
fn remove_trailing_commas(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_string {
            if chars[i] == '\\' {
                escape_next = true;
            } else if chars[i] == '"' {
                in_string = false;
            }
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == ',' {
            // Look ahead past whitespace for } or ]
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '}' || chars[j] == ']') {
                // Skip the comma (trailing comma)
                i += 1;
                continue;
            }
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Replace single-quoted strings with double-quoted strings.
/// Conservative: only replace when quotes appear at string boundaries.
fn replace_single_quotes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_double_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_double_string {
            if chars[i] == '\\' {
                escape_next = true;
            } else if chars[i] == '"' {
                in_double_string = false;
            }
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_double_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '\'' {
            // Check if this looks like a string boundary
            if is_string_boundary_before(&chars, i) {
                // Find the closing single quote
                if let Some(close) = find_closing_single_quote(&chars, i + 1) {
                    // Check that the closing quote is at a boundary
                    if is_string_boundary_after(&chars, close) {
                        result.push('"');
                        // Copy content, escaping any embedded double quotes
                        for &ch in &chars[i + 1..close] {
                            if ch == '"' {
                                result.push('\\');
                            }
                            result.push(ch);
                        }
                        result.push('"');
                        i = close + 1;
                        continue;
                    }
                }
            }
            result.push(chars[i]);
            i += 1;
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Check if the character before position `i` suggests a string boundary.
fn is_string_boundary_before(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let mut j = i - 1;
    // Skip whitespace backwards
    while j > 0 && chars[j].is_whitespace() {
        j -= 1;
    }
    if chars[j].is_whitespace() && j == 0 {
        return true;
    }
    matches!(chars[j], '{' | '[' | ':' | ',')
}

/// Check if the character after position `i` suggests a string boundary.
fn is_string_boundary_after(chars: &[char], i: usize) -> bool {
    if i + 1 >= chars.len() {
        return true;
    }
    let mut j = i + 1;
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    if j >= chars.len() {
        return true;
    }
    matches!(chars[j], '}' | ']' | ':' | ',')
}

/// Find the closing single quote, handling escaped quotes.
fn find_closing_single_quote(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == '\'' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Quote unquoted object keys: `{key: "value"}` -> `{"key": "value"}`.
fn quote_unquoted_keys(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_string {
            if chars[i] == '\\' {
                escape_next = true;
            } else if chars[i] == '"' {
                in_string = false;
            }
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        // After { or , we might have an unquoted key
        if chars[i] == '{' || chars[i] == ',' {
            result.push(chars[i]);
            i += 1;
            // Skip whitespace
            while i < len && chars[i].is_whitespace() {
                result.push(chars[i]);
                i += 1;
            }
            // Check if next is an unquoted identifier followed by :
            if i < len && (chars[i].is_alphabetic() || chars[i] == '_') {
                let key_start = i;
                while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let key_end = i;
                // Skip whitespace after key
                while i < len && chars[i].is_whitespace() {
                    i += 1;
                }
                if i < len && chars[i] == ':' {
                    // This is an unquoted key
                    result.push('"');
                    for &ch in &chars[key_start..key_end] {
                        result.push(ch);
                    }
                    result.push('"');
                    // Push any whitespace that was between key and colon
                    // Actually we already consumed it, just continue
                } else {
                    // Not a key:value pattern, push the chars back
                    for &ch in &chars[key_start..i] {
                        result.push(ch);
                    }
                }
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }
    result
}

/// Append missing closing brackets/braces.
fn close_missing_brackets(s: &str) -> String {
    let mut open_braces = 0i32;
    let mut open_brackets = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in s.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => open_braces += 1,
            '}' => open_braces -= 1,
            '[' => open_brackets += 1,
            ']' => open_brackets -= 1,
            _ => {}
        }
    }

    let mut result = s.to_string();
    for _ in 0..open_brackets.max(0) {
        result.push(']');
    }
    for _ in 0..open_braces.max(0) {
        result.push('}');
    }
    result
}

/// Escape raw newlines inside string values.
fn escape_raw_newlines(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_string = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            result.push(chars[i]);
            i += 1;
            continue;
        }

        if in_string {
            if chars[i] == '\\' {
                escape_next = true;
                result.push(chars[i]);
            } else if chars[i] == '"' {
                in_string = false;
                result.push(chars[i]);
            } else if chars[i] == '\n' {
                result.push_str("\\n");
            } else if chars[i] == '\r' {
                result.push_str("\\r");
            } else {
                result.push(chars[i]);
            }
            i += 1;
            continue;
        }

        if chars[i] == '"' {
            in_string = true;
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_comma_object() {
        let input = r#"{"a": 1, "b": 2,}"#;
        let result = try_repair_json(input).unwrap();
        assert_eq!(result, r#"{"a": 1, "b": 2}"#);
    }

    #[test]
    fn trailing_comma_array() {
        let input = "[1, 2, 3,]";
        let result = try_repair_json(input).unwrap();
        assert_eq!(result, "[1, 2, 3]");
    }

    #[test]
    fn single_quotes() {
        let input = "{'key': 'value'}";
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn python_booleans() {
        let input = r#"{"active": True}"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["active"], true);
    }

    #[test]
    fn python_none() {
        let input = r#"{"data": None}"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["data"].is_null());
    }

    #[test]
    fn unquoted_keys() {
        let input = r#"{name: "Josh", age: 30}"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["name"], "Josh");
        assert_eq!(parsed["age"], 30);
    }

    #[test]
    fn inline_comment() {
        let input = r#"{"a": 1} // comment"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn missing_close_brace() {
        let input = r#"{"a": 1"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn missing_close_bracket() {
        let input = "[1, 2, 3";
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
    }

    #[test]
    fn mixed_errors() {
        let input = "{'a': True, 'b': None,}";
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], true);
        assert!(parsed["b"].is_null());
    }

    #[test]
    fn no_repair_needed() {
        let input = r#"{"a": 1}"#;
        assert!(try_repair_json(input).is_none());
    }

    #[test]
    fn unrepairable() {
        let input = "not json at all";
        assert!(try_repair_json(input).is_none());
    }

    #[test]
    fn apostrophe_safety() {
        let input = r#"{"text": "don't stop"}"#;
        // Already valid JSON, should return None
        assert!(try_repair_json(input).is_none());
    }

    #[test]
    fn nested_trailing_commas() {
        let input = r#"{"a": [1, 2,], "b": 3,}"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["b"], 3);
    }

    #[test]
    fn block_comment() {
        let input = r#"{"a": /* comment */ 1}"#;
        let result = try_repair_json(input).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn single_quoted_array() {
        let input = "['tag1', 'tag2', 'tag3']";
        let result = try_repair_json(input).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed, vec!["tag1", "tag2", "tag3"]);
    }
}
