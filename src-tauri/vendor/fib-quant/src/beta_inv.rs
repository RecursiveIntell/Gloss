use statrs::distribution::{Beta, ContinuousCDF};

use crate::{FibQuantError, Result};

/// Inverse CDF for a Beta distribution, with fail-closed bounds checking.
pub fn beta_inv(q: f64, alpha: f64, beta: f64) -> Result<f64> {
    if !(0.0..=1.0).contains(&q)
        || !alpha.is_finite()
        || !beta.is_finite()
        || alpha <= 0.0
        || beta <= 0.0
    {
        return Err(FibQuantError::NumericalFailure(format!(
            "invalid beta inverse inputs q={q}, alpha={alpha}, beta={beta}"
        )));
    }
    let dist = Beta::new(alpha, beta)
        .map_err(|err| FibQuantError::NumericalFailure(format!("beta distribution: {err}")))?;
    Ok(dist.inverse_cdf(q).clamp(0.0, 1.0))
}
