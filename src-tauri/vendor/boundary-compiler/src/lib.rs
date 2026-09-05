//! boundary-compiler — RFC 8785 JCS with boundary profiles and duplicate-key rejection.
//!
//! # Overview
//!
//! This crate implements [RFC 8785](https://www.rfc-editor.org/rfc/rfc8785.html) — JSON Canonicalization Scheme (JCS):
//!
//! - Canonicalizer that serializes JSON into deterministic byte sequences
//! - Duplicate-key rejection (RFC 8785 mandates duplicate object keys are errors)
//! - blake3 Content-Digest of the JCS string
//! - Boundary profiles with enforced byte, node, depth, and container ceilings
//! - Fail-closed schema admission when no validator/schema is configured
//!
//! # Integration
//!
//! This crate is intended to replace `semantic_memory::graph::canonical_json_string()`
//! with a standards-compliant JCS implementation.
//!
//! # Example
//!
//! ```rust
//! use boundary_compiler::{Canonicalizer, ContentDigest, BoundaryProfile};
//! use serde_json::json;
//!
//! let c = Canonicalizer::new();
//! let val = json!({"b": 2, "a": 1});
//! let canonical = c.canonicalize(&val).unwrap();
//! assert_eq!(canonical, r#"{"a":1,"b":2}"#);
//!
//! let digest = ContentDigest::compute(&val).unwrap();
//! println!("Digest: {}", digest);
//! ```
//!
//! # Error handling
//!
//! All fallible operations use `thiserror` errors — no `unwrap()` or `expect()` in production.
//! Duplicate keys return `JcsError::DuplicateKey`, and schema validation failures return
//! `JcsError::SchemaValidation`.

pub mod canonicalizer;
pub mod digest;
pub mod error;
pub mod profile;
pub mod schema;

pub use canonicalizer::{
    canonicalize_flexible, parse_and_validate, parse_with_dup_check, Canonicalizer,
};
pub use digest::{
    ContentDigest, DigestMetadata, CANONICALIZATION_PROFILE, CANONICAL_JSON_DOMAIN,
    DEFAULT_SCHEMA_ID, DEFAULT_SCHEMA_VERSION, DIGEST_ALGORITHM,
};
pub use error::JcsError;
pub use profile::{
    BoundaryAdmission, BoundaryEnforcementReceipt, BoundaryProfile, EnforcedRule, ResourceCeilings,
};
pub use schema::SchemaValidator;
