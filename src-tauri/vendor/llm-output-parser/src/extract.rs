//! Shared extraction strategies for LLM output parsing.
//!
//! This is the load-bearing module — every parser calls into these functions
//! for preprocessing, code block extraction, and bracket matching.

use crate::error::ParseError;

/// Full preprocessing pipeline applied to every LLM response.
///
/// Strips `<think>` and `<thinking>` blocks, then trims whitespace.
/// Every parser module calls this as step 1.
pub fn preprocess(text: &str) -> String {
    let stripped = strip_think_tags(text);
    stripped.trim().to_string()
}

/// Preprocess with configurable think-tag stripping.
///
/// When `strip` is false, only trims whitespace.
pub(crate) fn preprocess_opts(text: &str, strip: bool) -> String {
    if strip {
        preprocess(text)
    } else {
        text.trim().to_string()
    }
}

/// Strip all `<think>...</think>` and `<thinking>...</thinking>` blocks from text.
///
/// Handles complete blocks, incomplete blocks (no closing tag),
/// and multiple sequential blocks.
///
/// # Examples
///
/// ```
/// use llm_output_parser::strip_think_tags;
///
/// assert_eq!(strip_think_tags("<think>reasoning</think>result"), "result");
/// assert_eq!(strip_think_tags("<think>no closing tag"), "");
/// assert_eq!(strip_think_tags("<thinking>also works</thinking>done"), "done");
/// ```
pub fn strip_think_tags(text: &str) -> String {
    let mut result = strip_tag_variant(text, "<think>", "</think>");
    result = strip_tag_variant(&result, "<thinking>", "</thinking>");
    result
}

/// Strip a specific open/close tag pair from text.
fn strip_tag_variant(text: &str, open: &str, close: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(open) {
        if let Some(end_offset) = result[start..].find(close) {
            let end = start + end_offset + close.len();
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            // No closing tag — strip from open tag to end
            result = result[..start].to_string();
            break;
        }
    }
    result
}

/// Extract content from the first matching markdown code block.
///
/// Searches for `` ```lang `` and bare `` ``` `` fences.
/// Returns `(language_hint, content)` where hint is `None` for bare fences.
///
/// # Examples
///
/// ```
/// use llm_output_parser::extract::extract_code_block;
///
/// let input = "Here:\n```json\n{\"a\": 1}\n```";
/// let (lang, content) = extract_code_block(input).unwrap();
/// assert_eq!(lang, Some("json"));
/// assert_eq!(content, "{\"a\": 1}");
/// ```
pub fn extract_code_block(text: &str) -> Option<(Option<&str>, &str)> {
    let mut search_from = 0;
    while let Some(fence_start) = text[search_from..].find("```") {
        let abs_fence = search_from + fence_start;
        let after_backticks = abs_fence + 3;

        // Determine language hint: everything between ``` and the next newline
        let line_end = text[after_backticks..].find('\n')?;
        let lang_str = text[after_backticks..after_backticks + line_end].trim();
        let lang = if lang_str.is_empty() {
            None
        } else {
            Some(lang_str)
        };

        let content_start = after_backticks + line_end + 1;

        // Find the closing ```
        if let Some(close_offset) = text[content_start..].find("```") {
            let content = text[content_start..content_start + close_offset].trim();
            return Some((lang, content));
        }

        search_from = after_backticks;
    }
    None
}

/// Extract content from a code block matching a specific language.
///
/// e.g., `extract_code_block_for(text, "json")` looks for `` ```json `` blocks.
/// Falls back to any code block if no language-specific block is found.
///
/// # Examples
///
/// ```
/// use llm_output_parser::extract::extract_code_block_for;
///
/// let input = "```json\n[1, 2, 3]\n```";
/// assert_eq!(extract_code_block_for(input, "json"), Some("[1, 2, 3]"));
/// ```
pub fn extract_code_block_for<'a>(text: &'a str, lang: &str) -> Option<&'a str> {
    // First pass: look for a block with the matching language
    let mut search_from = 0;
    while let Some(fence_start) = text[search_from..].find("```") {
        let abs_fence = search_from + fence_start;
        let after_backticks = abs_fence + 3;

        if let Some(line_end) = text[after_backticks..].find('\n') {
            let lang_str = text[after_backticks..after_backticks + line_end].trim();
            let content_start = after_backticks + line_end + 1;

            if lang_str.eq_ignore_ascii_case(lang) {
                if let Some(close_offset) = text[content_start..].find("```") {
                    let content = text[content_start..content_start + close_offset].trim();
                    return Some(content);
                }
            }

            search_from = content_start;
        } else {
            break;
        }
    }
    None
}

