use statrs::distribution::{ContinuousCDF, Normal};

use crate::{profile::DirectionMethod, FibQuantError, Result};

const GOLDEN_RATIO: f64 = 1.618_033_988_749_895;

/// Planar Fibonacci spiral directions on `S^1`.
pub fn fibonacci_spiral_2d(n: usize) -> Result<Vec<[f64; 2]>> {
    if n == 0 {
        return Err(FibQuantError::InvalidCodebookSize(n));
    }
    let theta_g = 1.0 - 1.0 / GOLDEN_RATIO;
    Ok((0..n)
        .map(|idx| {
            let theta = 2.0 * std::f64::consts::PI * idx as f64 * theta_g;
            [theta.cos(), theta.sin()]
        })
        .collect())
}

/// Fibonacci sphere directions on `S^2`.
pub fn fibonacci_sphere_3d(n: usize) -> Result<Vec<[f64; 3]>> {
    if n == 0 {
        return Err(FibQuantError::InvalidCodebookSize(n));
    }
    let theta_g = 1.0 - 1.0 / GOLDEN_RATIO;
    Ok((0..n)
        .map(|idx| {
            let z = 1.0 - (2.0 * (idx + 1) as f64 - 1.0) / n as f64;
            let theta = 2.0 * std::f64::consts::PI * idx as f64 * theta_g;
            let r = (1.0 - z * z).max(0.0).sqrt();
            [r * theta.cos(), r * theta.sin(), z]
        })
        .collect())
}

/// Roberts-Kronecker sequence mapped through inverse normal and projected to `S^{k-1}`.
pub fn roberts_kronecker(k: usize, n: usize) -> Result<Vec<Vec<f64>>> {
    if k < 4 {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim: k + 1,
            block_dim: k,
        });
    }
    if n == 0 {
        return Err(FibQuantError::InvalidCodebookSize(n));
    }
    let phi = roberts_phi(k)?;
    let normal = Normal::new(0.0, 1.0)
        .map_err(|err| FibQuantError::NumericalFailure(format!("normal: {err}")))?;
    let mut dirs = Vec::with_capacity(n);
    for idx in 0..n {
        let mut values = Vec::with_capacity(k);
        let mut norm_sq = 0.0;
        for j in 1..=k {
            let step = phi.powi(-(j as i32));
            let frac = (((idx as f64 + 0.5) * step).fract()).clamp(1.0e-12, 1.0 - 1.0e-12);
            let value = normal.inverse_cdf(frac);
            norm_sq += value * value;
            values.push(value);
        }
        if norm_sq <= 0.0 || !norm_sq.is_finite() {
            return Err(FibQuantError::NumericalFailure(
                "Roberts-Kronecker projected zero/non-finite vector".into(),
            ));
        }
        let norm = norm_sq.sqrt();
        dirs.push(values.into_iter().map(|value| value / norm).collect());
    }
    Ok(dirs)
}

pub(crate) fn directions_for_method(
    k: usize,
    n: usize,
    method: &DirectionMethod,
) -> Result<Vec<Vec<f64>>> {
    match k {
        2 if method == &DirectionMethod::FibonacciSpiral => Ok(fibonacci_spiral_2d(n)?
            .into_iter()
            .map(|v| vec![v[0], v[1]])
            .collect()),
        3 if method == &DirectionMethod::FibonacciSphere => Ok(fibonacci_sphere_3d(n)?
            .into_iter()
            .map(|v| vec![v[0], v[1], v[2]])
            .collect()),
        _ if k >= 4 && method == &DirectionMethod::RobertsKronecker => roberts_kronecker(k, n),
        _ => Err(FibQuantError::CorruptPayload(format!(
            "direction method {method:?} is not supported for k={k}"
        ))),
    }
}

fn roberts_phi(k: usize) -> Result<f64> {
    let mut lo = 1.0f64;
    let mut hi = 2.0f64;
    for _ in 0..128 {
        let mid = (lo + hi) / 2.0;
        let value = mid.powi((k + 1) as i32) - mid - 1.0;
        if value > 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let phi = (lo + hi) / 2.0;
    if phi.is_finite() && phi > 1.0 {
        Ok(phi)
    } else {
        Err(FibQuantError::NumericalFailure(format!(
            "invalid Roberts phi for k={k}"
        )))
    }
}
