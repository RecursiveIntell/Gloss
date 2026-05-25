use crate::retrieval::context::ContextPassage;
use crate::retrieval::hybrid_search::SearchResult;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A citation mapping from a numbered reference to a source chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub chunk_id: String,
    pub source_id: String,
    pub source_title: String,
    pub quote: Option<String>,
    pub page: Option<i32>,
    pub section: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationAnchorV1 {
    pub ref_number: usize,
    pub source_id: String,
    pub chunk_id: String,
    pub quote_digest: String,
    pub evidence_class: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationFilterReasonV1 {
    pub ref_number: usize,
    pub reason_code: String,
    pub detail: String,
}

fn quote_digest(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn citation_anchors_for_context(source_context: &[ContextPassage]) -> Vec<CitationAnchorV1> {
    source_context
        .iter()
        .enumerate()
        .filter_map(|(idx, passage)| {
            passage.chunk_id.as_ref().map(|chunk_id| CitationAnchorV1 {
                ref_number: idx + 1,
                source_id: passage.source_id.clone(),
                chunk_id: chunk_id.clone(),
                quote_digest: quote_digest(&passage.content),
                evidence_class: passage.evidence_class.clone(),
            })
        })
        .collect()
}

pub fn count_unique_citation_refs(response: &str) -> usize {
    let re = Regex::new(r"\[(\d+)\]").unwrap();
    re.captures_iter(response)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Extract citation references [1], [2], etc. from LLM output
/// and map them to the provided search results.
#[allow(dead_code)]
pub fn extract_citations(
    response: &str,
    search_results: &[SearchResult],
    source_titles: &std::collections::HashMap<String, String>,
) -> Vec<Citation> {
    let re = Regex::new(r"\[(\d+)\]").unwrap();
    let mut citations = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cap in re.captures_iter(response) {
        if let Some(num_str) = cap.get(1) {
            if let Ok(num) = num_str.as_str().parse::<usize>() {
                if num == 0 {
                    continue;
                }
                let idx = num.saturating_sub(1); // [1] -> index 0
                if idx < search_results.len() && !seen.contains(&idx) {
                    seen.insert(idx);
                    let result = &search_results[idx];
                    let title = source_titles
                        .get(&result.chunk.source_id)
                        .cloned()
                        .unwrap_or_else(|| "Unknown".to_string());

                    // Extract a short quote from the chunk
                    let quote = result.chunk.content.chars().take(200).collect::<String>();

                    citations.push(Citation {
                        chunk_id: result.chunk.id.clone(),
                        source_id: result.chunk.source_id.clone(),
                        source_title: title,
                        quote: Some(quote),
                        page: None,
                        section: None,
                    });
                }
            }
        }
    }

    citations
}

/// Extract citations from LLM response using source_context (title, content) pairs.
/// This matches the [1], [2] ordering in the system prompt exactly.
#[allow(dead_code)]
pub fn extract_citations_from_context(
    response: &str,
    source_context: &[ContextPassage],
) -> Vec<Citation> {
    extract_citations_from_context_with_reasons(response, source_context).0
}

pub fn extract_citations_from_context_with_reasons(
    response: &str,
    source_context: &[ContextPassage],
) -> (Vec<Citation>, Vec<CitationFilterReasonV1>) {
    let re = Regex::new(r"\[(\d+)\]").unwrap();
    let mut citations = Vec::new();
    let mut filtered = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for cap in re.captures_iter(response) {
        if let Some(num_str) = cap.get(1) {
            if let Ok(num) = num_str.as_str().parse::<usize>() {
                if num == 0 {
                    filtered.push(CitationFilterReasonV1 {
                        ref_number: num,
                        reason_code: "missing_ref".to_string(),
                        detail: "citation refs are 1-based; [0] is invalid".to_string(),
                    });
                    continue;
                }
                let idx = num.saturating_sub(1);
                if idx >= source_context.len() {
                    filtered.push(CitationFilterReasonV1 {
                        ref_number: num,
                        reason_code: "out_of_range".to_string(),
                        detail: format!("citation [{num}] has no matching context passage"),
                    });
                    continue;
                }
                if seen.contains(&idx) {
                    continue;
                }
                let passage = &source_context[idx];
                let Some(chunk_id) = passage.chunk_id.as_ref() else {
                    filtered.push(CitationFilterReasonV1 {
                        ref_number: num,
                        reason_code: "unanchored_chunk".to_string(),
                        detail: format!(
                            "citation [{num}] points at context without chunk identity"
                        ),
                    });
                    continue;
                };
                seen.insert(idx);
                let quote: String = passage.content.chars().take(200).collect();

                citations.push(Citation {
                    chunk_id: chunk_id.clone(),
                    source_id: passage.source_id.clone(),
                    source_title: passage.title.clone(),
                    quote: Some(quote),
                    page: None,
                    section: None,
                });
            }
        }
    }

    (citations, filtered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notebook_db::Chunk;

    fn make_result(id: &str, source_id: &str, content: &str) -> SearchResult {
        SearchResult {
            chunk: Chunk {
                id: id.to_string(),
                source_id: source_id.to_string(),
                chunk_index: 0,
                content: content.to_string(),
                token_count: None,
                start_offset: None,
                end_offset: None,
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            },
            score: 1.0,
        }
    }

    #[test]
    fn test_extract_citations() {
        let results = vec![
            make_result("c1", "s1", "Rust is a systems language"),
            make_result("c2", "s2", "Python is interpreted"),
        ];
        let mut titles = std::collections::HashMap::new();
        titles.insert("s1".to_string(), "Rust Book".to_string());
        titles.insert("s2".to_string(), "Python Guide".to_string());

        let response = "Rust is fast [1] and Python is easy [2].";
        let citations = extract_citations(response, &results, &titles);

        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0].source_title, "Rust Book");
        assert_eq!(citations[1].source_title, "Python Guide");
    }

    #[test]
    fn test_no_citations() {
        let results = vec![make_result("c1", "s1", "content")];
        let titles = std::collections::HashMap::new();
        let response = "No citations here.";
        let citations = extract_citations(response, &results, &titles);
        assert!(citations.is_empty());
    }

    #[test]
    fn test_out_of_range_citation() {
        let results = vec![make_result("c1", "s1", "content")];
        let titles = std::collections::HashMap::new();
        let response = "Reference [5] is invalid.";
        let citations = extract_citations(response, &results, &titles);
        assert!(citations.is_empty());
    }

    #[test]
    fn zero_citation_is_invalid_and_never_anchored() {
        let results = vec![make_result("c1", "s1", "content")];
        let titles = std::collections::HashMap::new();
        assert!(
            extract_citations("[0] should not cite first result", &results, &titles).is_empty()
        );

        let context = vec![ContextPassage {
            source_id: "s1".to_string(),
            chunk_id: Some("c1".to_string()),
            title: "Source One".to_string(),
            content: "anchored evidence".to_string(),
            evidence_class: "ranked_bm25".to_string(),
        }];
        assert!(
            extract_citations_from_context("[0] should not cite first passage", &context)
                .is_empty()
        );
        assert_eq!(count_unique_citation_refs("[0] plus [1]"), 2);
    }

    #[test]
    fn counts_unique_citation_references_for_invalid_citation_metrics() {
        assert_eq!(
            count_unique_citation_refs("Use [1], [2], [2], and [99]."),
            3
        );
        assert_eq!(count_unique_citation_refs("No numbered citations."), 0);
    }

    #[test]
    fn context_citations_require_valid_chunk_identity() {
        let context = vec![
            ContextPassage {
                source_id: "s1".to_string(),
                chunk_id: Some("c1".to_string()),
                title: "Source One".to_string(),
                content: "anchored evidence".to_string(),
                evidence_class: "ranked_bm25".to_string(),
            },
            ContextPassage {
                source_id: "s2".to_string(),
                chunk_id: None,
                title: "Raw Source".to_string(),
                content: "raw fallback without chunk identity".to_string(),
                evidence_class: "raw_fallback".to_string(),
            },
        ];

        let citations = extract_citations_from_context("Use [1] but ignore [2].", &context);

        assert_eq!(citations.len(), 1);
        assert_eq!(citations[0].source_id, "s1");
        assert_eq!(citations[0].chunk_id, "c1");
    }
}
