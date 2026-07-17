use thiserror::Error;

/// FibQuant crate result type.
pub type Result<T> = std::result::Result<T, FibQuantError>;

/// Fail-closed FibQuant error taxonomy.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum FibQuantError {
    /// Ambient dimension is zero.
    #[error("ambient dimension must be nonzero")]
    ZeroDimension,
    /// Block dimension is invalid for the ambient dimension.
    #[error("invalid block dimension {block_dim} for ambient dimension {ambient_dim}")]
    InvalidBlockDim {
        /// Ambient vector dimension.
        ambient_dim: usize,
        /// Requested block dimension.
        block_dim: usize,
    },
    /// `d` is not divisible by `k`.
    #[error("ambient dimension {ambient_dim} is not divisible by block dimension {block_dim}")]
    DimensionNotDivisible {
        /// Ambient vector dimension.
        ambient_dim: usize,
        /// Requested block dimension.
        block_dim: usize,
    },
    /// Codebook size is invalid.
    #[error("invalid codebook size {0}")]
    InvalidCodebookSize(usize),
    /// Input contains a non-finite value.
    #[error("non-finite input at index {0}")]
    NonFiniteInput(usize),
    /// Normal encode path received a zero vector.
    #[error("zero norm vector is not valid on the normal FibQuant encode path")]
    ZeroNorm,
    /// Stored profile digest did not match the expected digest.
    #[error("profile digest mismatch: expected {expected}, actual {actual}")]
    ProfileDigestMismatch {
        /// Expected digest.
        expected: String,
        /// Actual digest.
        actual: String,
    },
    /// Stored codebook digest did not match the expected digest.
    #[error("codebook digest mismatch: expected {expected}, actual {actual}")]
    CodebookDigestMismatch {
        /// Expected digest.
        expected: String,
        /// Actual digest.
        actual: String,
    },
    /// Stored rotation digest did not match the expected digest.
    #[error("rotation digest mismatch: expected {expected}, actual {actual}")]
    RotationDigestMismatch {
        /// Expected digest.
        expected: String,
        /// Actual digest.
        actual: String,
    },
    /// Payload is malformed.
    #[error("corrupt payload: {0}")]
    CorruptPayload(String),
    /// Requested dimensions or payload sizes exceed alpha release resource limits.
    #[error("resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),
    /// Decoded index is outside the codebook range.
    #[error("index {index} is outside codebook size {codebook_size}")]
    IndexOutOfRange {
        /// Invalid index.
        index: u32,
        /// Codebook size.
        codebook_size: u32,
    },
    /// Numerical algorithm failed.
    #[error("numerical failure: {0}")]
    NumericalFailure(String),
    /// Empty-cell repair failed during Lloyd-Max refinement.
    #[error("empty-cell repair failed: {0}")]
    EmptyCellRepairFailed(String),
    /// Required dependency behavior is unsupported.
    #[error("dependency unsupported: {0}")]
    DependencyUnsupported(String),
}
