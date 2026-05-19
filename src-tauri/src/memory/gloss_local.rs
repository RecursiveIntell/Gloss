use crate::db::notebook_db::{NotebookDb, Source};
use crate::error::GlossError;
use crate::memory::backend::{excluded_source_count, invalid_requested_source_ids};
use crate::memory::backend::{requested_source_ids, scope_echo, MemorySearchBackend};
use crate::memory::types::{
    IndexSourceReceipt, IndexSourceRequest, MemoryBackendStatus, MemorySearchCandidate,
    MemorySearchRequest, MemorySearchResponse, MEMORY_BACKEND_GLOSS_LOCAL,
};
use std::collections::HashMap;

pub struct GlossLocalMemoryBackend<'a> {
    notebook_id: String,
    nb_db: &'a NotebookDb,
    all_sources: &'a [Source],
}

impl<'a> GlossLocalMemoryBackend<'a> {
    pub fn new(notebook_id: String, nb_db: &'a NotebookDb, all_sources: &'a [Source]) -> Self {
        Self {
            notebook_id,
            nb_db,
            all_sources,
        }
    }
}

fn sanitize_fts_query(query: &str) -> String {
    query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter_map(|word| {
            let sanitized = word
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect::<String>();
            if sanitized.is_empty() {
                None
            } else {
                Some(sanitized)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl MemorySearchBackend for GlossLocalMemoryBackend<'_> {
    fn backend_id(&self) -> &'static str {
        MEMORY_BACKEND_GLOSS_LOCAL
    }

    fn backend_status(&self) -> MemoryBackendStatus {
        MemoryBackendStatus {
            backend_id: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            default_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            active_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            available: true,
            semantic_memory_feature_enabled: cfg!(feature = "semantic-memory-backend"),
            semantic_memory_available: cfg!(feature = "semantic-memory-backend"),
            semantic_memory_path: Some("src-tauri/vendor/semantic-memory".to_string()),
            index_sync_status: "local".to_string(),
            sync_status: "local".to_string(),
            last_sync_at: None,
            last_sync_error: None,
            last_retrieval_receipt_id: None,
            last_receipt_ref: None,
            fallback_reason: None,
            degradation_markers: Vec::new(),
            backend_version_or_digest: None,
            degraded: false,
            diagnostic: None,
        }
    }

    fn index_source(&self, request: IndexSourceRequest) -> Result<IndexSourceReceipt, GlossError> {
        Ok(IndexSourceReceipt {
            backend_id: self.backend_id().to_string(),
            notebook_id: request.notebook_id,
            source_id: request.source_id,
            receipt_id: request
                .trace_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            indexed_chunks: 0,
            sync_status: "local-backend-noop".to_string(),
            error: None,
            vector_artifact_receipt: None,
        })
    }

    fn search(&self, request: MemorySearchRequest) -> Result<MemorySearchResponse, GlossError> {
        let resolved_scope = request.source_scope.resolve(self.all_sources);
        let requested_ids = requested_source_ids(&request);
        let invalid_source_ids = invalid_requested_source_ids(&requested_ids, &resolved_scope);
        let excluded_source_count = excluded_source_count(self.all_sources, &resolved_scope);
        let title_map = self
            .all_sources
            .iter()
            .map(|source| (source.id.clone(), source.title.clone()))
            .collect::<HashMap<_, _>>();
        let mut candidates = Vec::new();
        let mut fallback_reason = None;
        let mut degradation_markers = Vec::new();
        let mut fallback_used = false;
        let mut retrieval_mode = "gloss-local-fts5-bm25";

        if !resolved_scope.is_none() {
            let fts_query = sanitize_fts_query(&request.query);
            if fts_query.is_empty() {
                fallback_reason = Some("local FTS5 query sanitized to empty".to_string());
            } else {
                match self.nb_db.fts_search_chunks_in_sources(
                    &fts_query,
                    resolved_scope.source_ids(),
                    request.limit,
                ) {
                    Ok(results) => {
                        for (chunk, rank) in results {
                            candidates.push(MemorySearchCandidate {
                                chunk_id: chunk.id.clone(),
                                source_id: chunk.source_id.clone(),
                                notebook_id: Some(self.notebook_id.clone()),
                                source_title: title_map.get(&chunk.source_id).cloned(),
                                citation_anchor: Some(format!("{}#{}", chunk.source_id, chunk.id)),
                                content: chunk.content,
                                score: -rank,
                                backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
                                receipt_ref: None,
                                degradation: Vec::new(),
                            });
                        }
                        if candidates.is_empty() {
                            fallback_reason =
                                Some("local FTS5/BM25 returned no matching chunks".to_string());
                        }
                    }
                    Err(err) => {
                        fallback_reason = Some(format!("local FTS5/BM25 failed: {err}"));
                    }
                }
            }

            if candidates.is_empty() && fallback_reason.is_some() {
                degradation_markers.push("gloss-local-fts5-bm25-degraded".to_string());
                if request.allow_fallback {
                    fallback_used = true;
                    retrieval_mode = "gloss-local-degraded-source-order";
                    for source_id in resolved_scope.source_ids() {
                        for chunk in self.nb_db.get_chunks_for_source(source_id)? {
                            if candidates.len() >= request.limit {
                                break;
                            }
                            candidates.push(MemorySearchCandidate {
                                chunk_id: chunk.id.clone(),
                                source_id: chunk.source_id.clone(),
                                notebook_id: Some(self.notebook_id.clone()),
                                source_title: title_map.get(&chunk.source_id).cloned(),
                                citation_anchor: Some(format!("{}#{}", chunk.source_id, chunk.id)),
                                content: chunk.content,
                                score: 0.0,
                                backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
                                receipt_ref: None,
                                degradation: vec!["gloss-local-degraded-source-order".to_string()],
                            });
                        }
                        if candidates.len() >= request.limit {
                            break;
                        }
                    }
                }
            }
        }

        let receipt_id = request
            .trace_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let degraded = !degradation_markers.is_empty();

        Ok(MemorySearchResponse {
            backend_id: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_requested: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            source_scope_mode: scope_echo(requested_ids.clone(), &resolved_scope).mode,
            selected_source_ids: resolved_scope.source_ids().to_vec(),
            invalid_source_ids,
            excluded_source_count,
            scope: scope_echo(requested_ids, &resolved_scope),
            candidates,
            receipt_id: receipt_id.clone(),
            provenance: serde_json::json!({
                "receipt_id": receipt_id,
                "notebook_id": self.notebook_id,
                "backend": MEMORY_BACKEND_GLOSS_LOCAL,
                "retrieval_mode": retrieval_mode,
                "fallback_used": fallback_used,
                "fallback_reason": fallback_reason.clone(),
                "degradation_markers": degradation_markers.clone(),
                "source_scope_preserved": true
            }),
            fallback_reason,
            degradation_markers,
            source_scope_preserved: true,
            fallback_used,
            degraded,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notebook_db::{Chunk, NotebookDb};
    use crate::retrieval::source_scope::SourceScope;
    use tempfile::tempdir;

    fn source(id: &str, title: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: title.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: None,
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn chunk(id: &str, source_id: &str, idx: i32, content: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_id: source_id.to_string(),
            chunk_index: idx,
            content: content.to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        }
    }

    fn test_db() -> NotebookDb {
        let dir = tempdir().unwrap();
        let path = dir.path().join("notebook.db");
        NotebookDb::open(&path).unwrap()
    }

    #[test]
    fn local_search_returns_late_answer_chunk_before_unrelated_early_chunks() {
        let db = test_db();
        let sources = vec![source("s1", "Scoped Source"), source("s2", "Other Source")];
        for source in &sources {
            db.insert_source(source).unwrap();
        }
        db.insert_chunk(&chunk("early-1", "s1", 0, "alpha beta general overview"))
            .unwrap();
        db.insert_chunk(&chunk("early-2", "s1", 1, "general setup notes"))
            .unwrap();
        db.insert_chunk(&chunk(
            "late-answer",
            "s1",
            99,
            "needleterm needleterm ranked answer lives here",
        ))
        .unwrap();
        db.insert_chunk(&chunk(
            "outside-scope",
            "s2",
            0,
            "needleterm needleterm outside source should be excluded",
        ))
        .unwrap();

        let backend = GlossLocalMemoryBackend::new("nb".to_string(), &db, &sources);
        let response = backend
            .search(MemorySearchRequest {
                notebook_id: "nb".to_string(),
                source_scope: SourceScope::Explicit(vec!["s1".to_string()]),
                query: "needleterm".to_string(),
                limit: 3,
                trace_id: Some("trace-local-ranked".to_string()),
                allow_fallback: true,
            })
            .unwrap();

        assert_eq!(response.backend_used, MEMORY_BACKEND_GLOSS_LOCAL);
        assert!(!response.fallback_used);
        assert!(!response.degraded);
        assert_eq!(
            response.provenance["retrieval_mode"],
            "gloss-local-fts5-bm25"
        );
        assert_eq!(response.candidates[0].chunk_id, "late-answer");
        assert!(response
            .candidates
            .iter()
            .all(|candidate| candidate.source_id == "s1"));
    }

    #[test]
    fn local_search_marks_source_order_fallback_as_degraded() {
        let db = test_db();
        let sources = vec![source("s1", "Scoped Source")];
        db.insert_source(&sources[0]).unwrap();
        db.insert_chunk(&chunk("first", "s1", 0, "fallback content"))
            .unwrap();

        let backend = GlossLocalMemoryBackend::new("nb".to_string(), &db, &sources);
        let response = backend
            .search(MemorySearchRequest {
                notebook_id: "nb".to_string(),
                source_scope: SourceScope::Explicit(vec!["s1".to_string()]),
                query: "!!!".to_string(),
                limit: 3,
                trace_id: Some("trace-local-degraded".to_string()),
                allow_fallback: true,
            })
            .unwrap();

        assert!(response.fallback_used);
        assert!(response.degraded);
        assert_eq!(
            response.provenance["retrieval_mode"],
            "gloss-local-degraded-source-order"
        );
        assert_eq!(response.candidates[0].chunk_id, "first");
        assert!(response
            .degradation_markers
            .contains(&"gloss-local-fts5-bm25-degraded".to_string()));
    }

    #[test]
    fn local_search_handles_hyphenated_queries_without_degrading() {
        let db = test_db();
        let sources = vec![source("s1", "Scoped Source")];
        db.insert_source(&sources[0]).unwrap();
        db.insert_chunk(&chunk("hyphen", "s1", 0, "alpha beta release note"))
            .unwrap();

        let backend = GlossLocalMemoryBackend::new("nb".to_string(), &db, &sources);
        let response = backend
            .search(MemorySearchRequest {
                notebook_id: "nb".to_string(),
                source_scope: SourceScope::Explicit(vec!["s1".to_string()]),
                query: "alpha-beta".to_string(),
                limit: 3,
                trace_id: Some("trace-local-hyphen".to_string()),
                allow_fallback: true,
            })
            .unwrap();

        assert!(!response.fallback_used);
        assert!(!response.degraded);
        assert_eq!(response.candidates[0].chunk_id, "hyphen");
        assert_eq!(
            response.provenance["retrieval_mode"],
            "gloss-local-fts5-bm25"
        );
    }
}
