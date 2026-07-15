//! Candidate list + ranking utilities for compressed-domain search

use crate::error::ScorerResult;
use crate::trait_def::ScoredCandidate;

/// A ranked list of candidates from compressed-domain scoring.
///
/// Maintains a bounded top-K heap as candidates are scored, avoiding
/// the need to sort all N candidates.
pub struct CandidateList {
    candidates: heapless::Vec<ScoredCandidate, 256>,
    k: usize,
    min_score: f32,
}

impl CandidateList {
    /// Create a new candidate list that keeps the top-K results.
    pub fn new(k: usize) -> Self {
        Self {
            candidates: heapless::Vec::new(),
            k,
            min_score: f32::NEG_INFINITY,
        }
    }

    /// Consider a candidate. If its score is higher than the current minimum
    /// (or the list isn't full yet), it's added.
    pub fn consider(&mut self, idx: usize, score: f32) {
        if self.candidates.len() < self.k {
            let _ = self.candidates.push(ScoredCandidate::new(idx, score));
            self.update_min();
        } else if score > self.min_score {
            // Replace the worst candidate
            if let Some(worst_idx) = self.find_worst() {
                self.candidates[worst_idx] = ScoredCandidate::new(idx, score);
                self.update_min();
            }
        }
    }

    /// Get the sorted (descending) list of candidates.
    pub fn sorted(&self) -> &[ScoredCandidate] {
        // Note: callers should call sort_descending before accessing
        // For now, we return unsorted and let caller sort
        &self.candidates
    }

    /// Sort candidates by score descending (in-place)
    pub fn sort_descending(&mut self) {
        self.candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
    }

    /// Number of candidates currently held
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Is the list empty?
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    /// Get the candidate indices (unsorted)
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.candidates.iter().map(|c| c.idx)
    }

    /// Drain into a sorted Vec (for when you need owned results)
    pub fn into_sorted(self) -> heapless::Vec<ScoredCandidate, 256> {
        let mut v = self.candidates;
        v.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        v
    }

    fn update_min(&mut self) {
        self.min_score = self
            .candidates
            .iter()
            .map(|c| c.score)
            .fold(f32::INFINITY, f32::min);
    }

    fn find_worst(&self) -> Option<usize> {
        let mut worst_idx = 0;
        let mut worst_score = f32::INFINITY;
        for (i, c) in self.candidates.iter().enumerate() {
            if c.score < worst_score {
                worst_score = c.score;
                worst_idx = i;
            }
        }
        Some(worst_idx)
    }
}

/// Helper: score all compressed vectors and return top-K candidates.
///
/// This is the main entry point for compressed-domain search.
/// It prepares the query, scores all candidates, and returns the top-K.
pub fn search_topk<S: crate::trait_def::CompressedScorer>(
    scorer: &S,
    query: &[f32],
    compressed: &[S::Compressed],
    k: usize,
) -> ScorerResult<heapless::Vec<ScoredCandidate, 256>> {
    if compressed.is_empty() || k == 0 {
        return Ok(heapless::Vec::new());
    }

    let prepared = scorer.prepare_query(query)?;
    let mut candidates = CandidateList::new(k);

    for (idx, code) in compressed.iter().enumerate() {
        let score = scorer.score_prepared(&prepared, code)?;
        candidates.consider(idx, score);
    }

    candidates.sort_descending();
    Ok(candidates.into_sorted())
}
