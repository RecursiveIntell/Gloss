use std::fmt;

/// GPU backend errors.
#[derive(Debug)]
pub enum GpuError {
    /// No CUDA device available.
    GpuUnavailable,
    /// Device memory insufficient.
    OutOfMemory { requested: usize, available: usize },
    /// Input dimension mismatch.
    DimensionMismatch { expected: usize, got: usize },
    /// Invalid configuration parameter.
    InvalidConfig(String),
    /// CUDA runtime error.
    CudaError(String),
    /// Internal error.
    Internal(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GpuUnavailable => write!(
                f,
                "GPU unavailable — CUDA device not found or feature disabled"
            ),
            Self::OutOfMemory {
                requested,
                available,
            } => {
                write!(
                    f,
                    "GPU out of memory: requested {} bytes, available {} bytes",
                    requested, available
                )
            }
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {}, got {}", expected, got)
            }
            Self::InvalidConfig(msg) => write!(f, "invalid config: {}", msg),
            Self::CudaError(msg) => write!(f, "CUDA error: {}", msg),
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for GpuError {}
