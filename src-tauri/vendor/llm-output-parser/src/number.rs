//! Numeric value extraction from LLM responses.
//!
//! Provides [`parse_number`] for extracting a numeric value and
//! [`parse_number_in_range`] for bounded extraction, handling common LLM
//! output patterns like "Score: 8.5", "8/10", and prose-wrapped numbers.

use std::str::FromStr;

use crate::error::{
    ensure_input_within_limits, record_extracted_span, ParseError, ParseOptions, ParseTrace,
};
use crate::extract::preprocess_opts;

/// Extract a numeric value from an LLM response.
///
/// Handles common patterns:
/// - Direct number: `"8.5"`
/// - Score format: `"8.5/10"`, `"8/10"`
/// - In prose: `"I'd rate it 8.5 out of 10"`
/// - Labeled: `"Score: 8.5"`, `"Rating: 8"`
/// - With think block: `"<think>considering...</think>8.5"`
///
/// # Examples
///
/// ```
/// use llm_output_parser::parse_number;
///
/// let score: f64 = parse_number("Score: 8.5").unwrap();
/// assert!((score - 8.5).abs() < f64::EPSILON);
/// ```
pub fn parse_number<T: FromStr>(response: &str) -> Result<T, ParseError> {
    let opts = ParseOptions::default();
    let (value, _trace) = parse_number_with_trace(response, &opts)?;
    Ok(value)
}

/// Extract a numeric value from an LLM response with diagnostic trace.
pub fn parse_number_with_trace<T: FromStr>(
    response: &str,
    opts: &ParseOptions,
) -> Result<(T, ParseTrace), ParseError> {
    ensure_input_within_limits(response, opts)?;

    let cleaned = preprocess_opts(response, opts.strip_think_tags);
    let mut trace = ParseTrace::default();

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    trace.strategies_tried.push("direct_parse");
    if let Ok(val) = cleaned.parse::<T>() {
        trace.extracted_span = Some((0, cleaned.len()));
        return Ok((val, trace));
    }

    let lower = cleaned.to_lowercase();

    trace.strategies_tried.push("labeled_number");
    for label in ["score:", "rating:", "result:"] {
        if let Some(pos) = lower.find(label) {
            let after = &cleaned[pos + label.len()..];
            let nums = find_all_numbers(after);
            if let Some(first) = nums.first() {
                if let Ok(val) = first.parse::<T>() {
                    record_extracted_span(&mut trace, &cleaned, first);
                    return Ok((val, trace));
                }
            }
        }
    }

    trace.strategies_tried.push("fraction_number");
    if let Some(num) = extract_fraction::<T>(&cleaned) {
        if let Some(fraction_text) = cleaned
            .split_whitespace()
            .find(|part| part.contains('/'))
            .map(str::trim)
        {
            record_extracted_span(&mut trace, &cleaned, fraction_text);
        }
        return Ok((num, trace));
    }

    trace.strategies_tried.push("scan_all_numbers");
    let numbers = find_all_numbers(&cleaned);
    // Try from the end
    for candidate in numbers.iter().rev() {
        if let Ok(val) = candidate.parse::<T>() {
            record_extracted_span(&mut trace, &cleaned, candidate);
            return Ok((val, trace));
        }
    }

    Err(ParseError::NoNumber)
}

/// Extract a number and verify it falls within `[min, max]` inclusive.
///
/// # Examples
///
/// ```
/// use llm_output_parser::parse_number_in_range;
///
/// let score: i32 = parse_number_in_range("8", 1, 10).unwrap();
/// assert_eq!(score, 8);
/// ```
pub fn parse_number_in_range<T: FromStr + PartialOrd + std::fmt::Display>(
    response: &str,
    min: T,
    max: T,
) -> Result<T, ParseError> {
    let opts = ParseOptions::default();
    let (val, _trace) = parse_number_in_range_with_trace(response, min, max, &opts)?;
    Ok(val)
}

