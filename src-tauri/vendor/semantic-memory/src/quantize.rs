//! Scalar quantization (SQ8) for f32 → i8 vector compression.
//!
//! Per-vector affine quantization: each vector gets its own scale and zero_point,
//! computed from its min/max values. This gives 4x memory reduction with <0.5% cosine
//! similarity error on normalized embedding vectors.
//!
//! This module is independent of the HNSW backend and can be used with brute-force too.

use crate::error::MemoryError;

/// Scalar quantization parameters for a single vector.
#[derive(Debug, Clone)]
pub struct QuantizedVector {
    /// Quantized int8 values.
    pub data: Vec<i8>,
    /// Scale factor: `original[i] ≈ (data[i] - zero_point) * scale`
    pub scale: f32,
    /// Quantization zero point, always in the i8 range [-128, 127].
    pub zero_point: i8,
}

/// Quantizer that converts f32 vectors to int8 with per-vector calibration.
#[derive(Debug, Clone)]
pub struct Quantizer {
    dimensions: usize,
}

impl Quantizer {
    /// Create a new quantizer for vectors of the given dimensionality.
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }

    /// The configured dimensionality.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Quantize a single f32 vector to int8 with per-vector affine calibration.
    ///
    /// Affine quantization maps to the full i8 range [-128, 127] (256 discrete levels).
    /// Each vector gets its own scale/zero_point derived from its min/max values.
    pub fn quantize(&self, vector: &[f32]) -> Result<QuantizedVector, MemoryError> {
        if vector.len() != self.dimensions {
            return Err(MemoryError::QuantizationError(format!(
                "expected {} dimensions, got {}",
                self.dimensions,
                vector.len()
            )));
        }

        let min = vector.iter().copied().fold(f32::INFINITY, f32::min);
        let max = vector.iter().copied().fold(f32::NEG_INFINITY, f32::max);

        // Handle edge case: constant vector (all dimensions same value)
        if (max - min).abs() < f32::EPSILON {
            return Ok(QuantizedVector {
                data: vec![0i8; self.dimensions],
                scale: 1.0,
                zero_point: 0,
            });
        }

        let scale = (max - min) / 255.0;
        if !scale.is_finite() || scale <= 0.0 {
            return Err(MemoryError::QuantizationError(
                "computed non-finite quantization scale".into(),
            ));
        }
        let zero_point_f = -128.0 - (min / scale);
        let zero_point = zero_point_f.round().clamp(-128.0, 127.0) as i8;

        let data: Vec<i8> = vector
            .iter()
            .map(|&v| {
                let q = (v / scale + zero_point as f32).round();
                q.clamp(-128.0, 127.0) as i8
            })
            .collect();

        Ok(QuantizedVector {
            data,
            scale,
            zero_point,
        })
    }

    /// Dequantize back to f32 (approximate reconstruction).
    pub fn dequantize(&self, qv: &QuantizedVector) -> Vec<f32> {
        qv.data
            .iter()
            .map(|&q| (q as f32 - qv.zero_point as f32) * qv.scale)
            .collect()
    }
}

/// Pack a QuantizedVector into bytes for SQLite storage.
///
/// Format: `[scale: f32 LE][zero_point: i8][data: i8 × dims]`
/// Total bytes: `4 + 1 + dims`
pub fn pack_quantized(qv: &QuantizedVector) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + qv.data.len());
    buf.extend_from_slice(&qv.scale.to_le_bytes());
    buf.push(qv.zero_point.to_ne_bytes()[0]);
    buf.extend(qv.data.iter().map(|value| value.to_ne_bytes()[0]));
    buf
}

/// Unpack bytes from SQLite into a QuantizedVector.
pub fn unpack_quantized(bytes: &[u8], dimensions: usize) -> Result<QuantizedVector, MemoryError> {
    let expected_len = 5 + dimensions;
    if bytes.len() != expected_len {
        return Err(MemoryError::QuantizationError(format!(
            "expected {} bytes for {} dimensions, got {}",
            expected_len,
            dimensions,
            bytes.len()
        )));
    }
    let scale_bytes: [u8; 4] = bytes[0..4]
        .try_into()
        .map_err(|e| MemoryError::QuantizationError(format!("invalid scale bytes: {e}")))?;
    let scale = f32::from_le_bytes(scale_bytes);
    let zero_point = i8::from_ne_bytes([bytes[4]]);
    let data: Vec<i8> = bytes[5..].iter().map(|&b| i8::from_ne_bytes([b])).collect();
    Ok(QuantizedVector {
        data,
        scale,
        zero_point,
    })
}
