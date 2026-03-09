//! XML-style tag extraction from LLM responses.
//!
//! Provides [`parse_xml_tag`] and [`parse_xml_tags`] for extracting content
//! from XML-style structured delimiters in LLM output. Does NOT use a full
//! XML parser — these are lightweight tag-matching functions.

use std::collections::HashMap;

use crate::error::{
    ensure_input_within_limits, truncate, ParseError, ParseOptions, ParseTrace,
};
use crate::extract::preprocess_opts;

/// Extract content from a single XML-style tag in an LLM response.
///
/// Looks for `<tag>content</tag>` after preprocessing.
/// Handles missing close tags (returns content to end of string).
/// Does NOT use a full XML parser — these are structured delimiters.
///
/// # Examples
///
/// ```
/// use llm_output_parser::parse_xml_tag;
///
/// let response = "<answer>The capital is Paris.</answer>";
/// let answer = parse_xml_tag(response, "answer").unwrap();
/// assert_eq!(answer, "The capital is Paris.");
/// ```
pub fn parse_xml_tag(response: &str, tag: &str) -> Result<String, ParseError> {
    let opts = ParseOptions::default();
    let (value, _trace) = parse_xml_tag_with_trace(response, tag, &opts)?;
    Ok(value)
}

/// Extract content from a single XML-style tag with diagnostic trace.
pub fn parse_xml_tag_with_trace(
    response: &str,
    tag: &str,
    opts: &ParseOptions,
) -> Result<(String, ParseTrace), ParseError> {
    ensure_input_within_limits(response, opts)?;

    let cleaned = preprocess_opts(response, opts.strip_think_tags);
    let mut trace = ParseTrace::default();

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    trace.strategies_tried.push("tag_lookup");

    if let Some(start) = cleaned.find(&open_tag) {
        let content_start = start + open_tag.len();
        let content = if let Some(end) = cleaned[content_start..].find(&close_tag) {
            &cleaned[content_start..content_start + end]
        } else {
            // No closing tag — take content to end
            &cleaned[content_start..]
        };
        let trimmed = content.trim();
        trace.extracted_span = Some((content_start, content_start + content.len()));
        return Ok((trimmed.to_string(), trace));
    }

    Err(ParseError::Unparseable {
        expected_format: "XML tag",
        text: truncate(&cleaned, 200),
    })
}

/// Extract content from multiple XML-style tags into a map.
///
/// Returns a `HashMap` of `tag_name -> content` for each tag found.
/// Missing tags are simply absent from the map (not an error).
/// At least one tag must be found or returns `ParseError`.
///
/// # Examples
///
/// ```
/// use llm_output_parser::parse_xml_tags;
///
/// let response = "<analysis>Looks good</analysis><confidence>0.95</confidence>";
/// let result = parse_xml_tags(response, &["analysis", "confidence"]).unwrap();
/// assert_eq!(result["analysis"], "Looks good");
/// assert_eq!(result["confidence"], "0.95");
/// ```
pub fn parse_xml_tags(
    response: &str,
    tags: &[&str],
) -> Result<HashMap<String, String>, ParseError> {
    let opts = ParseOptions::default();
    let (values, _trace) = parse_xml_tags_with_trace(response, tags, &opts)?;
    Ok(values)
}

