use crate::search::cosine_similarity;

/// Configuration for multi-resolution Matryoshka search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatryoshkaConfig {
    /// Candidate embedding dimensions, typically a progression from coarse to fine.
    pub dimensions: Vec<usize>,
    /// Candidate stage dimension for approximate retrieval.
    pub candidate_dim: usize,
    /// Exact rerank dimension.
    pub exact_dim: usize,
}

impl Default for MatryoshkaConfig {
    fn default() -> Self {
        Self {
            dimensions: vec![64, 256, 768],
            candidate_dim: 64,
            exact_dim: 768,
        }
    }
}

/// Result envelope for multi-resolution retrieval.
#[derive(Debug, Clone)]
pub struct MultiResolutionResult {
    /// Final ranked matches.
    pub results: Vec<(String, f64)>,
    /// Candidate-stage dimensionality actually used.
    pub candidate_dim_used: usize,
    /// Exact-stage dimensionality actually used.
    pub exact_dim_used: usize,
    /// Estimated recall for `candidate_dim_used` versus `exact_dim_used`.
    pub estimated_recall: f64,
}

/// Truncate an embedding to the target dimensionality using a prefix slice.
pub fn truncate_embedding(embedding: &[f32], target_dim: usize) -> Vec<f32> {
    let effective_dim = target_dim.min(embedding.len());
    embedding[..effective_dim].to_vec()
}

/// Two-stage cosine search:
/// 1) score candidates at `candidate_dim` and keep the top 3 × `top_k`,
/// 2) rerank those with full `exact_dim`, and
/// 3) return the top `top_k` results.
pub fn multi_resolution_search(
    query: &[f32],
    candidates: &[(String, Vec<f32>)],
    candidate_dim: usize,
    exact_dim: usize,
    top_k: usize,
) -> Vec<(String, f64)> {
    if query.is_empty() || candidates.is_empty() || top_k == 0 {
        return Vec::new();
    }

    let query_candidate = truncate_embedding(query, candidate_dim);
    let query_exact = truncate_embedding(query, exact_dim);

    let mut stage1: Vec<(String, &Vec<f32>, f64)> = candidates
        .iter()
        .filter_map(|(id, embedding)| {
            let candidate_prefix = truncate_embedding(embedding, candidate_dim);
            cosine_similarity(&query_candidate, &candidate_prefix)
                .ok()
                .map(|score| (id.clone(), embedding, f64::from(score)))
        })
        .collect();

    stage1.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let rerank_limit = top_k.saturating_mul(3);
    let mut stage2: Vec<(String, f64)> = stage1
        .into_iter()
        .take(rerank_limit)
        .filter_map(|(id, embedding, _)| {
            let candidate_full = truncate_embedding(embedding, exact_dim);
            cosine_similarity(&query_exact, &candidate_full)
                .ok()
                .map(|score| (id, f64::from(score)))
        })
        .collect();

    stage2.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    stage2.truncate(top_k);
    stage2
}

/// Heuristic recall estimate for a candidate dimension versus full exact dimension.
pub fn estimate_recall_at_dim(dim: usize, full_dim: usize) -> f64 {
    if dim == 0 || full_dim == 0 {
        return 0.0;
    }

    let ratio = (dim as f64 / full_dim as f64).clamp(0.0, 1.0);
    ratio.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::cosine_similarity;

    #[test]
    fn truncate_embedding_returns_prefix() {
        let embedding = vec![0.1, 0.2, 0.3, 0.4];

        assert_eq!(truncate_embedding(&embedding, 2), vec![0.1, 0.2]);
        assert_eq!(truncate_embedding(&embedding, 0), Vec::<f32>::new());
        assert_eq!(truncate_embedding(&embedding, 8), embedding);
    }

    #[test]
    fn multi_resolution_search_reproducible_with_candidate_and_exact() {
        let query: Vec<f32> = (0..768).map(|idx| (idx % 5) as f32 * 0.15).collect();

        let candidates = vec![
            ("full_match".to_string(), vec![1.0; 768]),
            {
                let mut truncated_match = vec![0.0; 768];
                for v in truncated_match.iter_mut().take(64) {
                    *v = 1.0;
                }
                ("truncated_match".to_string(), truncated_match)
            },
            ("control".to_string(), {
                let mut control = vec![0.0; 768];
                for v in control.iter_mut().skip(64) {
                    *v = 1.0;
                }
                control
            }),
        ];
        let result = multi_resolution_search(&query, &candidates, 64, 768, 2);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "full_match");
    }

    #[test]
    fn multi_resolution_search_same_as_direct_when_dims_match() {
        let query: Vec<f32> = (0..64).map(|idx| (idx as f32).sin() / 8.0).collect();
        let candidates = vec![
            ("a".to_string(), vec![1.0; 64]),
            ("b".to_string(), {
                let mut candidate = vec![0.0; 64];
                candidate[0] = 1.0;
                candidate
            }),
            ("c".to_string(), vec![0.5; 64]),
        ];

        let mut direct: Vec<(String, f64)> = candidates
            .iter()
            .filter_map(|(id, embedding)| {
                cosine_similarity(&query, embedding)
                    .ok()
                    .map(|score| (id.clone(), f64::from(score)))
            })
            .collect();
        direct.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });

        let result = multi_resolution_search(&query, &candidates, 64, 64, 3);
        assert_eq!(result, direct);
    }

    #[test]
    fn estimate_recall_is_monotonic_with_dimension() {
        let lower = estimate_recall_at_dim(64, 768);
        let higher = estimate_recall_at_dim(256, 768);

        assert!(higher > lower);
        assert!((estimate_recall_at_dim(768, 768) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn multi_resolution_search_empty_candidates_is_empty() {
        let result = multi_resolution_search(&[1.0, 2.0, 3.0], &[], 64, 768, 10);
        assert!(result.is_empty());
    }
}
