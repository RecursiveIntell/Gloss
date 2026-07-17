//! Archived Rust implementation of per_dim score_prepared, replaced by C kernel (c-kernels/scoring.c).
//! Kept for reference and verification. The C kernel produces identical output.
//!
//! Original location: src/per_dim_impl.rs
//! Archived: 2026-07-12

#![allow(dead_code)]

    fn score_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        if compressed.codes.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: compressed.codes.len(),
            });
        }
        if prepared.dim != self.dim || prepared.levels != self.levels as usize {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: prepared.dim,
            });
        }
        let expected_lut_len = self.dim * prepared.levels;
        if prepared.contribution_lut.len() != expected_lut_len {
            return Err(ScorerError::CorruptPayload(
                "per-dim prepared query lookup table has invalid length".into(),
            ));
        }
        let mut sum = 0.0f64;
        for i in 0..self.dim {
            let code = compressed.codes[i] as usize;
            sum += f64::from(prepared.contribution_lut[i * prepared.levels + code]);
        }
        let score = sum as f32;
        Ok(score)
    }