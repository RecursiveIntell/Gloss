//! YAML extraction from LLM responses (feature-gated behind `yaml`).
//!
//! Provides [`parse_yaml`] for extracting typed structs from YAML in LLM
//! output. Requires the `yaml` feature flag.

#[cfg(feature = "yaml")]
use serde::de::DeserializeOwned;

#[cfg(feature = "yaml")]
use crate::error::{truncate, ParseError};
#[cfg(feature = "yaml")]
use crate::extract::{extract_code_block, extract_code_block_for, preprocess};

/// Parse an LLM response containing YAML into a typed struct.
///
/// Strategies:
/// 1. Direct YAML parse on preprocessed text
/// 2. Extract from `` ```yaml `` code block
/// 3. Extract from any code block -> try as YAML
///
/// Requires the `yaml` feature flag.
///
/// # Examples
///
/// ```ignore
/// use serde::Deserialize;
/// use llm_output_parser::parse_yaml;
///
/// #[derive(Deserialize)]
/// struct Person { name: String, age: u32 }
///
/// let response = "name: Josh\nage: 30";
/// let person: Person = parse_yaml(response).unwrap();
/// assert_eq!(person.name, "Josh");
/// ```
#[cfg(feature = "yaml")]
pub fn parse_yaml<T: DeserializeOwned>(response: &str) -> Result<T, ParseError> {
    let cleaned = preprocess(response);

    if cleaned.is_empty() {
        return Err(ParseError::EmptyResponse);
    }

    // Strategy 1: Direct YAML parse
    if let Ok(val) = serde_yaml::from_str::<T>(&cleaned) {
        return Ok(val);
    }

    // Strategy 2: Extract from ```yaml code block
    if let Some(content) = extract_code_block_for(&cleaned, "yaml") {
        if let Ok(val) = serde_yaml::from_str::<T>(content) {
            return Ok(val);
        }
    }

    // Strategy 3: Extract from any code block
    if let Some((_lang, content)) = extract_code_block(&cleaned) {
        if let Ok(val) = serde_yaml::from_str::<T>(content) {
            return Ok(val);
        }
    }

    Err(ParseError::Unparseable {
        expected_format: "YAML",
        text: truncate(&cleaned, 200),
    })
}

#[cfg(all(test, feature = "yaml"))]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Person {
        name: String,
        age: u32,
    }

    #[test]
    fn direct_yaml() {
        let input = "name: Josh\nage: 30";
        let result: Person = parse_yaml(input).unwrap();
        assert_eq!(
            result,
            Person {
                name: "Josh".to_string(),
                age: 30
            }
        );
    }

    #[test]
    fn yaml_code_block() {
        let input = "```yaml\nname: Josh\nage: 30\n```";
        let result: Person = parse_yaml(input).unwrap();
        assert_eq!(
            result,
            Person {
                name: "Josh".to_string(),
                age: 30
            }
        );
    }

    #[test]
    fn think_then_yaml() {
        let input = "<think>analyzing</think>\nname: Josh\nage: 30";
        let result: Person = parse_yaml(input).unwrap();
        assert_eq!(
            result,
            Person {
                name: "Josh".to_string(),
                age: 30
            }
        );
    }
}
