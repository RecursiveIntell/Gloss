//! Enforced RFC 8785 boundary admission profiles.
//!
//! A profile contains only rules enforced by BoundaryProfile::parse.
//! Earlier metadata-only dialect, schema_id, schema_version,
//! canonicalization, unknown_field_policy, and max_float_digits fields
//! were removed: this crate has exactly one RFC 8785 dialect/profile, schema
//! identity and unknown-field admission belong to a configured schema engine,
//! and an arbitrary decimal digit cap would conflict with ECMAScript number
//! serialization.

use crate::{parse_with_dup_check, Canonicalizer, JcsError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Resource budgets enforced for every profiled parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceCeilings {
    /// Maximum UTF-8 input size before JSON parsing begins.
    pub max_input_bytes: usize,
    /// Maximum aggregate number of JSON values, including the root.
    pub max_nodes: usize,
    /// Maximum container nesting depth, with the root at depth zero.
    pub max_depth: usize,
    /// Maximum number of properties in any one object.
    pub max_object_keys: usize,
    /// Maximum UTF-8 byte length of any one property name or string value.
    pub max_string_bytes: usize,
    /// Maximum number of elements in any one array.
    pub max_array_len: usize,
}

impl Default for ResourceCeilings {
    fn default() -> Self {
        Self {
            max_input_bytes: 1 << 20,
            max_nodes: 100_000,
            max_depth: 32,
            max_object_keys: 128,
            max_string_bytes: 1 << 20,
            max_array_len: 1024,
        }
    }
}

/// One rule recorded by a successful boundary admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcedRule {
    /// Stable machine-readable rule name.
    pub rule: String,
    /// Maximum observed use for quantitative rules.
    pub used: Option<usize>,
    /// Configured ceiling for quantitative rules.
    pub limit: Option<usize>,
}

/// Receipt proving which rules were applied to an admitted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryEnforcementReceipt {
    /// The single canonicalization contract supported by this crate.
    pub canonicalization_profile: String,
    /// Rules applied in stable evaluation order.
    pub enforced_rules: Vec<EnforcedRule>,
}

/// Parsed and canonicalized output admitted by a boundary profile.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryAdmission {
    /// Duplicate-free parsed JSON value.
    pub value: Value,
    /// RFC 8785 canonical UTF-8 bytes for value.
    pub canonical_bytes: Vec<u8>,
    /// Receipt enumerating every enforced rule.
    pub receipt: BoundaryEnforcementReceipt,
}

/// RFC 8785 admission profile containing only executable resource policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BoundaryProfile {
    /// Resource budgets applied by BoundaryProfile::parse.
    pub resource_ceilings: ResourceCeilings,
}

#[derive(Default)]
struct ResourceUse {
    nodes: usize,
    depth: usize,
    object_keys: usize,
    string_bytes: usize,
    array_len: usize,
}

impl BoundaryProfile {
    /// Creates a profile with explicit enforced budgets.
    pub fn new(resource_ceilings: ResourceCeilings) -> Self {
        Self { resource_ceilings }
    }

    /// Creates the default RFC 8785 admission profile.
    pub fn rfc8785() -> Self {
        Self::default()
    }

    /// Parses, budget-checks, and canonicalizes one JSON input.
    pub fn parse(&self, input: &str) -> Result<BoundaryAdmission, JcsError> {
        self.check(
            "input_bytes",
            input.len(),
            self.resource_ceilings.max_input_bytes,
        )?;
        let value = parse_with_dup_check(input)?;
        let mut use_counts = ResourceUse::default();
        self.inspect(&value, 0, &mut use_counts)?;
        let canonical_bytes = Canonicalizer::new().canonicalize_bytes(&value)?;

        let rules = [
            (
                "input_bytes",
                input.len(),
                self.resource_ceilings.max_input_bytes,
            ),
            ("nodes", use_counts.nodes, self.resource_ceilings.max_nodes),
            ("depth", use_counts.depth, self.resource_ceilings.max_depth),
            (
                "object_keys",
                use_counts.object_keys,
                self.resource_ceilings.max_object_keys,
            ),
            (
                "string_bytes",
                use_counts.string_bytes,
                self.resource_ceilings.max_string_bytes,
            ),
            (
                "array_len",
                use_counts.array_len,
                self.resource_ceilings.max_array_len,
            ),
        ]
        .into_iter()
        .map(|(rule, used, limit)| EnforcedRule {
            rule: rule.to_owned(),
            used: Some(used),
            limit: Some(limit),
        })
        .chain(
            ["duplicate_keys", "canonicalization"]
                .into_iter()
                .map(|rule| EnforcedRule {
                    rule: rule.to_owned(),
                    used: None,
                    limit: None,
                }),
        )
        .collect();

        Ok(BoundaryAdmission {
            value,
            canonical_bytes,
            receipt: BoundaryEnforcementReceipt {
                canonicalization_profile: "rfc8785".to_owned(),
                enforced_rules: rules,
            },
        })
    }

    /// Checks a programmatically-created value against structural budgets.
    pub fn check_resources(&self, value: &Value) -> Result<(), JcsError> {
        self.inspect(value, 0, &mut ResourceUse::default())
    }

    fn inspect(
        &self,
        value: &Value,
        depth: usize,
        use_counts: &mut ResourceUse,
    ) -> Result<(), JcsError> {
        use_counts.nodes += 1;
        use_counts.depth = use_counts.depth.max(depth);
        self.check("nodes", use_counts.nodes, self.resource_ceilings.max_nodes)?;
        self.check("depth", depth, self.resource_ceilings.max_depth)?;

        match value {
            Value::Object(map) => {
                use_counts.object_keys = use_counts.object_keys.max(map.len());
                self.check(
                    "object_keys",
                    map.len(),
                    self.resource_ceilings.max_object_keys,
                )?;
                for (key, child) in map {
                    use_counts.string_bytes = use_counts.string_bytes.max(key.len());
                    self.check(
                        "string_bytes",
                        key.len(),
                        self.resource_ceilings.max_string_bytes,
                    )?;
                    self.inspect(child, depth + 1, use_counts)?;
                }
            }
            Value::Array(items) => {
                use_counts.array_len = use_counts.array_len.max(items.len());
                self.check(
                    "array_len",
                    items.len(),
                    self.resource_ceilings.max_array_len,
                )?;
                for child in items {
                    self.inspect(child, depth + 1, use_counts)?;
                }
            }
            Value::String(string) => {
                use_counts.string_bytes = use_counts.string_bytes.max(string.len());
                self.check(
                    "string_bytes",
                    string.len(),
                    self.resource_ceilings.max_string_bytes,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn check(&self, resource: &str, used: usize, limit: usize) -> Result<(), JcsError> {
        if used > limit {
            return Err(JcsError::ResourceCeilingExceeded {
                resource: resource.to_owned(),
                used,
                limit,
            });
        }
        Ok(())
    }

    /// Stable identifier for the only supported canonicalization profile.
    pub fn identifier(&self) -> &'static str {
        "rfc8785"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn check_resources_accepts_bounded_value() {
        BoundaryProfile::default()
            .check_resources(&json!({"a": "hello", "b": [1, 2, 3]}))
            .unwrap();
    }

    #[test]
    fn check_resources_rejects_excess_depth() {
        let mut profile = BoundaryProfile::default();
        profile.resource_ceilings.max_depth = 2;
        assert!(matches!(
            profile.check_resources(&json!({"a": {"b": {"c": 1}}})),
            Err(JcsError::ResourceCeilingExceeded { ref resource, .. }) if resource == "depth"
        ));
    }
}
