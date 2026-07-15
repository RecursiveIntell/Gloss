use crate::codec::{CompressedBlock, ExactFallbackCodec, KVecCodec};
use crate::error::Result;

/// Exact (uncompressed) fallback storage for verification and debug.
///
/// Stores raw f32 KV vectors without any compression. Used to verify
/// that the compression/decompression paths are correct.
pub struct ExactFallbackStore {
    /// The codec (always ExactFallbackCodec).
    codec: ExactFallbackCodec,
}

impl ExactFallbackStore {
    /// Create a new exact fallback store for the given head dimension.
    pub fn new(head_dim: usize) -> Self {
        Self {
            codec: ExactFallbackCodec::new(head_dim),
        }
    }

    /// Encode a key/value vector into a CompressedBlock (no actual compression).
    pub fn encode_block(&self, vector: &[f32], _seed: u64) -> Result<CompressedBlock> {
        let encoded = self.codec.encode(vector, 0)?;
        Ok(CompressedBlock::new(
            self.codec.codec_id(),
            encoded,
            vector.len(),
        ))
    }

    /// Decode a CompressedBlock back to the original f32 vector.
    pub fn decode_block(&self, block: &CompressedBlock) -> Result<Vec<f32>> {
        self.codec.decode(&block.encoded_payload, 0)
    }

    /// Verify that encoding then decoding produces the original vector.
    pub fn verify_roundtrip(&self, original: &[f32], _seed: u64) -> Result<bool> {
        let decoded = self.decode_block(&self.encode_block(original, 0)?)?;
        Ok(original
            .iter()
            .zip(decoded.iter())
            .all(|(a, b)| (a - b).abs() < 1e-6))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_fallback_roundtrip() {
        let store = ExactFallbackStore::new(8);
        let vec: Vec<f32> = vec![0.5, -0.25, 0.75, 1.0, -1.0, 0.1, 0.9, -0.5];
        let block = store.encode_block(&vec, 0).unwrap();
        let decoded = store.decode_block(&block).unwrap();
        assert_eq!(vec, decoded);
    }

    #[test]
    fn test_exact_fallback_dimension_mismatch() {
        let store = ExactFallbackStore::new(8);
        let vec: Vec<f32> = vec![0.1, 0.2, 0.3]; // wrong size
        let result = store.encode_block(&vec, 0);
        assert!(result.is_err());
    }
}
