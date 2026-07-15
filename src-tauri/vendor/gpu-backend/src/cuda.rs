//! CUDA-accelerated GPU operations via cudarc driver API.
//!
//! Uses CUDA driver dynamic loading — no nvcc needed at build time.
//! PTX kernels are loaded from file at runtime.
//! Falls back to CPU if no CUDA device or PTX unavailable.

use crate::error::GpuError;
use crate::GpuContext;
use crate::Result;
use std::sync::{Arc, OnceLock};

use cudarc::driver::PushKernelArg;

/// Lazily-initialized CUDA state.
static CUDA_STATE: OnceLock<Option<CudaState>> = OnceLock::new();

struct CudaState {
    stream: Arc<cudarc::driver::CudaStream>,
    module: Arc<cudarc::driver::CudaModule>,
}

/// Initialize CUDA context and probe device capabilities.
pub fn init_context() -> Result<GpuContext> {
    use cudarc::driver::CudaContext;

    let ctx = CudaContext::new(0).map_err(|_| GpuError::GpuUnavailable)?;

    let name = ctx.name().unwrap_or_else(|_| "unknown".into());
    let memory_bytes = ctx.total_mem().unwrap_or(0);

    // Lazy-init full state
    let _ = CUDA_STATE.get_or_init(|| init_cuda_state(&ctx));

    Ok(GpuContext {
        device_index: 0,
        memory_bytes,
        device_name: name,
    })
}

/// Initialize PTX module.
fn init_cuda_state(ctx: &Arc<cudarc::driver::CudaContext>) -> Option<CudaState> {
    let ptx = load_ptx()?;
    let module = match ctx.load_module(ptx) {
        Ok(m) => m,
        Err(_) => return None,
    };
    let stream = ctx.default_stream();
    Some(CudaState { stream, module })
}

/// Load PTX from file (pre-compiled) or embedded source.
fn load_ptx() -> Option<cudarc::nvrtc::Ptx> {
    #[cfg(feature = "precompiled-ptx")]
    {
        // Try loading from the crate's kernels directory
        let ptx_path = concat!(env!("CARGO_MANIFEST_DIR"), "/kernels/combined.ptx");
        if std::path::Path::new(ptx_path).exists() {
            return Some(cudarc::nvrtc::Ptx::from_file(ptx_path));
        }
    }
    // PTX not available — CPU fallback
    None
}

/// Check if CUDA state is ready (PTX loaded, kernels available).
fn cuda_ready() -> bool {
    CUDA_STATE.get().map(|s| s.is_some()).unwrap_or(false)
}

// ── Hadamard Batch ──

pub fn hadamard_batch_gpu(
    _ctx: &GpuContext,
    data: &mut [f32],
    n: usize,
    dim: usize,
    seed: u64,
) -> Result<()> {
    if !cuda_ready() {
        return crate::fallback::hadamard_batch_cpu(data, n, dim, seed);
    }
    hadamard_batch_cuda(data, n, dim, seed)
}

fn hadamard_batch_cuda(data: &mut [f32], n: usize, dim: usize, seed: u64) -> Result<()> {
    let state = CUDA_STATE.get().and_then(|s| s.as_ref()).unwrap();
    let signs = crate::fallback::generate_signs_i32(dim, seed);

    let dev_signs = state
        .stream
        .clone_htod(signs.as_slice())
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_data = state
        .stream
        .clone_htod(data)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let f = state
        .module
        .load_function("hadamard_wht_batch")
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let block_dim = dim.min(1024);
    let shared_bytes = (dim * std::mem::size_of::<f32>()) as u32;
    let cfg = cudarc::driver::LaunchConfig {
        grid_dim: (n as u32, 1, 1),
        block_dim: (block_dim as u32, 1, 1),
        shared_mem_bytes: shared_bytes,
    };
    let ni = n as i32;
    let di = dim as i32;

    let mut args = state.stream.launch_builder(&f);
    args.arg(&mut dev_data);
    args.arg(&dev_signs);
    args.arg(&ni);
    args.arg(&di);
    unsafe { args.launch(cfg) }.map_err(|e| GpuError::CudaError(format!("hadamard: {}", e)))?;

    state
        .stream
        .memcpy_dtoh(&dev_data, data)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .synchronize()
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    Ok(())
}

// ── Lloyd-Max ──