/// Find a bracketed substring by matching open/close delimiters.
///
/// Handles nesting. Prefers later (more likely to be the actual output)
/// over earlier occurrences.
///
/// - `find_bracketed(text, '[', ']')` — finds JSON arrays
/// - `find_bracketed(text, '{', '}')` — finds JSON objects
///
/// # Examples
///
/// ```
/// use llm_output_parser::extract::find_bracketed;
///
/// let input = r#"Result: {"a": [1, 2]}"#;
/// assert_eq!(find_bracketed(input, '{', '}'), Some(r#"{"a": [1, 2]}"#));
/// ```
pub fn find_bracketed(text: &str, open: char, close: char) -> Option<&str> {
    // Collect all top-level bracketed regions using nesting-aware scanning.
    // Prefer the last (later) match, which is more likely to be the LLM's answer.
    let mut best: Option<&str> = None;
    let mut scan_from = 0;

    while scan_from < text.len() {
        if let Some(offset) = text[scan_from..].find(open) {
            let start = scan_from + offset;
            let mut depth = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let mut found_end = None;

            for (i, ch) in text[start..].char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if ch == '\\' && in_string {
                    escape_next = true;
                    continue;
                }
                if ch == '"' {
                    in_string = !in_string;
                    continue;
                }
                if in_string {
                    continue;
                }
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        found_end = Some(start + i);
                        break;
                    }
                }
            }

            if let Some(end) = found_end {
                best = Some(&text[start..=end]);
                scan_from = end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    best
}

