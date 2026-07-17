/// Experimental lattice quantization module.
///
/// Provides Z1 (integer lattice) and A2-shaped (hexagonal lattice placeholder)
/// quantization. A2 operates on pairs using scalar rounding with pair-preserving
/// padding — this is NOT a full hexagonal nearest-lattice implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatticeKind {
    Z1,
    A2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LatticeQuantizationResult {
    pub kind: LatticeKind,
    pub codes: Vec<i16>,
    pub reconstructed: Vec<f32>,
    pub mse: f32,
}

fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn clamp_to_i16(v: f32) -> i16 {
    let rounded = v.round();
    if rounded >= i16::MAX as f32 {
        i16::MAX
    } else if rounded <= i16::MIN as f32 {
        i16::MIN
    } else {
        rounded as i16
    }
}

fn mse(original: &[f32], reconstructed: &[f32]) -> f32 {
    let n = original.len();
    if n == 0 {
        return 0.0;
    }
    let sum: f32 = original
        .iter()
        .zip(reconstructed.iter())
        .map(|(a, b)| (a - b).powi(2))
        .sum();
    sum / n as f32
}

/// Quantize via Z1 (integer lattice): round(value * scale), reconstruct = code / scale.
pub fn quantize_z1(values: &[f32], scale: f32) -> LatticeQuantizationResult {
    let scale = sanitize_scale(scale);
    let codes: Vec<i16> = values.iter().map(|&v| clamp_to_i16(v * scale)).collect();
    let reconstructed: Vec<f32> = codes.iter().map(|&c| c as f32 / scale).collect();
    let mse = mse(values, &reconstructed);
    LatticeQuantizationResult {
        kind: LatticeKind::Z1,
        codes,
        reconstructed,
        mse,
    }
}

/// Quantize via A2-shaped pair quantization (placeholder, not full hexagonal nearest-lattice).
///
/// Operates on consecutive pairs. If the input length is odd, the trailing element
/// is quantized independently via Z1 behavior and appended.
pub fn quantize_a2_pairs(values: &[f32], scale: f32) -> LatticeQuantizationResult {
    let scale = sanitize_scale(scale);
    let n = values.len();
    let mut codes = Vec::with_capacity(n);
    let mut reconstructed = Vec::with_capacity(n);

    let pairs = n / 2;
    for i in 0..pairs {
        let a = values[2 * i];
        let b = values[2 * i + 1];
        let ca = clamp_to_i16(a * scale);
        let cb = clamp_to_i16(b * scale);
        codes.push(ca);
        codes.push(cb);
        reconstructed.push(ca as f32 / scale);
        reconstructed.push(cb as f32 / scale);
    }

    // Odd trailing element: Z1 scalar fallback.
    if n % 2 == 1 {
        let v = values[n - 1];
        let c = clamp_to_i16(v * scale);
        codes.push(c);
        reconstructed.push(c as f32 / scale);
    }

    let mse = mse(values, &reconstructed);
    LatticeQuantizationResult {
        kind: LatticeKind::A2,
        codes,
        reconstructed,
        mse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z1_deterministic_and_same_length() {
        let values = vec![0.1, -0.5, 1.2, -3.7, 0.0];
        let r1 = quantize_z1(&values, 10.0);
        let r2 = quantize_z1(&values, 10.0);
        assert_eq!(r1.codes, r2.codes);
        assert_eq!(r1.codes.len(), values.len());
        assert_eq!(r1.reconstructed.len(), values.len());
    }

    #[test]
    fn z1_mse_finite() {
        let values: Vec<f32> = (0..64).map(|i| i as f32 * 0.1 - 3.2).collect();
        let r = quantize_z1(&values, 4.0);
        assert!(r.mse.is_finite());
    }

    #[test]
    fn a2_preserves_length_even() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        let r = quantize_a2_pairs(&values, 1.0);
        assert_eq!(r.codes.len(), 4);
        assert_eq!(r.reconstructed.len(), 4);
    }

    #[test]
    fn a2_preserves_length_odd() {
        let values = vec![1.0, 2.0, 3.0];
        let r = quantize_a2_pairs(&values, 1.0);
        assert_eq!(r.codes.len(), 3);
        assert_eq!(r.reconstructed.len(), 3);
    }

    #[test]
    fn nonpositive_scale_falls_back_to_1() {
        let values = vec![2.0, -2.0];
        let r0 = quantize_z1(&values, 0.0);
        let rn = quantize_z1(&values, -5.0);
        let rf = quantize_z1(&values, f32::NAN);
        let r1 = quantize_z1(&values, 1.0);
        assert_eq!(r0.codes, r1.codes);
        assert_eq!(rn.codes, r1.codes);
        assert_eq!(rf.codes, r1.codes);
    }

    #[test]
    fn large_values_clamp_to_i16_bounds() {
        let values = vec![1e10_f32, -1e10_f32];
        let r = quantize_z1(&values, 1.0);
        assert_eq!(r.codes[0], i16::MAX);
        assert_eq!(r.codes[1], i16::MIN);
    }
}