pub fn lloyd_max_batch_gpu(
    _ctx: &GpuContext,
    vectors: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    seed: u64,
) -> Result<(Vec<u8>, Vec<f32>)> {
    if !cuda_ready() {
        return crate::fallback::lloyd_max_batch_cpu(vectors, n, dim, k, n_levels, seed)
            .map_err(GpuError::InvalidConfig);
    }
    lloyd_max_batch_cuda(vectors, n, dim, k, n_levels, seed)
}

fn lloyd_max_batch_cuda(
    vectors: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    _seed: u64,
) -> Result<(Vec<u8>, Vec<f32>)> {
    let state = CUDA_STATE.get().and_then(|s| s.as_ref()).unwrap();
    let blocks_per_vector = dim / k;
    let total_blocks = n * blocks_per_vector;
    let total_scalars = total_blocks * k;

    let dev_input = state
        .stream
        .clone_htod(vectors)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_indices = state
        .stream
        .alloc_zeros::<u8>(total_scalars)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_norms = state
        .stream
        .alloc_zeros::<f32>(total_blocks)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let f = state
        .module
        .load_function("lloyd_max_encode")
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let shared = (k * std::mem::size_of::<f32>()) as u32;
    let cfg = cudarc::driver::LaunchConfig {
        grid_dim: (total_blocks as u32, 1, 1),
        block_dim: (k as u32, 1, 1),
        shared_mem_bytes: shared,
    };
    let ni = n as i32;
    let di = dim as i32;
    let ki = k as i32;
    let li = n_levels as i32;

    let mut args = state.stream.launch_builder(&f);
    args.arg(&dev_input);
    args.arg(&mut dev_indices);
    args.arg(&mut dev_norms);
    args.arg(&ni);
    args.arg(&di);
    args.arg(&ki);
    args.arg(&li);
    unsafe { args.launch(cfg) }.map_err(|e| GpuError::CudaError(format!("lloyd encode: {}", e)))?;

    let mut indices = vec![0u8; total_scalars];
    let mut norms = vec![0.0f32; total_blocks];
    state
        .stream
        .memcpy_dtoh(&dev_indices, &mut indices)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .memcpy_dtoh(&dev_norms, &mut norms)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .synchronize()
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    Ok((indices, norms))
}

#[allow(clippy::too_many_arguments)]
pub fn lloyd_max_decode_batch_gpu(
    _ctx: &GpuContext,
    indices: &[u8],
    norms: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    seed: u64,
) -> Result<Vec<f32>> {
    if !cuda_ready() {
        return crate::fallback::lloyd_max_decode_batch_cpu(
            indices, norms, n, dim, k, n_levels, seed,
        )
        .map_err(GpuError::InvalidConfig);
    }
    lloyd_max_decode_cuda(indices, norms, n, dim, k, n_levels, seed)
}

fn lloyd_max_decode_cuda(
    indices: &[u8],
    norms: &[f32],
    n: usize,
    dim: usize,
    k: usize,
    n_levels: usize,
    _seed: u64,
) -> Result<Vec<f32>> {
    let state = CUDA_STATE.get().and_then(|s| s.as_ref()).unwrap();
    let blocks_per_vector = dim / k;
    let total_blocks = n * blocks_per_vector;
    let total_out = n * dim;

    let dev_indices = state
        .stream
        .clone_htod(indices)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let dev_norms = state
        .stream
        .clone_htod(norms)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_out = state
        .stream
        .alloc_zeros::<f32>(total_out)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let f = state
        .module
        .load_function("lloyd_max_decode")
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let cfg = cudarc::driver::LaunchConfig {
        grid_dim: (total_blocks as u32, 1, 1),
        block_dim: (k as u32, 1, 1),
        shared_mem_bytes: 0,
    };
    let ni = n as i32;
    let di = dim as i32;
    let ki = k as i32;
    let li = n_levels as i32;

    let mut args = state.stream.launch_builder(&f);
    args.arg(&dev_indices);
    args.arg(&dev_norms);
    args.arg(&mut dev_out);
    args.arg(&ni);
    args.arg(&di);
    args.arg(&ki);
    args.arg(&li);
    unsafe { args.launch(cfg) }.map_err(|e| GpuError::CudaError(format!("lloyd decode: {}", e)))?;

    let mut output = vec![0.0f32; total_out];
    state
        .stream
        .memcpy_dtoh(&dev_out, &mut output)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .synchronize()
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    Ok(output)
}

// ── Bit Pack ──

pub fn bitpack_gpu(_ctx: &GpuContext, indices: &[u8], bits_per_index: usize) -> Result<Vec<u8>> {
    if !cuda_ready() {
        return crate::fallback::bitpack_cpu(indices, bits_per_index);
    }
    bitpack_cuda(indices, bits_per_index)
}

