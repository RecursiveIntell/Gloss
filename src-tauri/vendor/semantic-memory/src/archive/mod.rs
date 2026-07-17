//! Archived pure-Rust implementations replaced by C SIMD kernels.
//!
//! These are preserved for reference and potential rollback.  They are not
//! compiled into the active code path — the `#[allow(dead_code)]` attribute
//! on the module declaration in `lib.rs` suppresses warnings.

pub mod hubness_rust;
