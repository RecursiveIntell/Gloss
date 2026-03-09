//! # stack-ids
//!
//! Shared identity, scope, and trace primitives for the local-first AI systems stack.
//!
//! This crate is the single source of truth for cross-crate identity types.
//! No other crate in the stack should define competing ID newtypes for the
//! same concepts.
//!
//! ## Authority
//!
//! `stack-ids` is authoritative for:
//! - Opaque ID newtypes (EnvelopeId, ClaimId, ClaimVersionId, EntityId,
//!   EpisodeId, AttemptId, TrialId, ArtifactId, ProjectionId, RelationId,
//!   RelationVersionId, ImportBatchId)
//! - Scope representation (ScopeKey, Scope)
//! - Trace context (TraceCtx, W3C trace-context helpers)
//! - Content digest computation (canonical BLAKE3 digest)
//!
//! `stack-ids` is NOT authoritative for:
//! - What data these IDs point to (owned by respective crates)
//! - Storage schemas (owned by persistence crates)
//! - Business logic or policy (owned by domain crates)
//!
//! ## Phase status: current / implemented now

mod digest;
mod ids;
mod scope;
mod trace;

pub use digest::*;
pub use ids::*;
pub use scope::*;
pub use trace::*;
