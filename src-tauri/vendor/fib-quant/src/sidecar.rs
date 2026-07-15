//! Generic sidecar search index wrapping [`FibScorer`].
//!
//! `FibSidecarIndex<Id>` stores caller-owned IDs alongside encoded
//! [`FibCodeV1`] artifacts and provides approximate nearest-inner-product
//! search via the Gram-table estimator in [`FibScorer`]. The index is a
//! *sidecar*: it produces approximate candidates that callers must rerank
//! with an exact inner-product computation against the original (un-encoded)
//! vectors before acting on the result.
//!
//! ## Model
//!
//! 1. The caller constructs a [`FibScorer`] once (which builds the codebook
//!    Gram table).
//! 2. The caller encodes vectors with [`FibQuantizer::encode`] and adds them
//!    to the sidecar index via [`FibSidecarIndex::add`] / `add_batch`.
//! 3. At query time, the caller passes a raw query slice. The sidecar calls
//!    [`FibScorer::inner_product_estimate`] for every stored code, sorts the
//!    results in descending order of approximate score, and returns the top
//!    `top_k * oversample` candidates.
//! 4. [`FibSidecarIndex::search_with_receipt`] additionally returns a
//!    [`SearchReceiptV1`] documenting the approximate nature of the results.

use std::time::Instant;

use crate::{codec::FibCodeV1, profile::FibQuantProfileV1, scoring::FibScorer, Result};

/// A scored candidate from approximate sidecar search.
///
/// `id` is the caller-owned identifier passed to [`FibSidecarIndex::add`].
/// `approximate_score` is the Gram-table inner-product estimate — **not**
/// the exact inner product. `rank` is zero-indexed; rank 0 is the
/// highest-scoring candidate.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredCandidate<Id> {
    /// Caller-owned identifier.
    pub id: Id,
    /// Approximate inner product estimate from the Gram table.
    pub approximate_score: f32,
    /// Zero-indexed rank (0 = best).
    pub rank: usize,
}

/// Receipt documenting an approximate sidecar search.
///
/// This is deliberately lightweight compared to the turbo-quant analogue:
/// the fib-quant sidecar does not own the original vectors or codebook
/// digests beyond the profile, so the receipt captures only the operational
/// parameters and timing of the search.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchReceiptV1 {
    /// Schema marker, always `"fib_sidecar_search_receipt_v1"`.
    pub schema: String,
    /// Number of encoded entries currently in the index.
    pub indexed_count: usize,
    /// Requested `top_k`.
    pub top_k: usize,
    /// Requested `oversample` factor.
    pub oversample: usize,
    /// Number of candidates actually returned (≤ `top_k * oversample`).
    pub candidate_count: usize,
    /// Always `true`: this is an approximate index.
    pub approximate_only: bool,
    /// Always `true`: callers must rerank with exact inner product.
    pub exact_rerank_required: bool,
    /// Elapsed time of the search in microseconds.
    pub elapsed_micros: u128,
}

/// Receipt documenting an IVF-accelerated approximate sidecar search.
///
/// Extends [`SearchReceiptV1`] with IVF-specific metadata: number of
/// centroids, cells probed, and entries scored.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchReceiptIvfV1 {
    /// Schema marker, always `"fib_sidecar_search_ivf_receipt_v1"`.
    pub schema: String,
    /// Number of encoded entries currently in the index.
    pub indexed_count: usize,
    /// Requested `top_k`.
    pub top_k: usize,
    /// Requested `oversample` factor.
    pub oversample: usize,
    /// Number of candidates actually returned (≤ `top_k * oversample`).
    pub candidate_count: usize,
    /// Always `true`: this is an approximate index.
    pub approximate_only: bool,
    /// Always `true`: callers must rerank with exact inner product.
    pub exact_rerank_required: bool,
    /// Elapsed time of the search in microseconds.
    pub elapsed_micros: u128,
    /// Number of centroids in the IVF coarse quantizer.
    pub num_centroids: usize,
    /// Number of cells (centroids) probed at query time.
    pub nprobe: usize,
    /// Number of entries that fell into the probed cells (actually scored).
    pub entries_scored: usize,
    /// Whether IVF was used (false if fallback to linear scan).
    pub ivf_used: bool,
}

