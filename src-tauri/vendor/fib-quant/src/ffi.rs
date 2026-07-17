//! FFI bindings to the C kernels in `c-kernels/`.
//!
//! These wrappers provide safe Rust interfaces to the compiled C
//! implementations of the hot numerical loops. The Rust side handles
//! all validation; the C code only performs the tight inner loops.

// This module requires unsafe FFI calls to the C kernels. The crate-level
// `unsafe_code = "deny"` lint is overridden here because FFI is the
// intended and reviewed use of unsafe in this module.
#![allow(unsafe_code)]

use std::os::raw::c_int;

extern "C" {
    fn fq_encode_vector_block(
        vec: *const f32,
        dim: usize,
        codebook: *const f32,
        codebook_size: usize,
        block_dim: usize,
        out_indices: *mut u16,
    ) -> usize;

    fn fq_decode_vector_block(
        indices: *const u16,
        block_count: usize,
        codebook: *const f32,
        codebook_size: usize,
        block_dim: usize,
        out_vec: *mut f32,
    );

    fn fq_compressed_attention_logits(
        key_indices: *const u16,
        n_keys: usize,
        key_norms: *const f32,
        gram_table: *const f32,
        gram_size: usize,
        query_indices: *const u16,
        query_count: usize,
        query_norm: f32,
        scale: f32,
        out_logits: *mut f32,
    );

    fn fq_softmax(logits: *mut f32, n: usize) -> c_int;
}

/// Encode a vector block: find nearest codeword for each block of
/// `block_dim` floats. Returns the codeword indices.
///
/// # Safety
/// The caller must ensure:
/// - `vec.len() == dim` and `dim % block_dim == 0`
/// - `codebook.len() == codebook_size * block_dim`
/// - `out_indices` has capacity `dim / block_dim`
pub fn c_encode_vector_block(
    vec: &[f32],
    codebook: &[f32],
    codebook_size: usize,
    block_dim: usize,
) -> Vec<u16> {
    let dim = vec.len();
    let block_count = dim / block_dim;
    let mut indices = vec![0u16; block_count];
    // SAFETY: All pointers are valid slices with correct lengths.
    // The C function reads `dim` floats from vec, `codebook_size*block_dim`
    // floats from codebook, and writes `block_count` u16 to out_indices.
    unsafe {
        fq_encode_vector_block(
            vec.as_ptr(),
            dim,
            codebook.as_ptr(),
            codebook_size,
            block_dim,
            indices.as_mut_ptr(),
        );
    }
    indices
}

/// Decode a vector block: gather codewords by index.
///
/// # Panics
/// Panics if any index >= codebook_size (caller should validate).
pub fn c_decode_vector_block(
    indices: &[u16],
    codebook: &[f32],
    codebook_size: usize,
    block_dim: usize,
) -> Vec<f32> {
    let block_count = indices.len();
    let mut out = vec![0.0f32; block_count * block_dim];
    // SAFETY: All pointers are valid slices with correct lengths.
    unsafe {
        fq_decode_vector_block(
            indices.as_ptr(),
            block_count,
            codebook.as_ptr(),
            codebook_size,
            block_dim,
            out.as_mut_ptr(),
        );
    }
    out
}

/// Compute compressed attention logits via C kernel.
///
/// For each key, sums Gram table lookups across all blocks, then scales
/// by query_norm * stored_norm / scale.
#[allow(clippy::too_many_arguments)]
pub fn c_compressed_attention_logits(
    key_indices: &[u16],
    n_keys: usize,
    key_norms: &[f32],
    gram_table: &[f32],
    gram_size: usize,
    query_indices: &[u16],
    query_count: usize,
    query_norm: f32,
    scale: f32,
) -> Vec<f32> {
    let mut logits = vec![0.0f32; n_keys];
    // SAFETY: All pointers are valid slices with correct lengths.
    unsafe {
        fq_compressed_attention_logits(
            key_indices.as_ptr(),
            n_keys,
            key_norms.as_ptr(),
            gram_table.as_ptr(),
            gram_size,
            query_indices.as_ptr(),
            query_count,
            query_norm,
            scale,
            logits.as_mut_ptr(),
        );
    }
    logits
}

/// Numerically stable softmax via C kernel.
///
/// Returns `Ok(())` on success, `Err` on underflow or empty input.
#[allow(clippy::result_unit_err)]
pub fn c_softmax(logits: &mut [f32]) -> Result<(), ()> {
    if logits.is_empty() {
        return Err(());
    }
    // SAFETY: logits is a valid mutable slice.
    let ret = unsafe { fq_softmax(logits.as_mut_ptr(), logits.len()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(())
    }
}
