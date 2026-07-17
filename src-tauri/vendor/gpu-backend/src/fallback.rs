//! CPU fallback implementations for GPU backend operations.
//!
//! These are used when CUDA is unavailable or batch size is too small
//! for GPU launch overhead to be worthwhile.

// CPU fallback — no CUDA imports needed
use crate::error::GpuError;
use crate::Result;

/// In-place Walsh-Hadamard Transform on CPU.
/// dim must be a power of 2. Pads to next power of 2 if not.
pub fn hadamard_batch_cpu(data: &mut [f32], n: usize, dim: usize, seed: u64) -> Result<()> {
    // Round up to next power of 2
    let padded_dim = dim.next_power_of_two();
    let orig_dim = dim;

    if data.len() != n * orig_dim {
        return Err(GpuError::DimensionMismatch {
            expected: n * orig_dim,
            got: data.len(),
        });
    }

    // Generate deterministic signs from seed
    let signs = generate_signs(padded_dim, seed);

    for vec_idx in 0..n {
        let offset = vec_idx * orig_dim;
        // Copy to padded buffer
        let mut padded = vec![0.0f32; padded_dim];
        padded[..orig_dim].copy_from_slice(&data[offset..offset + orig_dim]);

        // In-place WHT: O(d log d)
        let mut step = 1;
        while step < padded_dim {
            for i in (0..padded_dim).step_by(step * 2) {
                for j in 0..step {
                    let a = padded[i + j];
                    let b = padded[i + j + step];
                    padded[i + j] = a + b;
                    padded[i + j + step] = a - b;
                }
            }
            step *= 2;
        }

        // Apply random signs
        for i in 0..padded_dim {
            padded[i] *= signs[i];
        }

        // Scale by 1/sqrt(padded_dim)
        let scale = 1.0 / (padded_dim as f32).sqrt();
        for item in padded.iter_mut().take(padded_dim) {
            *item *= scale;
        }

        // Copy back (truncate to original dim)
        data[offset..offset + orig_dim].copy_from_slice(&padded[..orig_dim]);
    }

    Ok(())
}

/// Generate deterministic ±1 signs from seed (as f32).
fn generate_signs(dim: usize, seed: u64) -> Vec<f32> {
    generate_signs_impl(dim, seed)
        .into_iter()
        .map(|s| s as f32)
        .collect()
}

/// Generate deterministic ±1 i32 signs from seed.
pub fn generate_signs_i32(dim: usize, seed: u64) -> Vec<i32> {
    generate_signs_impl(dim, seed)
}

fn generate_signs_impl(dim: usize, seed: u64) -> Vec<i32> {
    let mut state = seed;
    let mut signs = Vec::with_capacity(dim);
    for _ in 0..dim {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        signs.push(if (state >> 32) & 1 == 0 { -1 } else { 1 });
    }
    signs
}

/// Lloyd-Max batch quantization on CPU.
pub fn lloyd_max_batch_cpu(
    vectors: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    _seed: u64,
) -> std::result::Result<(Vec<u8>, Vec<f32>), String> {
    if k == 0 {
        return Err("k must be greater than zero".to_string());
    }
    if n_levels > 256 {
        return Err("n_levels cannot exceed 256".to_string());
    }

    let total_elements = n
        .checked_mul(dim)
        .ok_or_else(|| "n * dim overflows usize".to_string())?;
    if vectors.len() != total_elements {
        return Err(format!(
            "dimension mismatch: expected {total_elements}, got {}",
            vectors.len()
        ));
    }

    if dim % k != 0 {
        return Err(format!("dim ({}) must be divisible by k ({})", dim, k));
    }

    let blocks_per_vector = dim / k;
    let total_blocks = n
        .checked_mul(blocks_per_vector)
        .ok_or_else(|| "n * blocks_per_vector overflows usize".to_string())?;
    let total_indices = total_blocks
        .checked_mul(k)
        .ok_or_else(|| "total_blocks * k overflows usize".to_string())?;
    let mut indices = vec![0u8; total_blocks * k];
    let mut norms = vec![0.0f32; total_blocks];

    // Pre-compute codebook centroids for N(0,1)
    let centroids = gaussian_centroids(n_levels);

    for (vec_idx, chunk) in vectors.chunks_exact(dim).enumerate() {
        for block_idx in 0..blocks_per_vector {
            let start = block_idx * k;
            let block = &chunk[start..start + k];

            // Compute L2 norm
            let norm_sq: f32 = block.iter().map(|v| v * v).sum();
            let norm = norm_sq.sqrt();
            let block_offset = vec_idx * blocks_per_vector + block_idx;
            norms[block_offset] = norm;

            // Normalize and quantize each scalar
            if norm > 1e-10 {
                for (j, &val) in block.iter().enumerate() {
                    let normalized = val / norm;

                    // Find nearest centroid
                    let mut best_dist = f32::MAX;
                    let mut best_idx = 0u8;
                    for (c_idx, &c) in centroids.iter().enumerate() {
                        let dist = (normalized - c).abs();
                        if dist < best_dist {
                            best_dist = dist;
                            best_idx = c_idx as u8;
                        }
                    }
                    let index_pos = vec_idx * blocks_per_vector * k + block_idx * k + j;
                    indices[index_pos] = best_idx;
                }
            }
            // If norm is near zero, indices stay at 0 (which is correct)
        }
    }

    if indices.len() != total_indices {
        return Err("internal overflow in output size calculation".to_string());
    }

    Ok((indices, norms))
}

