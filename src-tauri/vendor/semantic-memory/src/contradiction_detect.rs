//! Content-based contradiction detection (lexical, deterministic, local-first).
//!
//! The rest of the contradiction machinery — the decoder, the community scan,
//! the factor graph — *consumes* contradiction pairs that were asserted as
//! graph edges. Nothing infers a contradiction from the text of two facts.
//! This module is that missing piece: given a set of co-retrieved items, it
//! proposes candidate contradiction pairs from their content alone.
//!
//! v1 is purely lexical and deterministic (no embeddings, no LLM, no I/O) so it
//! runs in CI and on a cold machine. A pair is proposed only when the two items
//! are *about the same thing* (a topical-overlap gate) **and** carry a concrete
//! disagreement signal:
//!
//! - **NumericDisagreement** — near-identical wording but different numbers
//!   ("exposes 33 tools" vs "exposes 12 tools").
//! - **ValueDisagreement** — same subject, different entity-like value
//!   ("the default embedder is Candle" vs "… is Ollama").
//! - **NegationPolarity** — near-identical wording, one negated
//!   ("X is supported" vs "X is not supported").
//! - **Antonym** — near-identical wording differing by a known antonym pair
//!   ("the feature is enabled" vs "… disabled").
//!
//! The detector's output is exactly what [`crate::eval_contradiction`] scores,
//! and what an ingestion pass could turn into `contradicts` graph edges.
//! Embedding-similarity gating (replacing the lexical topical gate) and an NLI
//! signal are the natural v2 upgrades.
//!
//! Behind `#[cfg(feature = "decoder")]` (the contradiction cluster).

use crate::search::cosine_similarity;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ─── Signals ─────────────────────────────────────────────────────────────────

/// The kind of disagreement that justified a proposed contradiction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContradictionSignal {
    /// Same wording, different numbers.
    NumericDisagreement,
    /// Same subject, different entity-like value.
    ValueDisagreement,
    /// Same wording, opposite negation polarity.
    NegationPolarity,
    /// Same wording differing by a known antonym pair.
    Antonym,
    /// High embedding similarity (same neighborhood) but a negation/antonym
    /// signal fired — paraphrased contradiction that lexical overlap misses.
    EmbeddingAntipode,
}

/// A proposed contradiction between two items, with the evidence for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedPair {
    /// First item id.
    pub a: String,
    /// Second item id.
    pub b: String,
    /// Confidence in `[0,1]` (topical overlap blended with signal count).
    pub score: f64,
    /// Signals that fired for this pair.
    pub signals: Vec<ContradictionSignal>,
    /// Human-readable explanation.
    pub reason: String,
}

/// Detector thresholds.
#[derive(Debug, Clone, Copy)]
pub struct DetectorConfig {
    /// Minimum Jaccard overlap of non-numeric content words for the numeric /
    /// negation / antonym signals (these compare whole sentences).
    pub min_overlap: f64,
    /// Minimum subject overlap for the value-disagreement signal.
    pub min_subject_overlap: f64,
    /// Minimum cosine similarity for the embedding-based contradiction gate.
    /// Pairs above this threshold are checked for negation/antonym signals
    /// even when lexical overlap is below `min_overlap`.
    pub embedding_similarity_threshold: f64,
    /// When true, use embeddings (if available) as the proximity gate instead
    /// of lexical Jaccard overlap.
    pub use_embeddings: bool,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            min_overlap: 0.6,
            min_subject_overlap: 0.6,
            embedding_similarity_threshold: 0.75,
            use_embeddings: false,
        }
    }
}

// ─── Lexical helpers ─────────────────────────────────────────────────────────

const STOPWORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "of", "to", "in", "on",
    "by", "with", "and", "or", "for", "it", "its", "this", "that", "these", "those", "as", "at",
    "from", "into", "via", "per", "but", "not", "no", "than", "then", "so", "such",
];

fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w)
}

/// Lowercased alphanumeric tokens.
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Whether a token is a bare number (e.g. `"33"`, `"1.5"`).
fn is_number_token(t: &str) -> bool {
    t.parse::<f64>().is_ok()
}

/// Content words: lowercased, length ≥ 2, no stopwords, no bare numbers.
fn content_words(s: &str) -> HashSet<String> {
    tokens(s)
        .into_iter()
        .filter(|t| t.len() >= 2 && !is_stopword(t) && !is_number_token(t))
        .collect()
}

/// Numbers appearing in the text.
fn numbers(s: &str) -> HashSet<String> {
    // Keep the raw token so "33" and "33.0" stay distinct; good enough for v1.
    s.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|t| !t.is_empty() && t.parse::<f64>().is_ok())
        .map(|t| t.to_string())
        .collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count() as f64;
    let union = a.union(b).count() as f64;
    if union == 0.0 {
        0.0
    } else {
        inter / union
    }
}

