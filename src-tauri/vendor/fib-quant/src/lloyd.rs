use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

use crate::{
    profile::{EmptyCellPolicy, FibQuantProfileV1, SourceMode},
    rotation::StoredRotation,
    spherical_beta::{sample_reference_projection, sample_spherical_beta},
    FibQuantError, Result,
};

pub const LLOYD_REPORT_SCHEMA: &str = "lloyd_report_v1";
const DONOR_SPLIT_EPSILON: f64 = 1.0e-6;

/// Deterministic empty-cell repair event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LloydRepairEventV1 {
    /// Restart index where the repair occurred.
    pub restart: u32,
    /// Iteration index where the repair occurred.
    pub iteration: u32,
    /// Empty cell that was repaired.
    pub empty_cell: u32,
    /// Donor cell selected for splitting.
    pub donor_cell: u32,
    /// Donor assignment count before splitting.
    pub donor_count_before: u32,
    /// Donor total distortion before splitting.
    pub donor_distortion: f64,
    /// Norm of the farthest residual used as split direction.
    pub residual_norm: f64,
    /// Epsilon used to split the donor centroid.
    pub split_epsilon: f64,
}

/// Lloyd-Max refinement report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LloydReportV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Number of requested restarts.
    pub restarts: u32,
    /// Number of requested iterations per restart.
    pub iterations: u32,
    /// Number of training samples used.
    pub training_samples: u32,
    /// Initial codebook MSE on the training set.
    pub init_mse: f64,
    /// Best MSE found.
    pub best_mse: f64,
    /// Best restart index.
    pub best_restart: u32,
    /// Number of empty cells repaired.
    pub empty_cells_repaired: u32,
    /// Detailed deterministic empty-cell repair events.
    pub repair_events: Vec<LloydRepairEventV1>,
    /// Deterministic seed used for refinement.
    pub seed: u64,
}

impl LloydReportV1 {
    pub(crate) fn validate_against_profile(&self, profile: &FibQuantProfileV1) -> Result<()> {
        if self.schema_version != LLOYD_REPORT_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "lloyd report schema_version {}, expected {LLOYD_REPORT_SCHEMA}",
                self.schema_version
            )));
        }
        if self.restarts != profile.lloyd_restarts
            || self.iterations != profile.lloyd_iterations
            || self.training_samples != profile.training_samples
            || self.seed != profile.codebook_seed
        {
            return Err(FibQuantError::CorruptPayload(
                "lloyd report does not match profile settings".into(),
            ));
        }
        if !self.init_mse.is_finite() || !self.best_mse.is_finite() {
            return Err(FibQuantError::CorruptPayload(
                "lloyd report contains non-finite mse".into(),
            ));
        }
        if self.empty_cells_repaired as usize != self.repair_events.len() {
            return Err(FibQuantError::CorruptPayload(
                "lloyd repair event count mismatch".into(),
            ));
        }
        Ok(())
    }
}

pub(crate) struct RefinedCodebook {
    pub codewords: Vec<f32>,
    pub init_mse: f64,
    pub training_mse: f64,
    pub report: LloydReportV1,
}

/// Skip Lloyd-Max refinement entirely. The radial-angular initialization
/// becomes the final codebook. The report records `best_mse == init_mse`
/// and `best_restart == 0` with zero repair events, so downstream consumers
/// can detect the skip path from the receipt.
pub(crate) fn skip_refinement(
    profile: &FibQuantProfileV1,
    initial: &[f64],
) -> Result<RefinedCodebook> {
    profile.validate()?;
    let k = profile.block_dim as usize;
    let n = profile.codebook_size as usize;
    if initial.len() != n * k {
        return Err(FibQuantError::CorruptPayload(format!(
            "initial codebook has {}, expected {}",
            initial.len(),
            n * k
        )));
    }
    let samples = training_samples(profile)?;
    let init_mse = mse_for_codebook(initial, k, &samples)?;
    let report = LloydReportV1 {
        schema_version: LLOYD_REPORT_SCHEMA.into(),
        restarts: profile.lloyd_restarts,
        iterations: profile.lloyd_iterations,
        training_samples: samples.len() as u32,
        init_mse,
        best_mse: init_mse,
        best_restart: 0,
        empty_cells_repaired: 0,
        repair_events: Vec::new(),
        seed: profile.codebook_seed,
    };
    Ok(RefinedCodebook {
        codewords: initial.iter().map(|&v| v as f32).collect(),
        init_mse,
        training_mse: init_mse,
        report,
    })
}

struct RepairRecorder<'a> {
    events: &'a mut Vec<LloydRepairEventV1>,
    restart: u32,
    iteration: u32,
}

