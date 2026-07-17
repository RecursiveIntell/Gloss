//! JSON Schema generation for the public bitemporal types.
//!
//! Enabled with the `schema` cargo feature. The schemas are
//! generated via `schemars::schema_for!` so they stay in lockstep
//! with the type definitions.

#![cfg(feature = "schema")]

use schemars::{schema::RootSchema, schema_for};

use crate::{BitemporalRecord, SupersessionReceipt, SupersessionTarget};

/// JSON Schema for [`BitemporalRecord<serde_json::Value>`]. The
/// value is left as an open `Value` so consumers can record any
/// JSON-serializable payload.
pub fn bitemporal_record_schema() -> RootSchema {
    schema_for!(BitemporalRecord<serde_json::Value>)
}

/// JSON Schema for [`SupersessionReceipt`].
pub fn supersession_receipt_schema() -> RootSchema {
    schema_for!(SupersessionReceipt)
}

/// JSON Schema for [`SupersessionTarget`].
pub fn supersession_target_schema() -> RootSchema {
    schema_for!(SupersessionTarget)
}