/// Extract content from multiple XML-style tags with diagnostic trace.
pub fn parse_xml_tags_with_trace(
    response: &str,
    tags: &[&str],
    opts: &ParseOptions,
) -> Result<(HashMap<String, String>, ParseTrace), ParseError> {
    ensure_input_within_limits(response, opts)?;

    let cleaned = preprocess_opts(response, opts.strip_think_tags);
    let mut trace = ParseTrace::default();

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    let mut results = HashMap::new();
    trace.strategies_tried.push("multi_tag_lookup");

    for &tag in tags {
        let open_tag = format!("<{}>", tag);
        let close_tag = format!("</{}>", tag);

        if let Some(start) = cleaned.find(&open_tag) {
            let content_start = start + open_tag.len();
            let content = if let Some(end) = cleaned[content_start..].find(&close_tag) {
                &cleaned[content_start..content_start + end]
            } else {
                &cleaned[content_start..]
            };
            if trace.extracted_span.is_none() {
                trace.extracted_span = Some((content_start, content_start + content.len()));
            }
            results.insert(tag.to_string(), content.trim().to_string());
        }
    }

    if results.is_empty() {
        return Err(ParseError::Unparseable {
            expected_format: "XML tags",
            text: truncate(&cleaned, 200),
        });
    }

    for tag in tags {
        if !results.contains_key(*tag) {
            trace.warnings.push(format!("tag '{}' not found", tag));
        }
    }

    Ok((results, trace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_tag() {
        let result = parse_xml_tag("<answer>Paris</answer>", "answer").unwrap();
        assert_eq!(result, "Paris");
    }

    #[test]
    fn think_then_tag() {
        let result =
            parse_xml_tag("<think>reasoning</think><answer>Paris</answer>", "answer").unwrap();
        assert_eq!(result, "Paris");
    }

    #[test]
    fn multiple_tags() {
        let result = parse_xml_tags("<a>one</a><b>two</b>", &["a", "b"]).unwrap();
        assert_eq!(result["a"], "one");
        assert_eq!(result["b"], "two");
    }

    #[test]
    fn nested_content() {
        let result = parse_xml_tag("<answer>The answer is <b>bold</b></answer>", "answer").unwrap();
        assert_eq!(result, "The answer is <b>bold</b>");
    }

    #[test]
    fn missing_close() {
        let result = parse_xml_tag("<answer>Paris", "answer").unwrap();
        assert_eq!(result, "Paris");
    }

    #[test]
    fn multiline_content() {
        let result = parse_xml_tag("<code>\nfn main() {}\n</code>", "code").unwrap();
        assert_eq!(result, "fn main() {}");
    }

    #[test]
    fn whitespace_trimming() {
        let result = parse_xml_tag("<answer>  Paris  </answer>", "answer").unwrap();
        assert_eq!(result, "Paris");
    }

    #[test]
    fn tag_not_found() {
        let result = parse_xml_tag("<wrong>data</wrong>", "answer");
        assert!(result.is_err());
    }

    #[test]
    fn partial_tags_found() {
        let result = parse_xml_tags("<a>one</a><b>two</b>", &["a", "b", "c"]).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result["a"], "one");
        assert_eq!(result["b"], "two");
    }

    #[test]
    fn no_tags_found() {
        let result = parse_xml_tags("<x>data</x>", &["a", "b"]);
        assert!(result.is_err());
    }

    #[test]
    fn case_sensitive() {
        let result = parse_xml_tag("<Answer>Paris</Answer>", "answer");
        assert!(result.is_err());
    }

    #[test]
    fn tag_with_trace_records_span() {
        let opts = ParseOptions::default();
        let (result, trace) = parse_xml_tag_with_trace("<answer>Paris</answer>", "answer", &opts)
            .unwrap();
        assert_eq!(result, "Paris");
        assert!(trace.strategies_tried.contains(&"tag_lookup"));
        assert!(trace.extracted_span.is_some());
    }

    #[test]
    fn tags_with_trace_warn_on_missing_tag() {
        let opts = ParseOptions::default();
        let (result, trace) =
            parse_xml_tags_with_trace("<a>one</a>", &["a", "b"], &opts).unwrap();
        assert_eq!(result["a"], "one");
        assert_eq!(trace.warnings, vec!["tag 'b' not found"]);
    }

    #[test]
    fn with_trace_rejects_oversized_input() {
        let opts = ParseOptions {
            max_input_bytes: 8,
            ..ParseOptions::default()
        };
        let err = parse_xml_tag_with_trace("<answer>too long</answer>", "answer", &opts)
            .unwrap_err();
        assert_eq!(err.kind(), "too_large");
    }
}
