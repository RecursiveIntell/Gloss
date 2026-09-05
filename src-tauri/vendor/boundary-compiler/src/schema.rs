//! Optional JSON Schema validation before and after JCS canonicalization.
//!
//! This module provides a `SchemaValidator` that can validate JSON values
//! against JSON Schema before and/or after canonicalization. It's designed
//! for boundary profiles that require schema conformance as part of
//! their transformation pipeline.
//!
//! The base crate has no schema engine, so schema-required admission fails closed.

use crate::error::JcsError;

/// JSON Schema validator companion to BoundaryProfile.
///
/// Fail-closed marker for schema-required admission in the base crate.
#[derive(Debug, Clone, Default)]
pub struct SchemaValidator;

impl SchemaValidator {
    /// Creates a validator marker.
    pub fn new() -> Self {
        Self
    }

    /// Refuses admission because no schema engine/schema is configured.
    pub fn validate(&self, _value: &serde_json::Value) -> Result<(), JcsError> {
        Err(JcsError::SchemaError(
            "schema validation is unavailable; refusing schema-required admission".into(),
        ))
    }
}
