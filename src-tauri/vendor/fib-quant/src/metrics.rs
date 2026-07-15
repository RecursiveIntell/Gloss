use crate::{FibQuantError, Result};

/// Mean squared error between equal-length vectors.
pub fn mse(left: &[f32], right: &[f32]) -> Result<f64> {
    if left.len() != right.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "metric length mismatch: {} vs {}",
            left.len(),
            right.len()
        )));
    }
    if left.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    let sum: f64 = left
        .iter()
        .zip(right)
        .map(|(a, b)| {
            let delta = f64::from(*a) - f64::from(*b);
            delta * delta
        })
        .sum();
    Ok(sum / left.len() as f64)
}

/// Cosine similarity between equal-length vectors.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f64> {
    if left.len() != right.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "metric length mismatch: {} vs {}",
            left.len(),
            right.len()
        )));
    }
    let mut dot = 0.0;
    let mut left_norm = 0.0;
    let mut right_norm = 0.0;
    for (a, b) in left.iter().zip(right) {
        let a = f64::from(*a);
        let b = f64::from(*b);
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm <= 0.0 || right_norm <= 0.0 {
        return Err(FibQuantError::ZeroNorm);
    }
    Ok(dot / (left_norm.sqrt() * right_norm.sqrt()))
}