/// Lloyd-Max decode on CPU.
pub fn lloyd_max_decode_batch_cpu(
    indices: &[u8],
    norms: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    __seed: u64,
) -> std::result::Result<Vec<f32>, String> {
    if k == 0 {
        return Err("k must be greater than zero".to_string());
    }
    if n_levels == 0 {
        return Err("n_levels must be greater than zero".to_string());
    }
    if n_levels > 256 {
        return Err("n_levels cannot exceed 256".to_string());
    }

    if norms.is_empty() {
        return Err("norms input must not be empty".to_string());
    }

    if dim % k != 0 {
        return Err(format!("dim ({}) must be divisible by k ({})", dim, k));
    }

    let blocks_per_vector = dim / k;
    let expected_output_len = n
        .checked_mul(dim)
        .ok_or_else(|| "n * dim overflows usize".to_string())?;
    let total_blocks = n
        .checked_mul(blocks_per_vector)
        .ok_or_else(|| "n * blocks_per_vector overflows usize".to_string())?;
    if norms.len() != total_blocks {
        return Err(format!(
            "dimension mismatch: expected {}, got {}",
            total_blocks,
            norms.len()
        ));
    }

    let centroids = gaussian_centroids(n_levels);
    let mut output = vec![0.0f32; expected_output_len];

    for vec_idx in 0..n {
        for block_idx in 0..blocks_per_vector {
            let block_offset = vec_idx * blocks_per_vector + block_idx;
            let norm = norms[block_offset];
            let out_start = vec_idx * dim + block_idx * k;

            for j in 0..k {
                let index_pos = vec_idx * blocks_per_vector * k + block_idx * k + j;
                let idx = indices[index_pos] as usize;
                let centroid = centroids.get(idx).copied().unwrap_or(0.0);
                output[out_start + j] = centroid * norm;
            }
        }
    }

    Ok(output)
}

/// Pre-computed optimal Lloyd-Max centroids for N(0,1) distribution.
fn gaussian_centroids(n_levels: usize) -> Vec<f32> {
    match n_levels {
        4 => vec![-1.510, -0.453, 0.453, 1.510],
        8 => vec![-2.152, -1.344, -0.756, -0.245, 0.245, 0.756, 1.344, 2.152],
        16 => vec![
            -2.637, -2.028, -1.579, -1.212, -0.891, -0.599, -0.327, -0.067, 0.067, 0.327, 0.599,
            0.891, 1.212, 1.579, 2.028, 2.637,
        ],
        32 => vec![
            -3.109, -2.643, -2.305, -2.029, -1.790, -1.577, -1.383, -1.204, -1.036, -0.878, -0.727,
            -0.582, -0.441, -0.304, -0.169, -0.036, 0.036, 0.169, 0.304, 0.441, 0.582, 0.727,
            0.878, 1.036, 1.204, 1.383, 1.577, 1.790, 2.029, 2.305, 2.643, 3.109,
        ],
        _ => {
            // Generate approximate centroids for arbitrary N
            let mut c = Vec::with_capacity(n_levels);
            for i in 0..n_levels {
                let p = (i as f64 + 0.5) / n_levels as f64;
                // Inverse CDF approximation for N(0,1)
                let z = probit(p);
                c.push(z as f32);
            }
            c
        }
    }
}

/// Approximate inverse normal CDF (probit function).
fn probit(p: f64) -> f64 {
    use std::f64::consts::SQRT_2;
    // Rational approximation (Abramowitz and Stegun)
    let t = (2.0 * p.min(1.0 - p)).ln().sqrt();
    let c = [2.515517, 0.802853, 0.010328];
    let d = [1.432788, 0.189269, 0.001308];
    let num = c[0] + c[1] * t + c[2] * t * t;
    let denom = 1.0 + d[0] * t + d[1] * t * t + d[2] * t * t * t;
    let z = t - num / denom;
    if p < 0.5 {
        -SQRT_2 * z
    } else {
        SQRT_2 * z
    }
}

