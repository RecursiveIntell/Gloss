use thiserror::Error;

#[derive(Debug, Error)]
pub enum TurboQuantError {
    #[error("dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: usize, got: usize },

    #[error("dimension must be even for polar encoding, got {got}")]
    OddDimension { got: usize },

    #[error("dimension must be non-zero")]
    ZeroDimension,

    #[error("bits must be between 1 and 16, got {got}")]
    InvalidBitWidth { got: u8 },

    #[error("projection count must be non-zero")]
    ZeroProjectionCount,

    #[error("rotation matrix generation failed: {reason}")]
    RotationFailed { reason: String },

    #[error("polar code is malformed: {reason}")]
    MalformedCode { reason: String },

    #[error("input vector contains non-finite value at index {index}")]
    NonFiniteInput { index: usize },

    #[error("codec profile mismatch: {reason}")]
    ProfileMismatch { reason: String },
}

pub type Result<T> = std::result::Result<T, TurboQuantError>;
