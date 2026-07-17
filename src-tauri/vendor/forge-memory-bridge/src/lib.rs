//! # forge-memory-bridge
//!
//! Transforms Forge export envelopes into projection
//! import batches for consumption by
//! `semantic-memory`'s projection importer.
//!
//! ## Authority boundary
//!
//! This crate owns the **transformation** from Forge's export vocabulary to
//! memory's import vocabulary. It does NOT:
//! - evaluate comparability or decide promotion,
//! - guess missing semantics from live memory,
//! - become a query service,
//! - persist any state of its own.
//!
//! ## Phase status: current / implemented now

mod batch;
mod error;
#[allow(deprecated)]
mod legacy;
mod lifecycle;
mod transform;

pub use batch::*;
pub use error::{
    BridgeError, BridgeImportFailureArtifact, BRIDGE_IMPORT_FAILURE_ARTIFACT_V1_SCHEMA,
};
pub use lifecycle::*;
pub use transform::*;

/// Compatibility-only legacy bridge helpers.
///
/// The canonical path is `ExportEnvelopeV3` -> `transform_envelope_v3()` ->
/// `ProjectionImportBatchV3`.
/// V1/V2 helpers remain only for migration compatibility and should not be
/// used for new integrations.
///
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[doc(hidden)]
#[deprecated(
    since = "0.2.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[allow(deprecated)]
pub mod compat {
    pub use crate::legacy::{
        transform_legacy_envelope, transform_legacy_envelope_at, upgrade_legacy_envelope,
        upgrade_legacy_envelope_at, LegacyEpisodeMeta, LegacyImportEnvelopeV1, LegacyImportRecord,
    };
}

/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[doc(hidden)]
#[allow(deprecated)]
pub use compat::transform_legacy_envelope;
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[doc(hidden)]
#[allow(deprecated)]
pub use compat::upgrade_legacy_envelope;
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[doc(hidden)]
#[allow(deprecated)]
pub use compat::LegacyEpisodeMeta;
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[doc(hidden)]
#[allow(deprecated)]
pub use compat::LegacyImportEnvelopeV1;
/// Phase status: migration-only
/// Removal condition: remove when all consumers have migrated to `transform_envelope_v3()` + `ProjectionImportBatchV3`
#[deprecated(
    since = "0.1.0",
    note = "Legacy bridge helpers are compatibility-only. Use `transform_envelope_v3()` + `ProjectionImportBatchV3`."
)]
#[doc(hidden)]
#[allow(deprecated)]
pub use compat::LegacyImportRecord;