/// Extract a bounded numeric value with diagnostic trace.
pub fn parse_number_in_range_with_trace<T: FromStr + PartialOrd + std::fmt::Display>(
    response: &str,
    min: T,
    max: T,
    opts: &ParseOptions,
) -> Result<(T, ParseTrace), ParseError> {
    let (val, trace) = parse_number_with_trace::<T>(response, opts)?;
    if val < min || val > max {
        return Err(ParseError::NoNumber);
    }
    Ok((val, trace))
}

/// Extract the numerator from a fraction pattern like "8/10" or "8.5/10".
fn extract_fraction<T: FromStr>(text: &str) -> Option<T> {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    for i in 0..len {
        if chars[i] == '/' {
            // Look backwards for the numerator
            let mut num_start = i;
            while num_start > 0 {
                let c = chars[num_start - 1];
                if c.is_ascii_digit() || c == '.' || c == '-' {
                    num_start -= 1;
                } else {
                    break;
                }
            }
            if num_start < i {
                let num_str: String = chars[num_start..i].iter().collect();
                if let Ok(val) = num_str.parse::<T>() {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Find all number-like substrings in text (digits, optional decimal, optional leading minus).
fn find_all_numbers(text: &str) -> Vec<String> {
    let mut numbers = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        // Check for start of a number
        let is_negative = chars[i] == '-' && i + 1 < len && chars[i + 1].is_ascii_digit();
        let is_digit = chars[i].is_ascii_digit();

        if is_digit || is_negative {
            let start = i;
            if is_negative {
                i += 1;
            }
            // Consume digits
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            // Optional decimal part
            if i < len && chars[i] == '.' && i + 1 < len && chars[i + 1].is_ascii_digit() {
                i += 1; // skip dot
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let num_str: String = chars[start..i].iter().collect();
            numbers.push(num_str);
            continue;
        }
        i += 1;
    }

    numbers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer() {
        let result: i32 = parse_number("8").unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn float() {
        let result: f64 = parse_number("8.5").unwrap();
        assert!((result - 8.5).abs() < f64::EPSILON);
    }

    #[test]
    fn fraction() {
        let result: i32 = parse_number("8/10").unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn in_prose() {
        let result: f64 = parse_number("I'd give it a 7.5").unwrap();
        assert!((result - 7.5).abs() < f64::EPSILON);
    }

    #[test]
    fn labeled() {
        let result: i32 = parse_number("Score: 9").unwrap();
        assert_eq!(result, 9);
    }

    #[test]
    fn with_think() {
        let result: f64 = parse_number("<think>considering</think>8.5").unwrap();
        assert!((result - 8.5).abs() < f64::EPSILON);
    }

    #[test]
    fn negative() {
        let result: i32 = parse_number("-3").unwrap();
        assert_eq!(result, -3);
    }

    #[test]
    fn range_pass() {
        let result: i32 = parse_number_in_range("8", 1, 10).unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn range_fail() {
        let result: Result<i32, _> = parse_number_in_range("15", 1, 10);
        assert!(result.is_err());
    }

    #[test]
    fn no_number() {
        let result: Result<i32, _> = parse_number("great work");
        assert!(result.is_err());
    }

    #[test]
    fn multiple_numbers_labeled() {
        let result: i32 = parse_number("Page 3 of 5, Score: 8").unwrap();
        assert_eq!(result, 8);
    }

    #[test]
    fn with_trace_records_fraction_strategy() {
        let opts = ParseOptions::default();
        let (result, trace): (i32, ParseTrace) = parse_number_with_trace("8/10", &opts).unwrap();
        assert_eq!(result, 8);
        assert!(trace.strategies_tried.contains(&"fraction_number"));
        assert!(trace.extracted_span.is_some());
    }

    #[test]
    fn in_range_with_trace_rejects_oversized_input() {
        let opts = ParseOptions {
            max_input_bytes: 4,
            ..ParseOptions::default()
        };
        let err = parse_number_in_range_with_trace::<i32>("Score: 8", 1, 10, &opts).unwrap_err();
        assert_eq!(err.kind(), "too_large");
    }
}