const NEGATIONS: &[&str] = &[
    "not", "no", "never", "cannot", "without", "n't", "isn", "aren", "doesn",
];

/// Whether the text carries a negation marker.
fn has_negation(s: &str) -> bool {
    let toks = tokens(s);
    toks.iter().any(|t| NEGATIONS.contains(&t.as_str())) || s.to_lowercase().contains("n't")
}

const ANTONYMS: &[(&str, &str)] = &[
    ("enabled", "disabled"),
    ("true", "false"),
    ("on", "off"),
    ("supported", "unsupported"),
    ("active", "deprecated"),
    ("present", "absent"),
    ("increase", "decrease"),
    ("allowed", "denied"),
    ("valid", "invalid"),
];

/// First antonym pair present across the two word sets, if any.
fn antonym_hit(a: &HashSet<String>, b: &HashSet<String>) -> bool {
    ANTONYMS
        .iter()
        .any(|(x, y)| (a.contains(*x) && b.contains(*y)) || (a.contains(*y) && b.contains(*x)))
}

/// Extract `(subject_words, value_token)` from an `<subject> is/are <value>`
/// sentence, where the value is *entity-like* (capitalized in the original).
/// Returns `None` when there is no copula or the value is not entity-like —
/// this keeps "the server is fast" from competing with "the server is built".
fn subject_value(original: &str) -> Option<(HashSet<String>, String)> {
    // Find " is " or " are " (with surrounding spaces to avoid substrings).
    let (idx, kw_len) = original.char_indices().find_map(|(idx, _)| {
        [" is ", " are ", " was ", " were "].iter().find_map(|kw| {
            original
                .get(idx..idx + kw.len())
                .filter(|candidate| candidate.eq_ignore_ascii_case(kw))
                .map(|_| (idx, kw.len()))
        })
    })?;
    let subject = &original[..idx];
    let after = &original[idx + kw_len..];

    // Value = first token after the copula that is entity-like: starts with an
    // uppercase letter in the original text (proper noun / named value).
    let value = after
        .split(|c: char| !c.is_alphanumeric())
        .find(|t| !t.is_empty())?;
    let entity_like = value
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false);
    if !entity_like {
        return None;
    }
    Some((content_words(subject), value.to_lowercase()))
}

// ─── Detector ────────────────────────────────────────────────────────────────

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Evaluate one ordered pair of items; return a [`DetectedPair`] if any signal
/// fires. `(a_id, a_text)` and `(b_id, b_text)`. When `embeddings` are provided,
/// uses cosine similarity as the proximity gate instead of lexical Jaccard.
fn evaluate_pair(
    a_id: &str,
    a_text: &str,
    b_id: &str,
    b_text: &str,
    cfg: &DetectorConfig,
    a_emb: Option<&[f32]>,
    b_emb: Option<&[f32]>,
) -> Option<DetectedPair> {
    let a_words = content_words(a_text);
    let b_words = content_words(b_text);
    let overlap = jaccard(&a_words, &b_words);

    // Determine proximity: embedding cosine similarity if available, else lexical.
    let (proximal, embedding_sim) = match (a_emb, b_emb) {
        (Some(ae), Some(be)) if ae.len() == be.len() => {
            let sim = cosine_similarity(ae, be).unwrap_or(0.0) as f64;
            (sim >= cfg.embedding_similarity_threshold, Some(sim))
        }
        _ => (overlap >= cfg.min_overlap, None),
    };

    let mut signals = Vec::new();
    let mut reasons = Vec::new();

    // 1. Numeric disagreement: same wording (sans numbers), different numbers.
    if proximal {
        let (na, nb) = (numbers(a_text), numbers(b_text));
        if !na.is_empty() && !nb.is_empty() && na.is_disjoint(&nb) {
            signals.push(ContradictionSignal::NumericDisagreement);
            reasons.push(format!(
                "same wording (overlap {:.2}) but different numbers {:?} vs {:?}",
                overlap, na, nb
            ));
        }
    }

    // 2. Value disagreement: same subject, different entity-like value.
    if let (Some((sa, va)), Some((sb, vb))) = (subject_value(a_text), subject_value(b_text)) {
        if va != vb && jaccard(&sa, &sb) >= cfg.min_subject_overlap {
            signals.push(ContradictionSignal::ValueDisagreement);
            reasons.push(format!(
                "same subject asserts different values '{va}' vs '{vb}'"
            ));
        }
    }

    // 3 & 4. Negation polarity / antonym: require proximity (embedding or lexical).
    if proximal {
        if has_negation(a_text) != has_negation(b_text) {
            if let Some(similarity) = embedding_sim.filter(|_| overlap < cfg.min_overlap) {
                // Flagged via embedding proximity, not lexical overlap — this is
                // a paraphrased contradiction that the lexical gate would miss.
                signals.push(ContradictionSignal::EmbeddingAntipode);
                reasons.push(format!(
                    "embedding similarity {:.2} but opposite negation (paraphrased contradiction)",
                    similarity
                ));
            } else {
                signals.push(ContradictionSignal::NegationPolarity);
                reasons.push(format!(
                    "same wording (overlap {overlap:.2}) but opposite negation"
                ));
            }
        }
        if antonym_hit(&a_words, &b_words) {
            if let Some(similarity) = embedding_sim.filter(|_| overlap < cfg.min_overlap) {
                signals.push(ContradictionSignal::EmbeddingAntipode);
                reasons.push(format!(
                    "embedding similarity {:.2} but antonym pair (paraphrased contradiction)",
                    similarity
                ));
            } else {
                signals.push(ContradictionSignal::Antonym);
                reasons.push("antonym pair across otherwise-similar items".to_string());
            }
        }
    }

    if signals.is_empty() {
        return None;
    }
    // Score: proximity blended with how many signals fired.
    let proximity_score = embedding_sim.unwrap_or(overlap);
    let score = clamp01(0.4 + 0.2 * signals.len() as f64 + 0.3 * proximity_score);
    Some(DetectedPair {
        a: a_id.to_string(),
        b: b_id.to_string(),
        score,
        signals,
        reason: reasons.join("; "),
    })
}