/// Run deterministic multi-restart Lloyd-Max refinement.
pub(crate) fn refine_codebook(
    profile: &FibQuantProfileV1,
    initial: &[f64],
) -> Result<RefinedCodebook> {
    profile.validate()?;
    let k = profile.block_dim as usize;
    let n = profile.codebook_size as usize;
    if initial.len() != n * k {
        return Err(FibQuantError::CorruptPayload(format!(
            "initial codebook has {}, expected {}",
            initial.len(),
            n * k
        )));
    }
    let samples = training_samples(profile)?;
    let init_mse = mse_for_codebook(initial, k, &samples)?;
    let restarts = profile.lloyd_restarts.max(1);
    let iterations = profile.lloyd_iterations;
    let mut best = initial.to_vec();
    let mut best_mse = init_mse;
    let mut best_restart = 0;
    let mut total_repairs = 0u32;
    let mut all_repair_events = Vec::new();

    for restart in 0..restarts {
        let mut codebook = rotated_initial(profile, initial, restart)?;
        let mut restart_repairs = 0u32;
        for iteration in 0..iterations {
            let assignments = assign_samples(&codebook, k, &samples);
            update_centroids(
                &mut codebook,
                k,
                &samples,
                &assignments,
                profile.empty_cell_policy.clone(),
                &mut restart_repairs,
                RepairRecorder {
                    events: &mut all_repair_events,
                    restart,
                    iteration,
                },
            )?;
        }
        let mse = mse_for_codebook(&codebook, k, &samples)?;
        total_repairs = total_repairs.saturating_add(restart_repairs);
        if mse < best_mse || restart == 0 && init_mse.is_infinite() {
            best_mse = mse;
            best = codebook;
            best_restart = restart;
        }
    }

    if best_mse > init_mse {
        best = initial.to_vec();
        best_mse = init_mse;
        best_restart = u32::MAX;
    }

    let report = LloydReportV1 {
        schema_version: LLOYD_REPORT_SCHEMA.into(),
        restarts,
        iterations,
        training_samples: samples.len() as u32,
        init_mse,
        best_mse,
        best_restart,
        empty_cells_repaired: total_repairs,
        repair_events: all_repair_events,
        seed: profile.codebook_seed,
    };
    Ok(RefinedCodebook {
        codewords: best.into_iter().map(|value| value as f32).collect(),
        init_mse,
        training_mse: best_mse,
        report,
    })
}

fn training_samples(profile: &FibQuantProfileV1) -> Result<Vec<Vec<f64>>> {
    let d = profile.ambient_dim as usize;
    let k = profile.block_dim as usize;
    let count = profile.training_samples.max(profile.codebook_size) as usize;
    let mut rng = ChaCha8Rng::seed_from_u64(profile.codebook_seed ^ 0x4651_5541_4e54);
    (0..count)
        .map(|_| match profile.source_mode {
            SourceMode::CanonicalSphericalBeta => sample_spherical_beta(d, k, &mut rng),
            SourceMode::ReferenceGaussianProjection => sample_reference_projection(d, k, &mut rng),
        })
        .collect()
}

fn rotated_initial(profile: &FibQuantProfileV1, initial: &[f64], restart: u32) -> Result<Vec<f64>> {
    let k = profile.block_dim as usize;
    if restart == 0 {
        return Ok(initial.to_vec());
    }
    let rotation = StoredRotation::new(
        k,
        profile
            .codebook_seed
            .wrapping_add(u64::from(restart) * 0x9e37_79b9),
    )?;
    let mut out = Vec::with_capacity(initial.len());
    for codeword in initial.chunks_exact(k) {
        out.extend(rotation.apply(codeword)?);
    }
    Ok(out)
}

fn assign_samples(codebook: &[f64], k: usize, samples: &[Vec<f64>]) -> Vec<usize> {
    samples
        .iter()
        .map(|sample| nearest_index(sample, codebook, k).0)
        .collect()
}