/// IVF (Inverted File) coarse quantizer for sub-linear search.
///
/// Stores k centroids and a mapping from each entry index to its
/// nearest centroid. At query time, only the `nprobe` nearest cells
/// to the query are scanned, reducing search from O(N) to O(N/k * nprobe).
/// With k = sqrt(N) and nprobe constant, this is O(sqrt(N)).
#[derive(Debug, Clone)]
pub struct IvfCoarseQuantizer {
    /// Centroid vectors (k × ambient_dim).
    centroids: Vec<Vec<f32>>,
    /// Which centroid each entry belongs to (length = number of entries).
    assignments: Vec<usize>,
    /// Default number of cells to probe at query time.
    nprobe: usize,
}

impl IvfCoarseQuantizer {
    /// Whether the IVF structure has been built (has centroids).
    pub fn is_built(&self) -> bool {
        !self.centroids.is_empty()
    }

    /// Number of centroids.
    pub fn num_centroids(&self) -> usize {
        self.centroids.len()
    }

    /// Default nprobe value.
    pub fn nprobe(&self) -> usize {
        self.nprobe
    }

    /// Set the default nprobe value.
    pub fn set_nprobe(&mut self, nprobe: usize) {
        self.nprobe = nprobe;
    }

    /// Return the assignments slice.
    pub fn assignments(&self) -> &[usize] {
        &self.assignments
    }

    /// Return the centroids slice.
    pub fn centroids(&self) -> &[Vec<f32>] {
        &self.centroids
    }

    /// Build inverted lists: for each centroid, the list of entry indices
    /// assigned to it.
    pub fn inverted_lists(&self) -> Vec<Vec<usize>> {
        let k = self.centroids.len();
        let mut lists: Vec<Vec<usize>> = vec![Vec::new(); k];
        for (entry_idx, &centroid_idx) in self.assignments.iter().enumerate() {
            if centroid_idx < k {
                lists[centroid_idx].push(entry_idx);
            }
        }
        lists
    }
}

/// Generic ID-typed sidecar search index.
///
/// Wraps a [`FibScorer`] and stores a list of `(Id, FibCodeV1)` entries.
/// Search uses the Gram-table estimator for each stored code and returns
/// candidates sorted by descending approximate inner product.
///
/// The index is cheap to clone (via `Clone` on the scorer and entries) but
/// in typical usage a single instance is shared by reference across queries.
///
/// ## Generics
///
/// `Id` must be `Clone + Eq + Debug`. Common choices are `u64`, `String`,
/// or a newtype key. The index does not enforce uniqueness — adding the
/// same ID twice will produce two entries.
pub struct FibSidecarIndex<Id>
where
    Id: Clone + Eq + std::fmt::Debug,
{
    scorer: FibScorer,
    entries: Vec<(Id, FibCodeV1)>,
    profile: FibQuantProfileV1,
    /// Optional IVF coarse quantizer for sub-linear search.
    /// Built by [`build_ivf`](Self::build_ivf). When present and populated,
    /// [`search_ivf`](Self::search_ivf) probes only `nprobe` cells.
    ivf: Option<IvfCoarseQuantizer>,
}

