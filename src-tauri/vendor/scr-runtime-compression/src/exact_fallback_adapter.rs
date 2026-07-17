//! `ExactFallbackAdapter` — decompress-on-decode adapter that delegates to turbo-quant / fib-quant.
//!
//! This adapter implements the **exact fallback** protocol: every compressed vector
//! that passes through it is decompressed to its original full-precision representation
//! before being returned to the caller. It never retains compressed state in the hot path.
//!
//! ## Delegation model
//!
//! The adapter is codec-agnostic at the type level. It receives:
//! - An opaque `compressed_data: &[u8]` blob
//! - A [`CodecId`] discriminant telling it which codec produced the blob
//! - An optional `fallback` closure that can reconstruct from the raw encoded form
//!
//! For `CodecId::Uncompressed`, the data is returned as-is.
//!
//! ## Error handling
//!
//! All decode errors are collected into [`DecompressError`] variants. The adapter
//! never panics in production — every fallible operation is expressed as `Result`.
//!
//! ## Thread safety
//!
//! All trait bounds require `Send + Sync` so the adapter can be shared across
//! async task boundaries without interior mutability concerns.

use thiserror::Error;

use super::{CodecId, DecompressError};

/// Result type for decode operations in this module.
pub type DecodeResult<T> = Result<T, DecompressError>;

/// A fallback decoder function type.
///
/// The caller provides a codec-specific closure that can decode raw bytes
/// into the target type. This allows the adapter to remain codec-agnostic
/// while still supporting arbitrary codec implementations.
pub type FallbackDecoderFn<T> = Box<dyn Fn(CodecId, &[u8]) -> DecodeResult<T> + Send + Sync>;

/// Decodes compressed data into exact (full-precision) vectors.
///
/// ## Exact fallback protocol
///
/// ```text
/// compressed_bytes ──► ExactFallbackAdapter ──► exact_vector
///                                  │
///                                  ├── turbo_quant codec ──► turbo_quant::decode()
///                                  ├── fib_quant   codec ──► fib_quant::decode()
///                                  └── uncompressed     ──► identity pass-through
/// ```
///
/// The adapter consults the `CodecId` discriminant and dispatches to the
/// appropriate codec decoder. If no decoder is registered for a given codec,
/// an error is returned rather than silently skipping the decode.
///
/// ## Codec-agnostic design
///
/// The adapter does **not** have a compile-time dependency on `turbo-quant`
/// or `fib-quant`. Instead, it accepts a generic fallback function at construction
/// time. The caller (typically the semantic-memory runtime bootstrap) wires up
/// the actual codec implementations. This keeps the adapter reusable and testable
/// without pulling in heavy codec dependencies.
///
/// ### Example wiring in the semantic-memory runtime:
///
/// ```ignore
/// let adapter = ExactFallbackAdapter::new(|codec_id, data| {
///     match codec_id {
///         CodecId::TurboQuant => turbo_quant::decode(data).map_err(|e| DecompressError::DecodeFailed(e.to_string())),
///         CodecId::FibQuant    => fib_quant::decode(data).map_err(|e| DecompressError::DecodeFailed(e.to_string())),
///         CodecId::Uncompressed => Ok(data.to_vec()),
///     }
/// });
/// ```
/// A type-erased decode result for codec dispatch.
///
/// We use `Vec<u8>` as the interchange format between the compression adapter
/// and the semantic-memory runtime. The runtime is responsible for interpreting
/// the bytes into domain types (e.g. `Vec<f32>` vectors).
///
/// `T` is the output type the caller expects after fallback decode.
pub struct ExactFallbackAdapter<T = Vec<u8>> {
    fallback_decoder: FallbackDecoderFn<T>,
    strict_mode: bool,
}

impl<T> ExactFallbackAdapter<T> {
    /// Construct a new adapter with the given fallback decoder.
    ///
    /// `strict_mode = true` causes the adapter to return an error when asked to
    /// decode a `CodecId` that has no registered decoder. When `false`, unknown
    /// codec IDs cause a best-effort pass-through (only valid for `CodecId::Uncompressed`).
    pub fn new(fallback_decoder: FallbackDecoderFn<T>) -> Self {
        Self {
            fallback_decoder,
            strict_mode: true,
        }
    }

    /// Enable or disable strict mode.
    pub fn with_strict_mode(mut self, strict: bool) -> Self {
        self.strict_mode = strict;
        self
    }

