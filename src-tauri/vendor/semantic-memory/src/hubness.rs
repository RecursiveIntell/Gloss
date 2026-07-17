// FFI: the workspace forbids unsafe, but this module calls a C kernel via FFI.
// The `unsafe` is isolated to a single `extern "C"` call with validated inputs
// (length + emptiness checked before the call).  The C side returns NaN for
// degenerate inputs (zero norms), which we convert back to `None`.
#![allow(unsafe_code)]

/// Deterministic CPU-only hubness scoring over a set of dense embeddings.
///
/// Hubness is the tendency of a small number of vectors to appear in the
/// k-nearest-neighbour lists of many other vectors in high-dimensional spaces.
/// This module computes a lightweight hub score for each item given a flat
/// slice of (id, embedding) pairs.

/// Per-item hubness score.
#[derive(Debug, Clone, PartialEq)]
pub struct HubnessScore {
    pub item_id: String,
    /// How many times this item appeared in another item's top-k neighbour list.
    pub neighbor_hits: usize,
    /// `neighbor_hits / max_possible_hits` where `max_possible_hits = n - 1`.
    /// Zero when there are fewer than two items.
    pub normalized_score: f32,
}

// FFI to the C SIMD kernel (c-kernels/similarity.c, compiled via build.rs).
// The C function returns NaN for zero-norm or degenerate inputs; we convert
// NaN back to `None` so the public API is unchanged.
extern "C" {
    fn sm_cosine_similarity(a: *const f32, b: *const f32, n: usize) -> f32;
}

/// Cosine similarity between two equal-length vectors.
///
/// Returns `None` if the slices have different lengths, either has zero norm,
/// or either is empty.
///
/// The actual computation is delegated to a C SIMD kernel
/// (`c-kernels/similarity.c`) compiled with optimization enabled. AVX2/FMA
/// flags are used only when Cargo reports those target features.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let result = unsafe { sm_cosine_similarity(a.as_ptr(), b.as_ptr(), a.len()) };
    if result.is_nan() {
        return None;
    }
    Some(result)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, v: Vec<f32>) -> (String, Vec<f32>) {
        (id.to_string(), v)
    }

    // ── cosine_similarity ────────────────────────────────────────────────────

    #[test]
    fn cosine_rejects_zero_vector() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), None);
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 0.0]), None);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[0.0, 0.0]), None);
    }

    #[test]
    fn cosine_rejects_empty() {
        assert_eq!(cosine_similarity(&[], &[]), None);
    }

    #[test]
    fn cosine_rejects_dimension_mismatch() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 0.0]), None);
    }

    #[test]
    fn cosine_identical_vectors() {
        let v = [1.0_f32, 2.0, 3.0];
        let s = cosine_similarity(&v, &v).unwrap();
        assert!((s - 1.0).abs() < 1e-6, "expected 1.0, got {s}");
    }

    #[test]
    fn cosine_matches_archived_rust_reference() {
        for dim in [1, 2, 3, 16, 128, 768] {
            let a: Vec<f32> = (0..dim).map(|i| ((i as f32 + 1.0) * 0.017).sin()).collect();
            let b: Vec<f32> = (0..dim).map(|i| ((i as f32 + 3.0) * 0.023).cos()).collect();
            let c = cosine_similarity(&a, &b).unwrap();
            let rust = crate::archive::hubness_rust::cosine_similarity(&a, &b).unwrap();
            assert!((c - rust).abs() <= 2.0e-5, "dim={dim}: C={c}, Rust={rust}");
        }
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let s = cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).unwrap();
        assert!(s.abs() < 1e-6, "expected 0.0, got {s}");
    }

    // ── compute_hubness_scores ───────────────────────────────────────────────

    #[test]
    fn empty_input_returns_empty() {
        let scores = compute_hubness_scores(&[], 5);
        assert!(scores.is_empty());
    }

    #[test]
    fn top_k_zero_returns_zero_hits() {
        let data = vec![mk("a", vec![1.0, 0.0]), mk("b", vec![0.0, 1.0])];
        let scores = compute_hubness_scores(&data, 0);
        assert!(scores.iter().all(|s| s.neighbor_hits == 0));
    }

    #[test]
    fn central_vector_has_higher_or_equal_hubness() {
        // "center" is close to both "near_a" and "near_b".
        // "isolated" is orthogonal to everything.
        let data = vec![
            mk("center", vec![1.0, 1.0, 0.0]),
            mk("near_a", vec![1.0, 0.9, 0.0]),
            mk("near_b", vec![0.9, 1.0, 0.0]),
            mk("isolated", vec![0.0, 0.0, 1.0]),
        ];
        let scores = compute_hubness_scores(&data, 1);
        let center_hits = scores
            .iter()
            .find(|s| s.item_id == "center")
            .unwrap()
            .neighbor_hits;
        let isolated_hits = scores
            .iter()
            .find(|s| s.item_id == "isolated")
            .unwrap()
            .neighbor_hits;
        assert!(
            center_hits >= isolated_hits,
            "center({center_hits}) should be >= isolated({isolated_hits})"
        );
    }

    #[test]
    fn dimension_mismatch_is_skipped_not_panicked() {
        let data = vec![
            mk("a", vec![1.0, 0.0]),
            mk("b", vec![1.0, 0.0, 0.0]), // wrong dim — skipped
            mk("c", vec![1.0, 0.0]),
        ];
        // Should not panic; "b" may get 0 hits because its pairs are skipped.
        let scores = compute_hubness_scores(&data, 1);
        assert_eq!(scores.len(), 3);
        let b = scores.iter().find(|s| s.item_id == "b").unwrap();
        // "b" has a mismatched dim so it cannot appear in anyone's neighbour list.
        assert_eq!(b.neighbor_hits, 0);
    }

    #[test]
    fn deterministic_tie_ordering() {
        // All three vectors are identical, so ties in neighbor_hits are resolved by item_id.
        let data = vec![
            mk("gamma", vec![1.0, 0.0]),
            mk("alpha", vec![1.0, 0.0]),
            mk("beta", vec![1.0, 0.0]),
        ];
        let scores = compute_hubness_scores(&data, 2);
        // All should have the same neighbor_hits; order must be alpha < beta < gamma.
        let ids: Vec<&str> = scores.iter().map(|s| s.item_id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn normalized_score_bounded() {
        let data = vec![
            mk("a", vec![1.0, 0.0]),
            mk("b", vec![0.5, 0.5]),
            mk("c", vec![0.0, 1.0]),
        ];
        let scores = compute_hubness_scores(&data, 2);
        for s in &scores {
            assert!(
                s.normalized_score >= 0.0 && s.normalized_score <= 1.0,
                "{}: normalized_score {} out of [0,1]",
                s.item_id,
                s.normalized_score
            );
        }
    }

    #[test]
    fn single_item_returns_zero_hits() {
        let data = vec![mk("only", vec![1.0, 0.0])];
        let scores = compute_hubness_scores(&data, 1);
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].neighbor_hits, 0);
        assert_eq!(scores[0].normalized_score, 0.0);
    }
}
