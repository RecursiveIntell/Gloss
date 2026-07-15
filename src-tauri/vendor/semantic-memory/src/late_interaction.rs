//! ColBERT-style late interaction scoring primitives.

use std::collections::HashMap;

/// Configuration for late-interaction indexing and scoring.
#[derive(Debug, Clone)]
pub struct LateInteractionConfig {
    /// Whether late interaction scoring is enabled for this index instance.
    pub enabled: bool,
    /// Maximum number of token embeddings retained per document.
    pub max_tokens_per_doc: usize,
    /// Minimum per-token similarity required before provenance is emitted.
    pub similarity_threshold: f32,
}

impl Default for LateInteractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_tokens_per_doc: 512,
            similarity_threshold: 0.0,
        }
    }
}

/// In-memory late-interaction index storing per-document token embeddings.
#[derive(Debug, Clone, Default)]
pub struct LateInteractionIndex {
    /// Per-document token vectors. Each inner vector is one token embedding.
    pub documents: HashMap<String, Vec<Vec<f32>>>,
    /// Scoring controls for this index.
    pub config: LateInteractionConfig,
}

impl LateInteractionIndex {
    /// Build an empty index with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
            config: LateInteractionConfig::default(),
        }
    }

    /// Build an empty index with a pre-sized document map.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            documents: HashMap::with_capacity(capacity),
            config: LateInteractionConfig::default(),
        }
    }

    /// Build an empty index with custom configuration.
    #[must_use]
    pub fn with_config(config: LateInteractionConfig) -> Self {
        Self {
            documents: HashMap::new(),
            config,
        }
    }

    /// Insert or replace the token embeddings for one document.
    pub fn upsert_document(&mut self, doc_id: String, mut token_embeddings: Vec<Vec<f32>>) {
        token_embeddings.truncate(self.config.max_tokens_per_doc);
        self.documents.insert(doc_id, token_embeddings);
    }

    /// Compute top late-interaction scores for every document in the index.
    ///
    /// Returns only non-empty documents; empty documents are skipped to keep empty-index
    /// behavior predictable and cheap.
    pub fn score_documents(&self, query_tokens: &[Vec<f32>]) -> Vec<(String, f32)> {
        if !self.config.enabled || query_tokens.is_empty() || self.documents.is_empty() {
            return Vec::new();
        }

        let mut results: Vec<(String, f32)> = self
            .documents
            .iter()
            .filter_map(|(doc_id, doc_tokens)| {
                if doc_tokens.is_empty() {
                    return None;
                }
                let (_, matches) = score_with_provenance(query_tokens, doc_tokens, doc_id.as_str());
                let maxsim_score = matches
                    .iter()
                    .filter(|token_match| {
                        token_match.similarity >= self.config.similarity_threshold
                    })
                    .map(|token_match| token_match.similarity)
                    .sum::<f32>();
                if maxsim_score <= self.config.similarity_threshold {
                    return None;
                }
                Some((doc_id.clone(), maxsim_score))
            })
            .collect();

        results.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.0.cmp(&a.0))
        });

        results
    }

    /// Replace the index configuration.
    pub fn set_config(&mut self, config: LateInteractionConfig) {
        self.config = config;
    }

    /// Maximum number of token embeddings this index stores per document.
    #[allow(dead_code)]
    fn max_tokens_per_doc(&self) -> usize {
        self.config.max_tokens_per_doc
    }
}

/// Result record for a query-token to document-token alignment.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenMatch {
    /// Index of the query token.
    pub query_token_idx: usize,
    /// Index of the best-matching token in the target document.
    pub doc_token_idx: usize,
    /// Owning document id.
    pub doc_id: String,
    /// Cosine similarity for the best match.
    pub similarity: f32,
}

/// Compute ColBERT MaxSim score for a single query/document pair.
///
/// For each query token, this finds the best cosine match among all document tokens and
/// sums those maxima.
pub fn score_maxsim(query_tokens: &[Vec<f32>], doc_tokens: &[Vec<f32>]) -> f32 {
    let mut score = 0.0f32;

    for query_token in query_tokens {
        let mut best = None;

        for doc_token in doc_tokens {
            let sim = cosine_similarity(query_token, doc_token);
            let should_use = match best {
                None => true,
                Some(current) => sim > current,
            };
            if should_use {
                best = Some(sim);
            }
        }

        if let Some(best_similarity) = best {
            score += best_similarity;
        }
    }

    score
}