/// Propose candidate contradictions across a set of items. `items` is
/// `(id, content)`. O(n²) over the set — intended for a retrieved working set
/// (tens of items), not the whole corpus. Uses lexical Jaccard overlap as the
/// proximity gate.
pub fn detect_contradictions(
    items: &[(String, String)],
    cfg: &DetectorConfig,
) -> Vec<DetectedPair> {
    let mut out = Vec::new();
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if let Some(pair) = evaluate_pair(
                &items[i].0,
                &items[i].1,
                &items[j].0,
                &items[j].1,
                cfg,
                None,
                None,
            ) {
                out.push(pair);
            }
        }
    }
    out
}

/// Propose candidate contradictions using embeddings as the proximity gate.
/// `items` is `(id, content, embedding)`. Pairs with cosine similarity above
/// `cfg.embedding_similarity_threshold` are checked for negation/antonym
/// signals. Catches paraphrased contradictions that lexical overlap misses.
pub fn detect_contradictions_semantic(
    items: &[(String, String, Vec<f32>)],
    cfg: &DetectorConfig,
) -> Vec<DetectedPair> {
    let mut out = Vec::new();
    for i in 0..items.len() {
        for j in (i + 1)..items.len() {
            if let Some(pair) = evaluate_pair(
                &items[i].0,
                &items[i].1,
                &items[j].0,
                &items[j].1,
                cfg,
                Some(&items[i].2),
                Some(&items[j].2),
            ) {
                out.push(pair);
            }
        }
    }
    out
}

/// A `contradicts` graph edge proposed from content. Map this onto a stored
/// graph edge (`edge_type = "contradicts"`) so the decoder, community scan, and
/// factor graph — which only consume asserted edges — pick the conflict up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposedEdge {
    /// Source item id.
    pub source: String,
    /// Target item id.
    pub target: String,
    /// Always `"contradicts"`.
    pub edge_type: String,
    /// Detector confidence in `[0,1]`.
    pub score: f64,
    /// Why the edge was proposed.
    pub reason: String,
}