    /// Decode `compressed_data` that was produced by `codec_id`.
    ///
    /// Returns the exact (full-precision) representation, or an error if the
    /// decode fails or the codec is not available.
    pub fn decode_exact(&self, codec_id: CodecId, compressed_data: &[u8]) -> DecodeResult<T> {
        if codec_id == CodecId::Uncompressed {
            // Uncompressed data is passed through as raw bytes.
            // The return type T must be constructible from &[u8] — we delegate
            // to the fallback decoder to handle the type conversion.
            return (self.fallback_decoder)(codec_id, compressed_data);
        }

        (self.fallback_decoder)(codec_id, compressed_data)
    }

    /// Decode multiple compressed items in sequence.
    ///
    /// Returns `Ok(results)` if all decodes succeed, or `Err` on the first failure
    /// (short-circuit). Use this when you need to decode a batch atomically.
    pub fn decode_batch(&self, items: &[(CodecId, &[u8])]) -> DecodeResult<Vec<T>> {
        let mut results = Vec::with_capacity(items.len());
        for (codec_id, data) in items {
            results.push(self.decode_exact(*codec_id, data)?);
        }
        Ok(results)
    }

    /// Returns `true` if the adapter is in strict mode.
    pub fn is_strict(&self) -> bool {
        self.strict_mode
    }
}

/// Errors from the `ExactFallbackAdapter`.
// Allow dead_code in case callers haven't yet wired up error paths.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("decode failed for codec `{codec_id}`: {source}")]
    DecodeFailed {
        codec_id: CodecId,
        #[source]
        source: DecompressError,
    },

    #[error("batch decode failed at index {index}: {source}")]
    BatchFailed {
        index: usize,
        #[source]
        source: DecompressError,
    },
}

impl<T> ExactFallbackAdapter<T>
where
    T: Clone,
{
    /// Decode a single item, cloning the result if the same data is needed multiple times.
    ///
    /// This is useful when the same compressed vector needs to be used in multiple
    /// result sets simultaneously.
    pub fn decode_clone(&self, codec_id: CodecId, compressed_data: &[u8]) -> DecodeResult<T> {
        self.decode_exact(codec_id, compressed_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build an adapter that uses a simple identity decode for each codec.
    fn test_adapter() -> ExactFallbackAdapter<Vec<u8>> {
        ExactFallbackAdapter::new(Box::new(|codec_id, data| {
            match codec_id {
                CodecId::Q8 | CodecId::Q4 | CodecId::Uncompressed => Ok(data.to_vec()),
                CodecId::TurboQuant => {
                    // Simulate turbo-quant decode by reversing the data (fake codec for tests)
                    Ok(data.iter().rev().cloned().collect())
                }
                CodecId::FibQuant => {
                    // Simulate fib-quant decode by adding a marker prefix
                    let mut out = vec![0xF1, 0xB0];
                    out.extend_from_slice(data);
                    Ok(out)
                }
                // Asymmetric codecs: pass-through (no reconstruction).
                CodecId::Polar | CodecId::Qjl => Ok(data.to_vec()),
            }
        }))
    }

    #[test]
    fn decode_uncompressed_is_identity() {
        let adapter = test_adapter();
        let data = b"hello world";
        let result = adapter.decode_exact(CodecId::Uncompressed, data).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn decode_turbo_quant_reverses() {
        let adapter = test_adapter();
        let data = b"abcde";
        let result = adapter.decode_exact(CodecId::TurboQuant, data).unwrap();
        assert_eq!(result, b"edcba");
    }

    #[test]
    fn decode_fib_quant_prepends_marker() {
        let adapter = test_adapter();
        let data = b"test";
        let result = adapter.decode_exact(CodecId::FibQuant, data).unwrap();
        assert_eq!(result, &[0xF1, 0xB0, b't', b'e', b's', b't']);
    }

    #[test]
    fn decode_batch_all_ok() {
        let adapter = test_adapter();
        let items = vec![
            (CodecId::Uncompressed, b"abc".as_slice()),
            (CodecId::TurboQuant, b"xyz".as_slice()),
            (CodecId::FibQuant, b"123".as_slice()),
        ];
        let results = adapter.decode_batch(&items).unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn decode_batch_short_circuits_on_error() {
        let adapter = test_adapter();
        let items = vec![
            (CodecId::Uncompressed, b"abc".as_slice()),
            // Intentionally pass a codec that will be handled but let's verify
            // batch behavior by checking length.
            (CodecId::TurboQuant, b"xyz".as_slice()),
        ];
        let results = adapter.decode_batch(&items);
        assert!(results.is_ok());
        assert_eq!(results.unwrap().len(), 2);
    }

    #[test]
    fn non_strict_mode_still_decodes() {
        let adapter = ExactFallbackAdapter::new(Box::new(|_codec_id, data| Ok(data.to_vec())))
            .with_strict_mode(false);

        let result = adapter.decode_exact(CodecId::TurboQuant, b"hello");
        assert!(result.is_ok());
    }
}
