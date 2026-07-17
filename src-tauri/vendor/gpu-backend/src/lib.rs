//! GPU backend crate for vector quantization acceleration.
//!
//! Provides CUDA kernels for:
//! - Fast Walsh-Hadamard Transform (WHT) — shared by fib-quant and turbo-quant
//! - Lloyd-Max scalar quantization — per-coordinate codebook quantization
//! - Bit-packing — compact index storage
//!
//! Feature-gated: `gpu` feature enables CUDA via cudarc.
//! Without it, this crate is a stub — all operations return `GpuUnavailable`.

use std::sync::OnceLock;

#[cfg(feature = "gpu")]
pub mod cuda;
pub mod error;
pub mod fallback;
pub mod simd_nearest;
pub mod validated;

pub use error::GpuError;
pub use validated::{QuantProfile, TensorShape};

/// Result type for GPU operations.
pub type Result<T> = std::result::Result<T, GpuError>;

/// Global GPU context — initialized once, shared across crates.
static GPU_CTX: OnceLock<Option<GpuContext>> = OnceLock::new();

/// GPU context holding device, stream, and compiled kernels.
#[derive(Debug)]
pub struct GpuContext {
    /// CUDA device index
    pub device_index: u32,
    /// Available device memory in bytes
    pub memory_bytes: usize,
    /// Device name
    pub device_name: String,
}

impl GpuContext {
    /// Initialize GPU context. Returns None if no CUDA device is available
    /// or the `gpu` feature is disabled.
    pub fn init() -> Option<&'static GpuContext> {
        GPU_CTX.get_or_init(|| {
            #[cfg(feature = "gpu")]
            {
                cuda::init_context().ok()
            }
            #[cfg(not(feature = "gpu"))]
            {
                None
            }
        });
        GPU_CTX.get().and_then(|c| c.as_ref())
    }

    /// Check if GPU acceleration is available.
    pub fn is_available() -> bool {
        Self::init().is_some()
    }

    /// Minimum batch size for GPU to be worth the launch overhead.
    pub const GPU_MIN_BATCH_SIZE: usize = 16;
    /// Minimum dimension for GPU acceleration (small dims are faster on CPU).
    pub const GPU_MIN_DIM: usize = 64;
}

/// Batched Hadamard Walsh-Hadamard Transform.
///
/// Applies in-place WHT to `n` vectors of length `dim`.
/// `dim` must be a power of 2. Pad input before calling.
/// Uses GPU if available and batch size warrants it.
pub fn hadamard_batch(data: &mut [f32], n: usize, dim: usize, seed: u64) -> Result<()> {
    if data.len() != n * dim {
        return Err(GpuError::DimensionMismatch {
            expected: n * dim,
            got: data.len(),
        });
    }

    #[cfg(feature = "gpu")]
    {
        if let Some(ctx) = GpuContext::init() {
            if n >= GpuContext::GPU_MIN_BATCH_SIZE && dim >= GpuContext::GPU_MIN_DIM {
                return cuda::hadamard_batch_gpu(ctx, data, n, dim, seed);
            }
        }
    }

    // CPU fallback
    fallback::hadamard_batch_cpu(data, n, dim, seed)
}

/// Batched Lloyd-Max quantization (QUANT-001 hardened).
///
/// Quantizes a block of `n` vectors, each of `dim` scalars, into
/// `n_levels` codebook entries per block of size `k`.
///
/// Returns (indices, norms) where:
/// - indices: flat u8 array of length n * (dim / k) — codebook indices
/// - norms: flat f32 array of length n * (dim / k) — per-block L2 norms
pub fn lloyd_max_batch(
    vectors: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    seed: u64,
) -> Result<(Vec<u8>, Vec<f32>)> {
    // QUANT-001: validate all parameters before dispatch
    let shape = TensorShape::new(n, dim)?;
    shape.validate_data(vectors)?;
    // k==0 is caught by QuantProfile::new
    let _profile = QuantProfile::new(k, n_levels, n_levels.ilog2() as usize + 1)?;

    #[cfg(feature = "gpu")]
    {
        if let Some(ctx) = GpuContext::init() {
            if n >= GpuContext::GPU_MIN_BATCH_SIZE {
                return cuda::lloyd_max_batch_gpu(ctx, vectors, n, dim, k, n_levels, seed)
                    .map_err(GpuError::InvalidConfig);
            }
        }
    }

    fallback::lloyd_max_batch_cpu(vectors, n, dim, k, n_levels, seed)
        .map_err(GpuError::InvalidConfig)
}

/// Batched Lloyd-Max decode (QUANT-001 hardened).
///
/// Reconstructs approximate f32 vectors from quantized indices and norms.
pub fn lloyd_max_decode_batch(
    indices: &[u8],
    norms: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    seed: u64,
) -> Result<Vec<f32>> {
    // QUANT-001: validate all parameters
    let shape = TensorShape::new(n, dim)?;
    let profile = QuantProfile::new(k, n_levels, n_levels.ilog2() as usize + 1)?;
    profile.validate_indices(&shape, indices)?;
    profile.validate_norms(&shape, norms)?;

    #[cfg(feature = "gpu")]
    {
        if let Some(ctx) = GpuContext::init() {
            if n >= GpuContext::GPU_MIN_BATCH_SIZE {
                return cuda::lloyd_max_decode_batch_gpu(
                    ctx, indices, norms, n, dim, k, n_levels, seed,
                )
                .map_err(GpuError::InvalidConfig);
            }
        }
    }

    fallback::lloyd_max_decode_batch_cpu(indices, norms, n, dim, k, n_levels, seed)
        .map_err(GpuError::InvalidConfig)
}

