//! ARCHIVED: Pure-Rust cosine_similarity and compute_hubness_scores.
//!
//! This file preserves the original Rust implementations that were replaced
//! by the C SIMD kernel in `c-kernels/similarity.c` (via FFI in `hubness.rs`).
//!
//! Archived on: 2026-07-12
//! Reason: Performance — C kernel with compiler auto-vectorization; AVX2/FMA
//!         is enabled only for targets that advertise those features.
//!         replaces the pure-Rust dot product / norm computation.
//!
//! The `cosine_similarity` function below is the original pure-Rust version.
//! `compute_hubness_scores` remains in Rust (in hubness.rs) and calls the
//! C-backed `cosine_similarity` — only the inner math was moved to C.
//!
//! To restore the pure-Rust version, copy these functions back into
//! hubness.rs and remove the FFI `extern "C"` block.

/// Cosine similarity between two equal-length vectors.
///
/// Returns `None` if the slices have different lengths, either has zero norm,
/// or either is empty.
#[allow(dead_code)]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return None;
    }
    Some(dot / (norm_a * norm_b))
}

/// Compute hubness scores for a collection of embeddings.
///
/// For each vector, the top-`top_k` most similar other vectors (by cosine
/// similarity) are found. Each neighbour's `neighbor_hits` counter is
/// incremented. Pairs whose embeddings differ in dimension or have a zero
/// norm are skipped rather than panicked.
///
/// The returned list is sorted descending by `neighbor_hits`, with ties
/// broken by `item_id` ascending for full determinism.
#[allow(dead_code)]
pub fn compute_hubness_scores(
    embeddings: &[(String, Vec<f32>)],
    top_k: usize,
) -> Vec<HubnessScore> {
    let n = embeddings.len();
    let mut hits = vec![0usize; n];

    if top_k == 0 || n < 2 {
        // Nothing to accumulate; still return correctly-zeroed scores.
        let max_hits = n.saturating_sub(1);
        let mut scores: Vec<HubnessScore> = embeddings
            .iter()
            .map(|(id, _)| HubnessScore {
                item_id: id.clone(),
                neighbor_hits: 0,
                normalized_score: 0.0,
            })
            .collect();
        scores.sort_unstable_by(|a, b| {
            b.neighbor_hits
                .cmp(&a.neighbor_hits)
                .then_with(|| a.item_id.cmp(&b.item_id))
        });
        let _ = max_hits; // explicitly used below in the normal path
        return scores;
    }

    let max_possible_hits = n.saturating_sub(1);

    for i in 0..n {
        let (_, ref qi) = embeddings[i];
        // Collect similarities to all other items with matching dimension.
        let mut sims: Vec<(f32, &str)> = embeddings
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .filter_map(|(_, (id, emb))| cosine_similarity(qi, emb).map(|s| (s, id.as_str())))
            .collect();

        // Sort descending by similarity; stable tie-break by item_id ascending.
        sims.sort_unstable_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(b.1))
        });

        // Increment hit counter for each top-k neighbour.
        for (_, neighbour_id) in sims.iter().take(top_k) {
            if let Some(j) = embeddings
                .iter()
                .position(|(id, _)| id.as_str() == *neighbour_id)
            {
                hits[j] += 1;
            }
        }
    }

    let mut scores: Vec<HubnessScore> = embeddings
        .iter()
        .enumerate()
        .map(|(i, (id, _))| {
            let h = hits[i];
            let norm = if max_possible_hits == 0 {
                0.0
            } else {
                h as f32 / max_possible_hits as f32
            };
            HubnessScore {
                item_id: id.clone(),
                neighbor_hits: h,
                normalized_score: norm,
            }
        })
        .collect();

    scores.sort_unstable_by(|a, b| {
        b.neighbor_hits
            .cmp(&a.neighbor_hits)
            .then_with(|| a.item_id.cmp(&b.item_id))
    });

    scores
}

/// Per-item hubness score (archived copy — the live one is in hubness.rs).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub struct HubnessScore {
    pub item_id: String,
    pub neighbor_hits: usize,
    pub normalized_score: f32,
}