fn update_centroids(
    codebook: &mut [f64],
    k: usize,
    samples: &[Vec<f64>],
    assignments: &[usize],
    policy: EmptyCellPolicy,
    repairs: &mut u32,
    recorder: RepairRecorder<'_>,
) -> Result<()> {
    let n = codebook.len() / k;
    let mut sums = vec![0.0; codebook.len()];
    let mut counts = vec![0usize; n];
    let mut distortion = vec![0.0; n];
    let mut farthest_samples = vec![vec![0.0; k]; n];
    let mut farthest_distances = vec![-1.0; n];
    for (sample, &assignment) in samples.iter().zip(assignments) {
        counts[assignment] += 1;
        let mut sample_dist = 0.0;
        for dim in 0..k {
            sums[assignment * k + dim] += sample[dim];
            let delta = sample[dim] - codebook[assignment * k + dim];
            sample_dist += delta * delta;
        }
        distortion[assignment] += sample_dist;
        if sample_dist > farthest_distances[assignment] {
            farthest_distances[assignment] = sample_dist;
            farthest_samples[assignment].clone_from(sample);
        }
    }
    for idx in 0..n {
        if counts[idx] > 0 {
            for dim in 0..k {
                codebook[idx * k + dim] = sums[idx * k + dim] / counts[idx] as f64;
            }
        }
    }
    let empty: Vec<_> = counts
        .iter()
        .enumerate()
        .filter_map(|(idx, count)| (*count == 0).then_some(idx))
        .collect();
    if empty.is_empty() {
        return Ok(());
    }
    if policy == EmptyCellPolicy::FailClosed {
        return Err(FibQuantError::EmptyCellRepairFailed(format!(
            "{} empty cells",
            empty.len()
        )));
    }
    for empty_idx in empty {
        let donor = counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count > 1)
            .max_by(|(left, _), (right, _)| distortion[*left].total_cmp(&distortion[*right]))
            .map(|(idx, _)| idx)
            .ok_or_else(|| FibQuantError::EmptyCellRepairFailed("no donor cell".into()))?;
        let donor_count_before = counts[donor];
        let donor_distortion = distortion[donor];
        let mut residual = vec![0.0; k];
        let mut residual_norm_sq = 0.0;
        for dim in 0..k {
            residual[dim] = farthest_samples[donor][dim] - codebook[donor * k + dim];
            residual_norm_sq += residual[dim] * residual[dim];
        }
        let residual_norm = residual_norm_sq.sqrt();
        if !residual_norm.is_finite() || residual_norm <= f64::EPSILON {
            return Err(FibQuantError::EmptyCellRepairFailed(
                "donor residual has zero direction".into(),
            ));
        }
        for dim in 0..k {
            let direction = residual[dim] / residual_norm;
            let centroid = codebook[donor * k + dim];
            codebook[donor * k + dim] = centroid - DONOR_SPLIT_EPSILON * direction;
            codebook[empty_idx * k + dim] = centroid + DONOR_SPLIT_EPSILON * direction;
        }
        recorder.events.push(LloydRepairEventV1 {
            restart: recorder.restart,
            iteration: recorder.iteration,
            empty_cell: empty_idx as u32,
            donor_cell: donor as u32,
            donor_count_before: donor_count_before as u32,
            donor_distortion,
            residual_norm,
            split_epsilon: DONOR_SPLIT_EPSILON,
        });
        counts[donor] -= 1;
        counts[empty_idx] = 1;
        distortion[donor] = 0.0;
        distortion[empty_idx] = 0.0;
        *repairs = repairs.saturating_add(1);
    }
    Ok(())
}

pub(crate) fn nearest_index(sample: &[f64], codebook: &[f64], k: usize) -> (usize, f64) {
    let mut best_idx = 0usize;
    let mut best_dist = f64::INFINITY;
    for (idx, codeword) in codebook.chunks_exact(k).enumerate() {
        let dist: f64 = sample
            .iter()
            .zip(codeword)
            .map(|(left, right)| {
                let delta = left - right;
                delta * delta
            })
            .sum();
        if dist < best_dist {
            best_dist = dist;
            best_idx = idx;
        }
    }
    (best_idx, best_dist)
}

fn mse_for_codebook(codebook: &[f64], k: usize, samples: &[Vec<f64>]) -> Result<f64> {
    if samples.is_empty() {
        return Err(FibQuantError::NumericalFailure(
            "empty Lloyd training set".into(),
        ));
    }
    let sum: f64 = samples
        .iter()
        .map(|sample| nearest_index(sample, codebook, k).1)
        .sum();
    let mse = sum / samples.len() as f64;
    if mse.is_finite() {
        Ok(mse)
    } else {
        Err(FibQuantError::NumericalFailure(
            "non-finite Lloyd MSE".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cell_repair_splits_highest_distortion_donor() {
        let mut codebook = vec![0.0, 0.0, 10.0, 0.0, 20.0, 0.0];
        let samples = vec![
            vec![0.0, 0.0],
            vec![1.0, 0.0],
            vec![2.0, 0.0],
            vec![10.0, 0.0],
            vec![10.1, 0.0],
        ];
        let assignments = vec![0, 0, 0, 1, 1];
        let mut repairs = 0;
        let mut events = Vec::new();

        update_centroids(
            &mut codebook,
            2,
            &samples,
            &assignments,
            EmptyCellPolicy::SplitHighestDistortion,
            &mut repairs,
            RepairRecorder {
                events: &mut events,
                restart: 0,
                iteration: 0,
            },
        )
        .unwrap();

        assert_eq!(repairs, 1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].empty_cell, 2);
        assert_eq!(events[0].donor_cell, 0);
        assert_eq!(events[0].donor_count_before, 3);
        assert!(events[0].donor_distortion > 1.0);
        assert!(events[0].residual_norm > 0.0);
        assert!(codebook[0] < 1.0);
        assert!(codebook[4] > 1.0);
    }
}