/// Find a bracketed substring with depth limit enforcement.
///
/// Returns `Err(ParseError::TooDeep)` if nesting exceeds `max_depth`.
/// Otherwise behaves identically to [`find_bracketed`].
pub(crate) fn find_bracketed_limited(
    text: &str,
    open: char,
    close: char,
    max_depth: usize,
) -> Result<Option<&str>, ParseError> {
    let mut best: Option<&str> = None;
    let mut scan_from = 0;

    while scan_from < text.len() {
        if let Some(offset) = text[scan_from..].find(open) {
            let start = scan_from + offset;
            let mut depth: usize = 0;
            let mut in_string = false;
            let mut escape_next = false;
            let mut found_end = None;

            for (i, ch) in text[start..].char_indices() {
                if escape_next {
                    escape_next = false;
                    continue;
                }
                if ch == '\\' && in_string {
                    escape_next = true;
                    continue;
                }
                if ch == '"' {
                    in_string = !in_string;
                    continue;
                }
                if in_string {
                    continue;
                }
                if ch == open {
                    depth += 1;
                    if depth > max_depth {
                        return Err(ParseError::TooDeep {
                            depth,
                            limit: max_depth,
                        });
                    }
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        found_end = Some(start + i);
                        break;
                    }
                }
            }

            if let Some(end) = found_end {
                best = Some(&text[start..=end]);
                scan_from = end + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- strip_think_tags --

    #[test]
    fn strip_think_tags_complete() {
        let input = "<think>reasoning</think>result";
        assert_eq!(strip_think_tags(input), "result");
    }

    #[test]
    fn strip_think_tags_incomplete() {
        let input = "<think>reasoning without close";
        assert_eq!(strip_think_tags(input), "");
    }

    #[test]
    fn strip_think_tags_multiple() {
        let input = "<think>first</think>middle<think>second</think>end";
        assert_eq!(strip_think_tags(input), "middleend");
    }

    #[test]
    fn strip_thinking_tags_complete() {
        let input = "<thinking>reasoning</thinking>result";
        assert_eq!(strip_think_tags(input), "result");
    }

    #[test]
    fn strip_thinking_tags_incomplete() {
        let input = "<thinking>reasoning without close";
        assert_eq!(strip_think_tags(input), "");
    }

    #[test]
    fn strip_think_tags_no_tags() {
        let input = "just plain text";
        assert_eq!(strip_think_tags(input), "just plain text");
    }

    #[test]
    fn strip_mixed_think_and_thinking() {
        let input = "<think>a</think>mid<thinking>b</thinking>end";
        assert_eq!(strip_think_tags(input), "midend");
    }

    // -- preprocess --

    #[test]
    fn preprocess_strips_and_trims() {
        let input = "  <think>stuff</think>  hello world  ";
        assert_eq!(preprocess(input), "hello world");
    }

    // -- extract_code_block --

    #[test]
    fn extract_json_code_block() {
        let input = "Here:\n```json\n{\"a\": 1}\n```";
        let (lang, content) = extract_code_block(input).unwrap();
        assert_eq!(lang, Some("json"));
        assert_eq!(content, "{\"a\": 1}");
    }

    #[test]
    fn extract_bare_code_block() {
        let input = "Here:\n```\n{\"a\": 1}\n```";
        let (lang, content) = extract_code_block(input).unwrap();
        assert_eq!(lang, None);
        assert_eq!(content, "{\"a\": 1}");
    }

    #[test]
    fn extract_code_block_no_fence() {
        let input = "no code blocks here";
        assert!(extract_code_block(input).is_none());
    }

    // -- extract_code_block_for --

    #[test]
    fn extract_code_block_for_json() {
        let input = "```json\n[1, 2, 3]\n```";
        assert_eq!(extract_code_block_for(input, "json"), Some("[1, 2, 3]"));
    }

    #[test]
    fn extract_code_block_for_wrong_lang() {
        let input = "```yaml\nname: test\n```";
        assert_eq!(extract_code_block_for(input, "json"), None);
    }

    // -- find_bracketed --

    #[test]
    fn find_bracketed_json_object() {
        let input = r#"Result: {"a": [1, 2]}"#;
        assert_eq!(find_bracketed(input, '{', '}'), Some(r#"{"a": [1, 2]}"#));
    }

    #[test]
    fn find_bracketed_json_array() {
        let input = r#"Here: ["a", "b"] done"#;
        assert_eq!(find_bracketed(input, '[', ']'), Some(r#"["a", "b"]"#));
    }

    #[test]
    fn find_bracketed_nested() {
        let input = r#"{"outer": {"inner": [1]}}"#;
        assert_eq!(
            find_bracketed(input, '{', '}'),
            Some(r#"{"outer": {"inner": [1]}}"#)
        );
    }

    #[test]
    fn find_bracketed_prefers_later() {
        // Two separate bracketed regions — prefer the later one
        let input = r#"[1, 2] and then ["a", "b"]"#;
        let result = find_bracketed(input, '[', ']');
        assert_eq!(result, Some(r#"["a", "b"]"#));
    }

    #[test]
    fn find_bracketed_no_match() {
        let input = "no brackets here";
        assert!(find_bracketed(input, '{', '}').is_none());
    }

    #[test]
    fn find_bracketed_with_string_containing_brackets() {
        let input = r#"{"text": "hello [world]"}"#;
        assert_eq!(
            find_bracketed(input, '{', '}'),
            Some(r#"{"text": "hello [world]"}"#)
        );
    }

    // -- find_bracketed_limited --

    #[test]
    fn find_bracketed_limited_ok() {
        let input = r#"{"a": {"b": 1}}"#;
        let result = find_bracketed_limited(input, '{', '}', 10).unwrap();
        assert_eq!(result, Some(r#"{"a": {"b": 1}}"#));
    }

    #[test]
    fn find_bracketed_limited_too_deep() {
        // Depth 3 nesting with limit 2
        let input = r#"{"a": {"b": {"c": 1}}}"#;
        let result = find_bracketed_limited(input, '{', '}', 2);
        assert!(matches!(
            result,
            Err(ParseError::TooDeep { depth: 3, limit: 2 })
        ));
    }

    #[test]
    fn find_bracketed_limited_exact_boundary() {
        // Depth exactly at limit should succeed
        let input = r#"{"a": {"b": 1}}"#;
        let result = find_bracketed_limited(input, '{', '}', 2).unwrap();
        assert_eq!(result, Some(r#"{"a": {"b": 1}}"#));
    }
}