fn bitpack_cuda(indices: &[u8], bits_per_index: usize) -> Result<Vec<u8>> {
    let state = CUDA_STATE.get().and_then(|s| s.as_ref()).unwrap();
    let num_indices = indices.len();
    let packed_len = (num_indices * bits_per_index).div_ceil(8);

    let dev_indices = state
        .stream
        .clone_htod(indices)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_packed = state
        .stream
        .alloc_zeros::<u8>(packed_len)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let threads: u32 = 256;
    let blocks = (num_indices as u32).div_ceil(threads * 8).max(1);
    let cfg = cudarc::driver::LaunchConfig {
        grid_dim: (blocks, 1, 1),
        block_dim: (threads, 1, 1),
        shared_mem_bytes: 0,
    };

    let f = state
        .module
        .load_function("bitpack_batch")
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let ni = num_indices as i32;
    let bi = bits_per_index as i32;

    let mut args = state.stream.launch_builder(&f);
    args.arg(&dev_indices);
    args.arg(&mut dev_packed);
    args.arg(&ni);
    args.arg(&bi);
    unsafe { args.launch(cfg) }.map_err(|e| GpuError::CudaError(format!("bitpack: {}", e)))?;

    let mut packed = vec![0u8; packed_len];
    state
        .stream
        .memcpy_dtoh(&dev_packed, &mut packed)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .synchronize()
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    Ok(packed)
}

// ── Codebook Lookup ──

/// Find the nearest codeword index for each (vector, sub-block) pair.
///
/// `input` is `[n × d]` rotated f32 vectors. `codebook` is `[N × k]`
/// row-major f32. Returns a `Vec<u32>` of length `n * (d / k)` in
/// row-major (vector, sub-block) order.
///
/// One CUDA block per (vector, sub-block); one thread per codeword.
/// N must be <= 32 to fit in a single warp.
pub fn codebook_lookup_batch_gpu(
    _ctx: &GpuContext,
    input: &[f32],
    codebook: &[f32],
    n: usize,
    d: usize,
    k: usize,
) -> Result<Vec<u32>> {
    if !cuda_ready() {
        return crate::fallback::codebook_lookup_cpu(input, codebook, n, d, k);
    }
    codebook_lookup_cuda(input, codebook, n, d, k)
}

fn codebook_lookup_cuda(
    input: &[f32],
    codebook: &[f32],
    n: usize,
    d: usize,
    k: usize,
) -> Result<Vec<u32>> {
    let state = CUDA_STATE.get().and_then(|s| s.as_ref()).unwrap();
    let blocks_per_vector = d / k;
    let total_blocks = n * blocks_per_vector;
    let n_codewords = codebook.len() / k;

    if n_codewords > 32 {
        // Kernel hard-codes 32-thread block; for larger codebooks we'd
        // need a different launch shape. Fall back to CPU.
        return crate::fallback::codebook_lookup_cpu(input, codebook, n, d, k);
    }

    let dev_input = state
        .stream
        .clone_htod(input)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let dev_codebook = state
        .stream
        .clone_htod(codebook)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    let mut dev_out = state
        .stream
        .alloc_zeros::<u32>(total_blocks)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let f = state
        .module
        .load_function("codebook_lookup_kernel")
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    let shared_bytes = (k * std::mem::size_of::<f32>()) as u32;
    let cfg = cudarc::driver::LaunchConfig {
        grid_dim: (total_blocks as u32, 1, 1),
        block_dim: (n_codewords as u32, 1, 1),
        shared_mem_bytes: shared_bytes,
    };
    let ni = n as i32;
    let di = d as i32;
    let ki = k as i32;
    let li = n_codewords as i32;

    let mut args = state.stream.launch_builder(&f);
    args.arg(&dev_input);
    args.arg(&dev_codebook);
    args.arg(&mut dev_out);
    args.arg(&ni);
    args.arg(&di);
    args.arg(&ki);
    args.arg(&li);
    unsafe { args.launch(cfg) }
        .map_err(|e| GpuError::CudaError(format!("codebook_lookup: {}", e)))?;

    let mut out = vec![0u32; total_blocks];
    state
        .stream
        .memcpy_dtoh(&dev_out, &mut out)
        .map_err(|e| GpuError::CudaError(e.to_string()))?;
    state
        .stream
        .synchronize()
        .map_err(|e| GpuError::CudaError(e.to_string()))?;

    Ok(out)
}