/// Bit-pack on CPU.
pub fn bitpack_cpu(indices: &[u8], bits_per_index: usize) -> Result<Vec<u8>> {
    let total_bits = indices.len() * bits_per_index;
    let packed_len = total_bits.div_ceil(8);
    let mut packed = vec![0u8; packed_len];

    for (i, &idx) in indices.iter().enumerate() {
        let bit_offset = i * bits_per_index;
        let byte_offset = bit_offset / 8;
        let bit_shift = bit_offset % 8;
        let value = (idx as u64) & ((1u64 << bits_per_index) - 1);

        // May span two bytes
        let byte0 = byte_offset;
        packed[byte0] |= (value << bit_shift) as u8;
        if bit_shift + bits_per_index > 8 && byte_offset + 1 < packed_len {
            packed[byte0 + 1] |= (value >> (8 - bit_shift)) as u8;
        }
    }

    Ok(packed)
}

/// Nearest-codeword index lookup on CPU.
///
/// For each (vector, sub-block) pair in `input` (shape `[n × d]`), finds
/// the index of the codeword in `codebook` (shape `[N × k]`, row-major)
/// that minimizes the squared L2 distance. The returned `Vec<u32>` is
/// length `n * (d / k)` in row-major (vector, sub-block) order.
///
/// Uses `f32` throughout to match the GPU kernel's precision; this
/// differs from `fib_quant::nearest_index` which uses `f64` and may
/// produce different argmins at edge cases (very close codewords).
pub fn codebook_lookup_cpu(
    input: &[f32],
    codebook: &[f32],
    n: usize,
    d: usize,
    k: usize,
) -> Result<Vec<u32>> {
    if input.len() != n * d {
        return Err(GpuError::DimensionMismatch {
            expected: n * d,
            got: input.len(),
        });
    }
    if d % k != 0 {
        return Err(GpuError::InvalidConfig(format!(
            "dim ({}) must be divisible by k ({})",
            d, k
        )));
    }
    let blocks_per_vector = d / k;
    let n_codewords = codebook.len() / k;
    if codebook.len() != n_codewords * k {
        return Err(GpuError::DimensionMismatch {
            expected: n_codewords * k,
            got: codebook.len(),
        });
    }

    let mut out = Vec::with_capacity(n * blocks_per_vector);
    for vec_idx in 0..n {
        let vec_base = vec_idx * d;
        for sub_idx in 0..blocks_per_vector {
            let sub_base = vec_base + sub_idx * k;
            let mut best_idx: u32 = 0;
            let mut best_dist: f32 = f32::INFINITY;
            for c in 0..n_codewords {
                let cw_base = c * k;
                let mut dist: f32 = 0.0;
                for j in 0..k {
                    let delta = input[sub_base + j] - codebook[cw_base + j];
                    dist += delta * delta;
                }
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = c as u32;
                }
            }
            out.push(best_idx);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hadamard_batch() {
        // Test with power-of-2 dim
        let dim = 8;
        let n = 2;
        let mut data = vec![0.0f32; n * dim];
        for v in 0..n {
            data[v * dim] = 1.0;
        }
        hadamard_batch_cpu(&mut data, n, dim, 42).unwrap();
        let expected_mag = 1.0 / (dim as f32).sqrt();
        for i in 0..n {
            for j in 0..dim {
                let val = data[i * dim + j].abs();
                assert!(
                    (val - expected_mag).abs() < 1e-5,
                    "vector {} element {}: expected {}, got {}",
                    i,
                    j,
                    expected_mag,
                    val
                );
            }
        }
    }

    #[test]
    fn test_hadamard_non_power_of_two() {
        // 768-dim (not power of 2) — should pad to 1024
        let dim = 768;
        let n = 1;
        let mut data = vec![0.0f32; n * dim];
        data[0] = 1.0;
        hadamard_batch_cpu(&mut data, n, dim, 42).unwrap();
        // After padding and truncation, data should not be all zeros
        let all_zero = data.iter().all(|&v| v == 0.0);
        assert!(!all_zero, "padded hadamard should produce non-zero output");
        assert_eq!(data.len(), n * dim, "output length preserved");
    }

    #[test]
    fn test_hadamard_deterministic() {
        let dim = 8;
        let n = 1;
        let data1 = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let data2 = data1.clone();

        let mut d1 = data1;
        let mut d2 = data2;
        hadamard_batch_cpu(&mut d1, n, dim, 42).unwrap();
        hadamard_batch_cpu(&mut d2, n, dim, 42).unwrap();

        assert_eq!(d1, d2);
    }

    #[test]
    fn lloyd_batch_rejects_zero_k() {
        let result = lloyd_max_batch_cpu(&[0.0], 1, 1, 0, 8, 42);
        assert!(result.is_err());
    }

    #[test]
    fn lloyd_batch_rejects_large_level_count() {
        let result = lloyd_max_batch_cpu(&[0.0; 4], 1, 4, 2, 257, 42);
        assert!(result.is_err());
    }

    #[test]
    fn lloyd_batch_rejects_dimension_overflow() {
        let vectors = vec![0.0f32; 1];
        let result = lloyd_max_batch_cpu(&vectors, usize::MAX, 2, 1, 8, 42);
        assert!(result.is_err());
    }

    #[test]
    fn lloyd_decode_rejects_empty_norms() {
        let result = lloyd_max_decode_batch_cpu(&[0u8; 4], &[], 1, 4, 2, 8, 42);
        assert!(result.is_err());
    }

    #[test]
    fn lloyd_decode_rejects_large_level_count() {
        let result = lloyd_max_decode_batch_cpu(&[0u8; 4], &[1.0, 2.0], 1, 4, 2, 257, 42);
        assert!(result.is_err());
    }

    #[test]
    fn test_lloyd_max_roundtrip() {
        let dim = 16;
        let k = 4;
        let n_levels = 8;
        let n = 4;

        let vectors: Vec<f32> = (0..n * dim).map(|i| (i as f32 * 0.1).sin()).collect();

        let (indices, norms) = lloyd_max_batch_cpu(&vectors, n, dim, k, n_levels, 42).unwrap();
        assert_eq!(indices.len(), n * dim); // k indices per block, dim/k blocks = dim indices total
        assert_eq!(norms.len(), n * dim / k);

        let decoded =
            lloyd_max_decode_batch_cpu(&indices, &norms, n, dim, k, n_levels, 42).unwrap();
        assert_eq!(decoded.len(), n * dim);

        // Check non-zero reconstruction
        let all_zero = decoded.iter().all(|&v| v == 0.0);
        assert!(!all_zero, "decoded output should not be all zeros");
    }

    #[test]
    fn test_bitpack_roundtrip() {
        let indices = vec![0u8, 1, 2, 3, 0, 1, 2, 3];
        let bits = 2;

        let packed = bitpack_cpu(&indices, bits).unwrap();
        // 8 indices × 2 bits = 16 bits = 2 bytes
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0], 0b11100100u8); // 0,1,2,3 packed into bits 0-7
    }

    #[test]
    fn test_gaussian_centroids_size() {
        assert_eq!(gaussian_centroids(4).len(), 4);
        assert_eq!(gaussian_centroids(8).len(), 8);
        assert_eq!(gaussian_centroids(16).len(), 16);
        assert_eq!(gaussian_centroids(32).len(), 32);
        assert_eq!(gaussian_centroids(7).len(), 7); // arbitrary
    }

    #[test]
    fn test_codebook_lookup_basic() {
        // 2 vectors of dim 8, k=4, N=4 codewords
        // Vector 0: first sub-block [1,0,0,0] should match codeword 0 exactly
        //           second sub-block [0,0,0,1] should match codeword 1 exactly
        // Vector 1: first sub-block [0,1,0,0] should match codeword 2 exactly
        //           second sub-block [0,0,1,0] should match codeword 3 exactly
        let input: Vec<f32> = vec![
            1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, // vector 0
            0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, // vector 1
        ];
        let codebook: Vec<f32> = vec![
            1.0, 0.0, 0.0, 0.0, // codeword 0
            0.0, 0.0, 0.0, 1.0, // codeword 1
            0.0, 1.0, 0.0, 0.0, // codeword 2
            0.0, 0.0, 1.0, 0.0, // codeword 3
        ];
        let n = 2;
        let d = 8;
        let k = 4;
        let indices = codebook_lookup_cpu(&input, &codebook, n, d, k).unwrap();
        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn test_codebook_lookup_nearest_wins() {
        // When the exact match isn't available, the closest codeword wins.
        // Input sub-block [1, 1, 0, 0] (norm sqrt(2)) is equidistant from
        // codewords [1, 0, 0, 0] and [0, 1, 0, 0] — both with squared
        // distance 1.0. The argmin should pick the first (lowest index)
        // since we use strict `<` comparison.
        let input: Vec<f32> = vec![1.0, 1.0, 0.0, 0.0];
        let codebook: Vec<f32> = vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let indices = codebook_lookup_cpu(&input, &codebook, 1, 4, 4).unwrap();
        assert_eq!(indices, vec![0], "tie should resolve to lowest index");
    }

    #[test]
    fn test_codebook_lookup_dimension_mismatch() {
        // input length doesn't match n*d
        let input = vec![0.0f32; 4];
        let codebook = vec![0.0f32; 16];
        let result = codebook_lookup_cpu(&input, &codebook, 1, 8, 4);
        assert!(result.is_err(), "input length mismatch should error");
    }
}