impl<Id> FibSidecarIndex<Id>
where
    Id: Clone + Eq + std::fmt::Debug,
{
    /// Construct a new sidecar index from a [`FibScorer`].
    ///
    /// The scorer's profile is cloned for validation of incoming codes.
    /// The index starts empty.
    pub fn new(scorer: FibScorer) -> Self {
        let profile = scorer.quantizer().profile().clone();
        Self {
            scorer,
            entries: Vec::new(),
            profile,
            ivf: None,
        }
    }

    /// Return a reference to the underlying scorer.
    pub fn scorer(&self) -> &FibScorer {
        &self.scorer
    }

    /// Return a reference to the profile used for validation.
    pub fn profile(&self) -> &FibQuantProfileV1 {
        &self.profile
    }

    /// Add a single encoded vector with a caller-owned ID.
    ///
    /// The code's `ambient_dim` and `block_dim` are checked against the
    /// scorer's profile to catch mismatches early.
    pub fn add(&mut self, id: Id, code: FibCodeV1) {
        debug_assert!(
            code.ambient_dim == self.profile.ambient_dim,
            "code ambient_dim {} != profile {}",
            code.ambient_dim,
            self.profile.ambient_dim
        );
        debug_assert!(
            code.block_dim == self.profile.block_dim,
            "code block_dim {} != profile {}",
            code.block_dim,
            self.profile.block_dim
        );
        self.entries.push((id, code));
    }

    /// Add many encoded vectors at once.
    ///
    /// Equivalent to calling [`add`](Self::add) in a loop but avoids
    /// repeated capacity growth.
    pub fn add_batch(&mut self, entries: Vec<(Id, FibCodeV1)>) {
        self.entries.reserve(entries.len());
        for (id, code) in entries {
            self.add(id, code);
        }
    }

    /// Number of encoded entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return a slice of all `(Id, FibCodeV1)` entries.
    ///
    /// Used by [`persistence`](crate::persistence) to serialize the index
    /// to disk. The slice borrows from the index.
    pub fn entries(&self) -> &[(Id, FibCodeV1)] {
        &self.entries
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // ------------------------------------------------------------------
    // Internal search core
    // ------------------------------------------------------------------

    /// Run the approximate scoring loop and return sorted candidates
    /// (without truncation). The returned vector has `self.entries.len()`
    /// elements sorted by descending `approximate_score`, ties broken by
    /// insertion order (stable sort).
    fn score_all(&self, query: &[f32]) -> Result<Vec<(usize, f32)>> {
        // Use prepared query: rotate+quantize the query ONCE, then
        // do O(1) Gram table lookups per stored code. This is ~380x
        // faster than calling inner_product_estimate per code (which
        // re-rotates the query each time).
        let prepared = self.scorer.prepare_query(query)?;
        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(self.entries.len());
        for (idx, (_, code)) in self.entries.iter().enumerate() {
            let s = self.scorer.score_prepared(&prepared, code)?;
            scored.push((idx, s));
        }
        // Sort descending by score. Stable to preserve insertion order
        // for equal scores.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored)
    }

    /// Approximate search returning sorted candidates.
    ///
    /// Scores every stored code against the query, sorts by descending
    /// approximate inner product, and returns the top `top_k * oversample`
    /// candidates (or fewer if the index has fewer entries).
    ///
    /// The returned candidates have `rank` assigned starting from 0.
    /// Callers **must** rerank these candidates with an exact inner product
    /// against the original (un-encoded) vectors before acting on results.
    pub fn search(
        &self,
        query: &[f32],
        top_k: usize,
        oversample: usize,
    ) -> Result<Vec<ScoredCandidate<Id>>> {
        let scored = self.score_all(query)?;
        let limit = top_k.saturating_mul(oversample.max(1)).min(scored.len());
        let candidates = scored
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(rank, (idx, score))| {
                let id = self.entries[idx].0.clone();
                ScoredCandidate {
                    id,
                    approximate_score: score,
                    rank,
                }
            })
            .collect();
        Ok(candidates)
    }

    /// Approximate search returning both candidates and a [`SearchReceiptV1`].
    ///
    /// See [`search`](Self::search) for search semantics. The receipt
    /// documents the operational parameters and timing of the search.
    pub fn search_with_receipt(
        &self,
        query: &[f32],
        top_k: usize,
        oversample: usize,
    ) -> Result<(Vec<ScoredCandidate<Id>>, SearchReceiptV1)> {
        let started = Instant::now();
        let candidates = self.search(query, top_k, oversample)?;
        let elapsed = started.elapsed().as_micros();

        let receipt = SearchReceiptV1 {
            schema: "fib_sidecar_search_receipt_v1".to_string(),
            indexed_count: self.entries.len(),
            top_k,
            oversample,
            candidate_count: candidates.len(),
            approximate_only: true,
            exact_rerank_required: true,
            elapsed_micros: elapsed,
        };

        Ok((candidates, receipt))
    }

    // ------------------------------------------------------------------
    // IVF coarse quantizer
    // ------------------------------------------------------------------

    /// Return a reference to the IVF coarse quantizer, if built.
    pub fn ivf(&self) -> Option<&IvfCoarseQuantizer> {
        self.ivf.as_ref()
    }

    /// Whether the IVF coarse quantizer has been built.
    pub fn ivf_is_built(&self) -> bool {
        self.ivf.as_ref().is_some_and(|ivf| ivf.is_built())
    }

    /// Build the IVF coarse quantizer by running k-means on decoded vectors.
    ///
    /// Decodes all stored `FibCodeV1` entries to their approximate vectors
    /// using the scorer's quantizer, runs k-means with `num_centroids`
    /// centroids (default: sqrt(N)), and stores the centroids and
    /// assignments. After this call, [`search_ivf`](Self::search_ivf)
    /// will use the IVF path instead of falling back to linear scan.
    ///
    /// # Arguments
    /// * `num_centroids` — number of k-means centroids (k). Pass
    ///   `(self.len() as f64).sqrt() as usize` or let `build_ivf_default`
    ///   compute it.
    pub fn build_ivf(&mut self, num_centroids: usize) -> Result<()> {
        let n = self.entries.len();
        if n == 0 {
            // Nothing to cluster — store an empty IVF so ivf_is_built() is true.
            self.ivf = Some(IvfCoarseQuantizer {
                centroids: Vec::new(),
                assignments: Vec::new(),
                nprobe: 8,
            });
            return Ok(());
        }

        let k = num_centroids.max(1).min(n);
        let d = self.profile.ambient_dim as usize;

        // Decode all stored codes to approximate vectors for k-means.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(n);
        for (_, code) in &self.entries {
            let v = self.scorer.quantizer().decode(code)?;
            vectors.push(v);
        }

        // K-means initialization: pick k evenly spaced entries as initial
        // centroids (deterministic, no RNG dependency).
        let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(k);
        if k == 1 {
            // Single centroid = mean of all vectors.
            let mut mean = vec![0.0f32; d];
            for v in &vectors {
                for (mi, &vi) in mean.iter_mut().zip(v.iter()) {
                    *mi += vi;
                }
            }
            for m in &mut mean {
                *m /= n as f32;
            }
            centroids.push(mean);
        } else {
            for i in 0..k {
                let idx = (i * n) / k;
                centroids.push(vectors[idx].clone());
            }

            // K-means iterations (Lloyd's algorithm).
            const MAX_ITERS: usize = 20;
            const CONVERGE_THRESHOLD: f32 = 1e-4;

            let mut assignments = vec![0usize; n];
            let mut sums: Vec<Vec<f64>> = vec![vec![0.0f64; d]; k];
            let mut counts: Vec<usize> = vec![0; k];

            for _iter in 0..MAX_ITERS {
                // Assignment step: assign each vector to nearest centroid.
                for (vi, v) in vectors.iter().enumerate() {
                    let mut best_dist = f32::MAX;
                    let mut best_c = 0;
                    for (ci, c) in centroids.iter().enumerate() {
                        // Squared L2 distance
                        let mut dist = 0.0f32;
                        for j in 0..d {
                            let diff = v[j] - c[j];
                            dist += diff * diff;
                        }
                        if dist < best_dist {
                            best_dist = dist;
                            best_c = ci;
                        }
                    }
                    assignments[vi] = best_c;
                }

                // Update step: recompute centroids as means.
                for s in sums.iter_mut() {
                    for x in s.iter_mut() {
                        *x = 0.0;
                    }
                }
                counts.fill(0);

                for (vi, v) in vectors.iter().enumerate() {
                    let c = assignments[vi];
                    counts[c] += 1;
                    for j in 0..d {
                        sums[c][j] += v[j] as f64;
                    }
                }

                let mut total_shift = 0.0f32;
                for ci in 0..k {
                    if counts[ci] == 0 {
                        // Empty cluster: reinitialize to a random-ish vector
                        // (pick the furthest entry from its centroid).
                        let mut furthest = 0;
                        let mut max_dist = 0.0f32;
                        for (vi, v) in vectors.iter().enumerate() {
                            let c = assignments[vi];
                            let mut dist = 0.0f32;
                            for j in 0..d {
                                let diff = v[j] - centroids[c][j];
                                dist += diff * diff;
                            }
                            if dist > max_dist {
                                max_dist = dist;
                                furthest = vi;
                            }
                        }
                        centroids[ci] = vectors[furthest].clone();
                        assignments[furthest] = ci;
                        continue;
                    }
                    let new_centroid: Vec<f32> = (0..d)
                        .map(|j| (sums[ci][j] / counts[ci] as f64) as f32)
                        .collect();
                    // Compute shift
                    for j in 0..d {
                        let diff = new_centroid[j] - centroids[ci][j];
                        total_shift += diff * diff;
                    }
                    centroids[ci] = new_centroid;
                }

                if total_shift < CONVERGE_THRESHOLD {
                    break;
                }
            }
        }

        // Final assignment pass with the converged centroids.
        let mut assignments = vec![0usize; n];
        for (vi, v) in vectors.iter().enumerate() {
            let mut best_dist = f32::MAX;
            let mut best_c = 0;
            for (ci, c) in centroids.iter().enumerate() {
                let mut dist = 0.0f32;
                for j in 0..d {
                    let diff = v[j] - c[j];
                    dist += diff * diff;
                }
                if dist < best_dist {
                    best_dist = dist;
                    best_c = ci;
                }
            }
            assignments[vi] = best_c;
        }

        let nprobe = 8usize.min(k);
        self.ivf = Some(IvfCoarseQuantizer {
            centroids,
            assignments,
            nprobe,
        });

        Ok(())
    }

    /// Build IVF with default k = sqrt(N) centroids.
    pub fn build_ivf_default(&mut self) -> Result<()> {
        let k = (self.len() as f64).sqrt().round() as usize;
        let k = k.max(1);
        self.build_ivf(k)
    }

    /// Find the `nprobe` nearest centroids to the query (exact, small k).
    /// Returns sorted (centroid_index, squared_distance) pairs.
    fn nearest_centroids(&self, query: &[f32], nprobe: usize) -> Vec<(usize, f32)> {
        let ivf = self.ivf.as_ref().expect("IVF must be built");
        let d = self.profile.ambient_dim as usize;
        let mut dists: Vec<(usize, f32)> = Vec::with_capacity(ivf.centroids.len());
        for (ci, c) in ivf.centroids.iter().enumerate() {
            let mut dist = 0.0f32;
            for j in 0..d.min(query.len()).min(c.len()) {
                let diff = query[j] - c[j];
                dist += diff * diff;
            }
            dists.push((ci, dist));
        }
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        dists.into_iter().take(nprobe).collect()
    }

    /// IVF-accelerated search.
    ///
    /// Finds the `nprobe` nearest centroids to the query, gathers all
    /// entries assigned to those centroids, scores them using the prepared
    /// query (Gram table), and returns the top `top_k * oversample`
    /// candidates sorted by descending approximate inner product.
    ///
    /// If IVF is not built (centroids empty or `ivf` is `None`), falls
    /// back to a full linear scan via [`search`](Self::search).
    pub fn search_ivf(
        &self,
        query: &[f32],
        top_k: usize,
        oversample: usize,
        nprobe: usize,
    ) -> Result<Vec<ScoredCandidate<Id>>> {
        // Fallback to linear scan if IVF not built.
        if !self.ivf_is_built() {
            return self.search(query, top_k, oversample);
        }

        let ivf = self.ivf.as_ref().unwrap();
        let nprobe_eff = nprobe.min(ivf.centroids.len()).max(1);

        // Find nearest centroids.
        let nearest = self.nearest_centroids(query, nprobe_eff);

        // Gather candidate entry indices from probed cells.
        let inverted = ivf.inverted_lists();
        let mut candidate_idxs: Vec<usize> = Vec::new();
        for (ci, _) in &nearest {
            candidate_idxs.extend_from_slice(&inverted[*ci]);
        }

        if candidate_idxs.is_empty() {
            return Ok(Vec::new());
        }

        // Score candidates using prepared query.
        let prepared = self.scorer.prepare_query(query)?;
        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(candidate_idxs.len());
        for &idx in &candidate_idxs {
            let code = &self.entries[idx].1;
            let s = self.scorer.score_prepared(&prepared, code)?;
            scored.push((idx, s));
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let limit = top_k.saturating_mul(oversample.max(1)).min(scored.len());
        let candidates = scored
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(rank, (idx, score))| {
                let id = self.entries[idx].0.clone();
                ScoredCandidate {
                    id,
                    approximate_score: score,
                    rank,
                }
            })
            .collect();
        Ok(candidates)
    }

    /// IVF-accelerated search with receipt.
    ///
    /// Like [`search_ivf`](Self::search_ivf) but returns a
    /// [`SearchReceiptIvfV1`] documenting the IVF parameters used.
    pub fn search_with_receipt_ivf(
        &self,
        query: &[f32],
        top_k: usize,
        oversample: usize,
        nprobe: usize,
    ) -> Result<(Vec<ScoredCandidate<Id>>, SearchReceiptIvfV1)> {
        let started = Instant::now();
        let ivf_used = self.ivf_is_built();

        let (candidates, entries_scored, nprobe_actual, num_centroids) = if ivf_used {
            let ivf = self.ivf.as_ref().unwrap();
            let nprobe_eff = nprobe.min(ivf.centroids.len()).max(1);
            // Count entries we'll score.
            let nearest = self.nearest_centroids(query, nprobe_eff);
            let inverted = ivf.inverted_lists();
            let count: usize = nearest.iter().map(|(ci, _)| inverted[*ci].len()).sum();
            let cands = self.search_ivf(query, top_k, oversample, nprobe)?;
            (cands, count, nprobe_eff, ivf.centroids.len())
        } else {
            // Linear scan fallback.
            let cands = self.search(query, top_k, oversample)?;
            let es = self.entries.len();
            (cands, es, 0, 0)
        };

        let elapsed = started.elapsed().as_micros();

        let receipt = SearchReceiptIvfV1 {
            schema: "fib_sidecar_search_ivf_receipt_v1".to_string(),
            indexed_count: self.entries.len(),
            top_k,
            oversample,
            candidate_count: candidates.len(),
            approximate_only: true,
            exact_rerank_required: true,
            elapsed_micros: elapsed,
            num_centroids,
            nprobe: nprobe_actual,
            entries_scored,
            ivf_used,
        };

        Ok((candidates, receipt))
    }
}