/// Run the detector and shape each proposed pair as a `contradicts` graph edge,
/// ready to persist. This is the bridge from content-based detection into the
/// graph the rest of the contradiction machinery already understands.
pub fn propose_contradiction_edges(
    items: &[(String, String)],
    cfg: &DetectorConfig,
) -> Vec<ProposedEdge> {
    detect_contradictions(items, cfg)
        .into_iter()
        .map(|p| ProposedEdge {
            source: p.a,
            target: p.b,
            edge_type: "contradicts".to_string(),
            score: p.score,
            reason: p.reason,
        })
        .collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn items(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(i, c)| (i.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn detects_numeric_disagreement() {
        let it = items(&[
            ("a", "The MCP server exposes 33 tools."),
            ("b", "The MCP server exposes 12 tools."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .signals
            .contains(&ContradictionSignal::NumericDisagreement));
    }

    #[test]
    fn detects_value_disagreement() {
        let it = items(&[
            ("a", "The default embedder is Candle (in-process, CPU)."),
            ("b", "The default embedder is Ollama."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert_eq!(found.len(), 1);
        assert!(found[0]
            .signals
            .contains(&ContradictionSignal::ValueDisagreement));
    }

    #[test]
    fn detects_negation_polarity() {
        let it = items(&[
            ("a", "The warm server endpoint is reachable on startup."),
            ("b", "The warm server endpoint is not reachable on startup."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert_eq!(found.len(), 1, "negation flip should be flagged");
        assert!(found[0]
            .signals
            .contains(&ContradictionSignal::NegationPolarity));
    }

    #[test]
    fn detects_antonym() {
        let it = items(&[
            ("a", "The autocapture feature is enabled by default."),
            ("b", "The autocapture feature is disabled by default."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert!(found
            .iter()
            .any(|p| p.signals.contains(&ContradictionSignal::Antonym)));
    }

    #[test]
    fn does_not_flag_unrelated_items() {
        let it = items(&[
            ("a", "The warm server listens on port 1739 by default."),
            ("b", "The warm HTTP server is co-hosted by the MCP server."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert!(
            found.is_empty(),
            "different claims, no shared number/value → no flag"
        );
    }

    #[test]
    fn does_not_flag_compatible_same_entity() {
        // Both mention Candle but make compatible claims — must not be flagged.
        let it = items(&[
            ("a", "The default embedder is Candle (in-process, CPU)."),
            ("c", "Candle downloads nomic-embed-text on first use."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert!(
            found.is_empty(),
            "compatible statements about Candle must not be flagged"
        );
    }

    #[test]
    fn proposes_contradicts_edges() {
        let it = items(&[
            ("a", "The MCP server exposes 33 tools."),
            ("b", "The MCP server exposes 12 tools."),
            ("c", "The server is built with the rmcp Rust SDK."),
        ]);
        let edges = propose_contradiction_edges(&it, &DetectorConfig::default());
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, "contradicts");
        assert!(!edges[0].reason.is_empty());
        let pair = (edges[0].source.as_str(), edges[0].target.as_str());
        assert!(pair == ("a", "b") || pair == ("b", "a"));
    }

    #[test]
    fn value_disagreement_ignores_lowercase_predicates() {
        // "is fast" vs "is built" share subject "the server" but values are not
        // entity-like → should NOT be a value disagreement.
        let it = items(&[
            ("a", "The server is fast."),
            ("b", "The server is built with rmcp."),
        ]);
        let found = detect_contradictions(&it, &DetectorConfig::default());
        assert!(
            found.is_empty(),
            "lowercase predicate values must not compete"
        );
    }

    #[test]
    fn semantic_detects_paraphrased_contradiction() {
        // "the server is up" vs "the service is down" — semantically similar
        // but lexically different. Lexical detector misses this; embedding
        // detector catches it via EmbeddingAntipode (negation flip in
        // embedding space). We simulate embeddings with high cosine similarity
        // but opposite negation polarity.
        let emb_a = vec![0.8, 0.1, 0.1, 0.0, 0.0];
        let emb_b = vec![0.79, 0.1, 0.1, 0.0, 0.0]; // very similar embedding
        let items = vec![
            ("a".to_string(), "The server is up.".to_string(), emb_a),
            ("b".to_string(), "The service is not up.".to_string(), emb_b),
        ];
        let cfg = DetectorConfig {
            use_embeddings: true,
            ..DetectorConfig::default()
        };
        let found = detect_contradictions_semantic(&items, &cfg);
        assert!(
            !found.is_empty(),
            "semantic detector should catch paraphrased negation contradiction"
        );
        assert!(
            found[0]
                .signals
                .contains(&ContradictionSignal::EmbeddingAntipode),
            "expected EmbeddingAntipode signal for paraphrased contradiction"
        );
    }

    #[test]
    fn semantic_does_not_flag_similar_non_contradictory() {
        // High embedding similarity but no negation/antonym signal → no contradiction.
        let emb_a = vec![0.8, 0.1, 0.1];
        let emb_b = vec![0.79, 0.1, 0.1];
        let items = vec![
            ("a".to_string(), "The server is fast.".to_string(), emb_a),
            (
                "b".to_string(),
                "The server is reliable.".to_string(),
                emb_b,
            ),
        ];
        let cfg = DetectorConfig {
            use_embeddings: true,
            ..DetectorConfig::default()
        };
        let found = detect_contradictions_semantic(&items, &cfg);
        assert!(
            found.is_empty(),
            "similar but non-contradictory items must not be flagged"
        );
    }
}
