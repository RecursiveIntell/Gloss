//! `scr-runtime-compression` — Runtime integration adapter for semantic-memory.
//!
//! Provides:
//! - [`CompressedSearchPath`] — search path wrapper that carries compression metadata
//! - [`ExactFallbackAdapter`] — decode adapter that decompresses via turbo-quant / fib-quant
//!
//! ## Design principles
//!
//! - **Never owns codec truth** — all compression/decompression is delegated to
//!   `turbo-quant` and `fib-quant`. This crate holds only the integration layer.
//! - **No `unwrap` in production paths** — all fallible operations return `Result` or `Option`.
//! - **Rust 2021, MSRV 1.75** — compatible with the workspace minimum.

mod codec_dispatch;
mod compressed_search_path;
mod error;
mod exact_fallback_adapter;

pub use codec_dispatch::{build_adapter, decode, encode, select_codec, CodecDispatch};
pub use compressed_search_path::CompressedSearchPath;
pub use error::{CompressionError, DecompressError};
pub use exact_fallback_adapter::ExactFallbackAdapter;

/// Codec identity for runtime dispatch.
///
/// Each compressed representation carries a codec discriminant so the adapter
/// can delegate to the correct encoder/decoder without static knowledge of the
/// underlying codec crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecId {
    /// Per-vector affine signed eight-bit scalar quantization.
    Q8,
    /// Per-vector affine signed four-bit scalar quantization, packed two values per byte.
    Q4,
    /// TurboQuant polar-code + residual-sketch codec.
    TurboQuant,
    /// FibQuant radial-angular codec.
    FibQuant,
    /// Polar-only quantization (asymmetric, no reconstruction).
    Polar,
    /// QJL random-projection sketch (asymmetric inner-product).
    Qjl,
    /// Uncompressed representation (identity pass-through).
    Uncompressed,
}

impl CodecId {
    /// Returns `true` if this codec requires exact fallback on decode.
    pub fn requires_exact_fallback(self) -> bool {
        matches!(self, Self::TurboQuant | Self::FibQuant)
    }
}

impl std::fmt::Display for CodecId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Q8 => write!(f, "q8"),
            Self::Q4 => write!(f, "q4"),
            Self::TurboQuant => write!(f, "turbo_quant"),
            Self::FibQuant => write!(f, "fib_quant"),
            Self::Polar => write!(f, "polar"),
            Self::Qjl => write!(f, "qjl"),
            Self::Uncompressed => write!(f, "uncompressed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CodecId;

    #[test]
    fn scalar_codec_ids_have_stable_wire_and_display_names() {
        for (codec, name) in [(CodecId::Q8, "q8"), (CodecId::Q4, "q4")] {
            assert_eq!(codec.to_string(), name);
            assert_eq!(
                serde_json::to_string(&codec).unwrap(),
                format!("\"{name}\"")
            );
            assert_eq!(
                serde_json::from_str::<CodecId>(&format!("\"{name}\"")).unwrap(),
                codec
            );
            assert!(!codec.requires_exact_fallback());
        }
    }
}
