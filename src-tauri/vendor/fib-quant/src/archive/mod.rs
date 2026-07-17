//! Archived Rust implementations of fib-quant hot paths, preserved for
//! reference after replacement by C kernels.
//!
//! These modules are NOT compiled into the crate. They exist purely as
//! historical records of the original Rust inner loops that were moved
//! to C (see `c-kernels/` and `build.rs`).
//!
//! - `codec_rust.rs`: Original `encode_vector_block` from `kv/codec.rs`.
//! - `attention_rust.rs`: Original `softmax` and `compressed_attention_logits`
//!   inner loop from `kv/compressed_attention.rs`.

// The archived source files are included as raw strings so they are
// visible in the source tree for reference but never compiled.

/// Original `encode_vector_block` from `kv/codec.rs` (line ~243).
pub const CODEC_RUST_ARCHIVE: &str = include_str!("codec_rust.rs.bak");

/// Original `softmax` and inner loop from `kv/compressed_attention.rs`.
pub const ATTENTION_RUST_ARCHIVE: &str = include_str!("attention_rust.rs.bak");