/// Compute score and token-level provenance for one query/document pair.
///
/// `doc_id` is copied onto each returned match row so callers can retain provenance for fused
/// downstream consumers.
pub fn score_with_provenance(
    query_tokens: &[Vec<f32>],
    doc_tokens: &[Vec<f32>],
    doc_id: &str,
) -> (f32, Vec<TokenMatch>) {
    let mut score = 0.0f32;
    let mut matches = Vec::new();

    if doc_tokens.is_empty() {
        return (0.0, matches);
    }

    for (query_token_idx, query_token) in query_tokens.iter().enumerate() {
        let mut best: Option<(usize, f32)> = None;

        for (doc_token_idx, doc_token) in doc_tokens.iter().enumerate() {
            let similarity = cosine_similarity(query_token, doc_token);
            let is_better = match best {
                None => true,
                Some((_, current_similarity)) => similarity > current_similarity,
            };
            if is_better {
                best = Some((doc_token_idx, similarity));
            }
        }

        if let Some((doc_token_idx, similarity)) = best {
            score += similarity;
            matches.push(TokenMatch {
                query_token_idx,
                doc_token_idx,
                doc_id: doc_id.to_string(),
                similarity,
            });
        }
    }

    (score, matches)
}

/// Convert late-interaction scores into a Reciprocal Rank Fusion style ranking.
///
/// Ranks are ordered by descending score, with ties resolved by document id for stability.
pub fn late_interaction_rrf_rank(scores: &[(String, f32)], k: usize) -> Vec<(String, f64)> {
    let mut ranked = scores.to_vec();
    ranked.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    ranked
        .into_iter()
        .enumerate()
        .map(|(rank, (id, _))| (id, 1.0_f64 / (k as f64 + (rank as f64 + 1.0))))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let has_invalid = a.iter().chain(b.iter()).any(|value| !value.is_finite());
    if has_invalid {
        return 0.0;
    }

    let norm_a: f32 = a.iter().map(|value| value * value).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    let dot: f32 = a
        .iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum();
    let similarity = dot / (norm_a * norm_b);
    if similarity.is_finite() {
        similarity
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maxsim_scoring_with_known_vectors() {
        let query_tokens = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let doc_tokens = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let score = score_maxsim(&query_tokens, &doc_tokens);
        assert!((score - 2.0).abs() < 1e-6);
    }

    #[test]
    fn score_with_provenance_tracks_token_alignments() {
        let query_tokens = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let doc_tokens = vec![vec![1.0, 0.0], vec![0.1, 0.9], vec![0.0, 1.0]];
        let (_, matches) = score_with_provenance(&query_tokens, &doc_tokens, "doc:1");

        assert_eq!(matches.len(), 2);
        assert_eq!(
            matches.first(),
            Some(&TokenMatch {
                query_token_idx: 0,
                doc_token_idx: 0,
                doc_id: "doc:1".to_string(),
                similarity: 1.0,
            })
        );
        assert_eq!(
            matches.get(1),
            Some(&TokenMatch {
                query_token_idx: 1,
                doc_token_idx: 2,
                doc_id: "doc:1".to_string(),
                similarity: 1.0,
            })
        );
    }

    #[test]
    fn late_interaction_rrf_rank_matches_expected_ordering() {
        let candidates = vec![
            ("doc:3".to_string(), 0.2),
            ("doc:1".to_string(), 2.0),
            ("doc:2".to_string(), 1.0),
        ];
        let ranked = late_interaction_rrf_rank(&candidates, 60);
        let ids: Vec<String> = ranked.iter().map(|(doc_id, _)| doc_id.clone()).collect();

        assert_eq!(ids, vec!["doc:1", "doc:2", "doc:3"]);
        assert!((ranked[0].1 - 1.0 / 61.0).abs() < 1e-12);
        assert!((ranked[1].1 - 1.0 / 62.0).abs() < 1e-12);
        assert!((ranked[2].1 - 1.0 / 63.0).abs() < 1e-12);
    }

    #[test]
    fn empty_index_returns_no_results() {
        let index = LateInteractionIndex::new();
        let ranked = index.score_documents(&[vec![1.0, 0.0]]);
        assert!(ranked.is_empty());
    }
}
