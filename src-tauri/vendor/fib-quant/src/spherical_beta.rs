use rand::Rng;
use rand_distr::{Distribution, Gamma, StandardNormal};

use crate::{beta_inv::beta_inv, FibQuantError, Result};

/// Bennett-Gersho radial companding Beta shape `beta_{d,k}`.
pub fn beta_d_k(d: usize, k: usize) -> Result<f64> {
    validate_dk(d, k)?;
    if k == d {
        return Ok(1.0);
    }
    let dk_term = (d as f64 - k as f64 - 2.0) / 2.0;
    let beta = (k as f64 / (k as f64 + 2.0)) * dk_term + 1.0;
    if beta.is_finite() && beta > 0.0 {
        Ok(beta)
    } else {
        Err(FibQuantError::NumericalFailure(format!(
            "invalid beta_{{d,k}} for d={d}, k={k}: {beta}"
        )))
    }
}

/// Radius quantile for codeword `n` in `1..=n_total`.
pub fn radius_quantile(d: usize, k: usize, n: usize, n_total: usize) -> Result<f64> {
    validate_dk(d, k)?;
    if n == 0 || n > n_total || n_total == 0 {
        return Err(FibQuantError::InvalidCodebookSize(n_total));
    }
    if k == d {
        return Ok(1.0);
    }
    let q = (n as f64 - 0.5) / n_total as f64;
    if k == 2 {
        return radius_quantile_k2_closed_form(d, q);
    }
    let alpha = k as f64 / 2.0;
    let beta = beta_d_k(d, k)?;
    Ok(beta_inv(q, alpha, beta)?.sqrt())
}

/// Paper closed-form radius path for `k = 2`.
pub fn radius_quantile_k2_closed_form(d: usize, q: f64) -> Result<f64> {
    if d <= 2 {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim: d,
            block_dim: 2,
        });
    }
    if !(0.0..=1.0).contains(&q) || !q.is_finite() {
        return Err(FibQuantError::NumericalFailure(format!(
            "invalid k=2 radius quantile q={q}"
        )));
    }
    Ok((1.0 - (1.0 - q).powf(4.0 / d as f64)).sqrt())
}

/// Sample the canonical spherical-Beta `k`-block source.
pub fn sample_spherical_beta(d: usize, k: usize, rng: &mut impl Rng) -> Result<Vec<f64>> {
    validate_dk(d, k)?;
    if k == d {
        return sample_unit_sphere(k, rng);
    }
    let r2 = sample_beta(k as f64 / 2.0, (d - k) as f64 / 2.0, rng)?;
    let direction = sample_unit_sphere(k, rng)?;
    let r = r2.sqrt();
    Ok(direction.into_iter().map(|value| r * value).collect())
}

/// Sample by normalizing a Gaussian in `R^d` and projecting the first `k` coordinates.
pub fn sample_reference_projection(d: usize, k: usize, rng: &mut impl Rng) -> Result<Vec<f64>> {
    validate_dk(d, k)?;
    let mut values = Vec::with_capacity(d);
    let mut norm_sq = 0.0;
    for _ in 0..d {
        let value: f64 = StandardNormal.sample(rng);
        norm_sq += value * value;
        values.push(value);
    }
    if norm_sq == 0.0 || !norm_sq.is_finite() {
        return Err(FibQuantError::NumericalFailure(
            "zero/non-finite gaussian norm".into(),
        ));
    }
    let norm = norm_sq.sqrt();
    Ok(values
        .into_iter()
        .take(k)
        .map(|value| value / norm)
        .collect())
}

pub(crate) fn sample_unit_sphere(k: usize, rng: &mut impl Rng) -> Result<Vec<f64>> {
    if k == 0 {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim: 0,
            block_dim: 0,
        });
    }
    loop {
        let mut values = Vec::with_capacity(k);
        let mut norm_sq = 0.0;
        for _ in 0..k {
            let value: f64 = StandardNormal.sample(rng);
            norm_sq += value * value;
            values.push(value);
        }
        if norm_sq > 0.0 && norm_sq.is_finite() {
            let norm = norm_sq.sqrt();
            return Ok(values.into_iter().map(|value| value / norm).collect());
        }
    }
}

fn sample_beta(alpha: f64, beta: f64, rng: &mut impl Rng) -> Result<f64> {
    let ga = Gamma::new(alpha, 1.0)
        .map_err(|err| FibQuantError::NumericalFailure(format!("gamma alpha: {err}")))?;
    let gb = Gamma::new(beta, 1.0)
        .map_err(|err| FibQuantError::NumericalFailure(format!("gamma beta: {err}")))?;
    let a: f64 = ga.sample(rng);
    let b: f64 = gb.sample(rng);
    let sum = a + b;
    if sum <= 0.0 || !sum.is_finite() {
        return Err(FibQuantError::NumericalFailure("beta sample sum".into()));
    }
    Ok(a / sum)
}

fn validate_dk(d: usize, k: usize) -> Result<()> {
    if d == 0 {
        return Err(FibQuantError::ZeroDimension);
    }
    if k == 0 || k > d {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim: d,
            block_dim: k,
        });
    }
    Ok(())
}