/// Bit-pack quantized indices into compact byte array.
///
/// Input: flat u8 array where each byte is a codebook index (0..n_levels-1).
/// Output: packed bytes using `bits_per_index` bits per index.
pub fn bitpack(indices: &[u8], bits_per_index: usize) -> Result<Vec<u8>> {
    if bits_per_index == 0 || bits_per_index > 8 {
        return Err(GpuError::InvalidConfig(format!(
            "bits_per_index must be 1-8, got {}",
            bits_per_index
        )));
    }

    #[cfg(feature = "gpu")]
    {
        if let Some(ctx) = GpuContext::init() {
            if indices.len() >= 1024 {
                return cuda::bitpack_gpu(ctx, indices, bits_per_index);
            }
        }
    }

    fallback::bitpack_cpu(indices, bits_per_index)
}

/// Nearest-codeword index lookup for fib-quant / vector quantization.
///
/// For each (vector, sub-block) pair in `input` (shape `[n × d]`, row-major
/// f32), finds the index of the codeword in `codebook` (shape `[N × k]`,
/// row-major f32) that minimizes the squared L2 distance. Returns
/// `n * (d / k)` u32 indices in row-major (vector, sub-block) order.
///
/// Uses GPU when available and the codebook size `N <= 32` (the kernel
/// is one warp wide). Falls back to CPU for larger codebooks.
///
/// This is the operation that dominates fib-quant's `encode_batch` after
/// the Hadamard rotation — for k=4, N=32, d=128, n=80 it runs ~1.5M
/// argmin computations and is the bottleneck of the poly-kv pool build.
pub fn codebook_lookup_batch(
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
    let n_codewords = codebook.len() / k;
    if n_codewords > 32 {
        // GPU kernel hard-codes 32-thread warp; fall back to CPU.
        return fallback::codebook_lookup_cpu(input, codebook, n, d, k);
    }

    #[cfg(feature = "gpu")]
    {
        if let Some(ctx) = GpuContext::init() {
            if n >= GpuContext::GPU_MIN_BATCH_SIZE && d >= GpuContext::GPU_MIN_DIM {
                return cuda::codebook_lookup_batch_gpu(ctx, input, codebook, n, d, k);
            }
        }
    }

    fallback::codebook_lookup_cpu(input, codebook, n, d, k)
}

/// True if a specific call to [`codebook_lookup_batch`] would dispatch
/// to GPU. Requires the codebook size to fit in a single warp (N <= 32)
/// and the standard batch/dim thresholds.
pub fn codebook_lookup_supports_gpu(n: usize, d: usize, n_codewords: usize) -> bool {
    n_codewords <= 32
        && n >= GpuContext::GPU_MIN_BATCH_SIZE
        && d >= GpuContext::GPU_MIN_DIM
        && GpuContext::is_available()
}

#[cfg(all(test, feature = "gpu"))]
mod gpu_parity_tests {
    use super::*;
    use crate::fallback::codebook_lookup_cpu;

    /// When the GPU is available and the kernel can be reached, the GPU
    /// result must be byte-identical to the CPU fallback. This is the
    /// receipt-honesty test: if it fails, the GPU path is silently
    /// producing different codebook indices and pool digests will drift.
    #[test]
    fn test_codebook_lookup_gpu_matches_cpu() {
        if !GpuContext::is_available() {
            // Skip — GPU not present in this build environment
            eprintln!("GPU not available, skipping parity test");
            return;
        }

        // Build a deterministic test case.
        let n: usize = 32;
        let d: usize = 128;
        let k: usize = 4;
        let n_codewords: usize = 32;
        let mut seed: u64 = 0xDEAD_BEEF;
        let next = |s: &mut u64| {
            *s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *s
        };

        // Random codebook and inputs.
        let codebook: Vec<f32> = (0..n_codewords * k)
            .map(|_| {
                let v = (next(&mut seed) >> 32) as i32;
                (v as f32 / i32::MAX as f32) * 0.5
            })
            .collect();
        let input: Vec<f32> = (0..n * d)
            .map(|_| {
                let v = (next(&mut seed) >> 32) as i32;
                (v as f32 / i32::MAX as f32) * 0.5
            })
            .collect();

        let cpu = codebook_lookup_cpu(&input, &codebook, n, d, k).expect("cpu fallback failed");
        let gpu = codebook_lookup_batch(&input, &codebook, n, d, k).expect("gpu dispatch failed");

        assert_eq!(
            cpu.len(),
            gpu.len(),
            "GPU returned {} indices, CPU returned {}",
            gpu.len(),
            cpu.len()
        );
        let mismatches = cpu.iter().zip(gpu.iter()).filter(|(a, b)| a != b).count();
        assert_eq!(
            mismatches,
            0,
            "GPU codebook lookup produced {} differing indices out of {}",
            mismatches,
            cpu.len()
        );
    }
}

/// AVX2-accelerated nearest-codeword index lookup (CPU).
///
/// Returns the index of the codeword in `codebook` (row-major f32, shape
/// `[N × k]`) that minimizes the squared L2 distance from `sample`.
///
/// On x86_64 with AVX2+FMA, this runs ~4-8× faster than a naive scalar
/// loop for the k=4 case. Falls back to a scalar loop on other platforms
/// or when AVX2 isn't available at runtime.
///
/// The result is byte-identical to a scalar f32 reference within f32
/// precision. fib-quant's parity tests assert that this matches the
/// canonical f64 reference for trained Lloyd-Max codebooks.
pub fn nearest_codeword_f32(sample: &[f32], codebook: &[f32], k: usize) -> usize {
    simd_nearest::nearest_codeword_f32(sample, codebook, k)
}