// ======================================================================
// Tests
// ======================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FibQuantProfileV1;
    use crate::{FibQuantizer, FibScorer};

    fn build_test_scorer() -> Result<FibScorer> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        let quantizer = FibQuantizer::new(profile)?;
        FibScorer::new(quantizer)
    }

    fn make_vectors(d: usize, count: usize) -> Vec<Vec<f32>> {
        (0..count)
            .map(|seed| {
                (0..d)
                    .map(|i| (seed as f32 * 0.1 + i as f32 * 0.05 - 0.3).sin())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn add_and_search_returns_correct_top_k() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 16);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        assert_eq!(query.len(), d);
        let results = index.search(&query, 5, 1)?;

        assert_eq!(results.len(), 5, "should return exactly top_k=5");
        // ranks 0..5
        for (i, r) in results.iter().enumerate() {
            assert_eq!(r.rank, i, "rank should be sequential from 0");
        }
        Ok(())
    }

    #[test]
    fn empty_index_search_returns_empty() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let query = vec![0.0f32; d];
        let results = index.search(&query, 5, 2)?;
        assert!(
            results.is_empty(),
            "empty index should return empty results"
        );

        let (results, receipt) = index.search_with_receipt(&query, 5, 2)?;
        assert!(results.is_empty());
        assert_eq!(receipt.indexed_count, 0);
        assert_eq!(receipt.candidate_count, 0);
        Ok(())
    }

    #[test]
    fn oversample_returns_more_than_top_k() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 20);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = index.search(&query, 5, 3)?;
        // top_k=5, oversample=3, entries=20 → min(15, 20) = 15
        assert_eq!(results.len(), 15, "oversample=3 should give 15 candidates");
        assert!(results.len() > 5, "should return more than top_k alone");
        Ok(())
    }

    #[test]
    fn results_sorted_descending() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 16);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = index.search(&query, 8, 1)?;
        for w in results.windows(2) {
            assert!(
                w[0].approximate_score >= w[1].approximate_score,
                "results not sorted descending: {} before {}",
                w[0].approximate_score,
                w[1].approximate_score
            );
        }
        Ok(())
    }

    #[test]
    fn receipt_fields_correct() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 12);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let (results, receipt) = index.search_with_receipt(&query, 5, 2)?;

        assert_eq!(receipt.schema, "fib_sidecar_search_receipt_v1");
        assert_eq!(receipt.indexed_count, 12);
        assert_eq!(receipt.top_k, 5);
        assert_eq!(receipt.oversample, 2);
        assert_eq!(receipt.candidate_count, results.len());
        assert!(receipt.approximate_only);
        assert!(receipt.exact_rerank_required);
        // elapsed_micros should be non-negative; can't assert upper bound
        // reliably but it should be a valid u128.
        let _ = receipt.elapsed_micros;
        Ok(())
    }

    #[test]
    fn add_batch_works() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 10);
        let entries: Vec<(u32, FibCodeV1)> = vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let code = index.scorer().quantizer().encode(v).unwrap();
                (i as u32, code)
            })
            .collect();

        index.add_batch(entries);
        assert_eq!(index.len(), 10);
        assert!(!index.is_empty());

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = index.search(&query, 3, 1)?;
        assert_eq!(results.len(), 3);
        Ok(())
    }

    #[test]
    fn len_and_is_empty() -> Result<()> {
        let scorer = build_test_scorer()?;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        assert!(index.is_empty());
        assert_eq!(index.len(), 0);

        let d = index.scorer().quantizer().profile().ambient_dim as usize;
        let v = vec![0.1f32; d];
        let code = index.scorer().quantizer().encode(&v)?;
        index.add(42, code);
        assert!(!index.is_empty());
        assert_eq!(index.len(), 1);
        Ok(())
    }

    // ------------------------------------------------------------------
    // IVF tests
    // ------------------------------------------------------------------

    #[test]
    fn ivf_build_and_search_returns_results() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 100);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        // Build IVF with 10 centroids.
        index.build_ivf(10)?;
        assert!(index.ivf_is_built(), "IVF should be built");
        assert_eq!(index.ivf().unwrap().num_centroids(), 10);
        assert_eq!(index.ivf().unwrap().assignments().len(), 100);

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        assert_eq!(query.len(), d);

        // Search with nprobe=4 (less than 10 centroids).
        let results = index.search_ivf(&query, 5, 2, 4)?;
        assert!(!results.is_empty(), "IVF search should return results");
        assert!(
            results.len() <= 10,
            "should return at most top_k*oversample=10"
        );

        // Verify results are sorted descending.
        for w in results.windows(2) {
            assert!(
                w[0].approximate_score >= w[1].approximate_score,
                "IVF results not sorted descending"
            );
        }
        Ok(())
    }

    #[test]
    fn ivf_nprobe_fewer_than_all_probes_fewer_entries() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 100);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        index.build_ivf(10)?;

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];

        // nprobe=1 should score fewer entries than nprobe=10 (all cells).
        let (_results1, receipt1) = index.search_with_receipt_ivf(&query, 5, 2, 1)?;
        let (_results_all, receipt_all) = index.search_with_receipt_ivf(&query, 5, 2, 10)?;

        assert!(receipt1.ivf_used, "IVF should be used");
        assert_eq!(receipt1.nprobe, 1);
        assert_eq!(receipt_all.nprobe, 10);
        assert!(
            receipt1.entries_scored <= receipt_all.entries_scored,
            "nprobe=1 should score <= entries than nprobe=10: {} vs {}",
            receipt1.entries_scored,
            receipt_all.entries_scored
        );
        assert_eq!(
            receipt_all.entries_scored, 100,
            "nprobe=10 (all centroids) should score all 100 entries"
        );
        Ok(())
    }

    #[test]
    fn ivf_fallback_when_not_built() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 20);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        // Do NOT build IVF — should fall back to linear scan.
        assert!(!index.ivf_is_built(), "IVF should not be built");

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = index.search_ivf(&query, 5, 2, 4)?;
        assert_eq!(
            results.len(),
            10,
            "fallback linear scan should return top_k*oversample=10"
        );

        // Verify receipt shows fallback.
        let (results, receipt) = index.search_with_receipt_ivf(&query, 5, 2, 4)?;
        assert!(!receipt.ivf_used, "receipt should show IVF not used");
        assert_eq!(receipt.num_centroids, 0);
        assert_eq!(receipt.nprobe, 0);
        assert_eq!(receipt.entries_scored, 20);
        assert_eq!(results.len(), 10);
        Ok(())
    }

    #[test]
    fn ivf_build_default_uses_sqrt_n() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 100);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        // sqrt(100) = 10 centroids.
        index.build_ivf_default()?;
        assert!(index.ivf_is_built());
        assert_eq!(index.ivf().unwrap().num_centroids(), 10);

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let results = index.search_ivf(&query, 5, 1, 8)?;
        assert!(!results.is_empty());
        Ok(())
    }

    #[test]
    fn ivf_receipt_fields_correct() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer().profile().ambient_dim as usize;
        let mut index: FibSidecarIndex<u32> = FibSidecarIndex::new(scorer);

        let vectors = make_vectors(d, 100);
        for (i, v) in vectors.iter().enumerate() {
            let code = index.scorer().quantizer().encode(v)?;
            index.add(i as u32, code);
        }

        index.build_ivf(10)?;

        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let (results, receipt) = index.search_with_receipt_ivf(&query, 5, 2, 4)?;

        assert_eq!(receipt.schema, "fib_sidecar_search_ivf_receipt_v1");
        assert_eq!(receipt.indexed_count, 100);
        assert_eq!(receipt.top_k, 5);
        assert_eq!(receipt.oversample, 2);
        assert_eq!(receipt.candidate_count, results.len());
        assert!(receipt.approximate_only);
        assert!(receipt.exact_rerank_required);
        assert_eq!(receipt.num_centroids, 10);
        assert_eq!(receipt.nprobe, 4);
        assert!(receipt.ivf_used);
        assert!(
            receipt.entries_scored <= 100,
            "entries_scored should be <= total entries"
        );
        assert!(
            receipt.entries_scored > 0,
            "entries_scored should be > 0 with 100 entries and nprobe=4"
        );
        Ok(())
    }
}
