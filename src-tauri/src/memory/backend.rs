use crate::db::notebook_db::Source;
use crate::error::GlossError;
use crate::memory::types::{
    IndexSourceReceipt, IndexSourceRequest, MemoryBackendComparison, MemoryBackendStatus,
    MemoryScopeEcho, MemorySearchCandidate, MemorySearchRequest, MemorySearchResponse,
    SemanticCandidateEnvelope, SemanticLinkRow, MEMORY_BACKEND_GLOSS_LOCAL,
    MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
};
use crate::retrieval::source_scope::{ResolvedSourceScope, ResolvedSourceScopeKind};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

#[allow(dead_code)]
pub trait MemorySearchBackend {
    fn backend_id(&self) -> &'static str;
    fn backend_status(&self) -> MemoryBackendStatus;
    fn index_source(&self, request: IndexSourceRequest) -> Result<IndexSourceReceipt, GlossError>;
    fn search(&self, request: MemorySearchRequest) -> Result<MemorySearchResponse, GlossError>;
}

pub fn scope_echo(
    requested_ids: Vec<String>,
    resolved_scope: &ResolvedSourceScope,
) -> MemoryScopeEcho {
    let mode = match resolved_scope.kind() {
        ResolvedSourceScopeKind::All => "all",
        ResolvedSourceScopeKind::Explicit => "explicit",
        ResolvedSourceScopeKind::None => "none",
    };
    MemoryScopeEcho {
        mode: mode.to_string(),
        requested_source_ids: requested_ids,
        effective_source_ids: resolved_scope.source_ids().to_vec(),
    }
}

pub fn invalid_requested_source_ids(
    requested_ids: &[String],
    resolved_scope: &ResolvedSourceScope,
) -> Vec<String> {
    if requested_ids.is_empty() {
        return Vec::new();
    }

    if resolved_scope.kind() == ResolvedSourceScopeKind::None {
        return requested_ids
            .iter()
            .filter(|id| !id.trim().is_empty())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
    }

    if resolved_scope.kind() != ResolvedSourceScopeKind::Explicit {
        return Vec::new();
    }

    requested_ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .filter(|id| !resolved_scope.allows(id))
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

pub fn excluded_source_count(
    all_sources: &[Source],
    resolved_scope: &ResolvedSourceScope,
) -> usize {
    match resolved_scope.kind() {
        ResolvedSourceScopeKind::All => 0,
        ResolvedSourceScopeKind::Explicit | ResolvedSourceScopeKind::None => all_sources
            .iter()
            .filter(|source| !resolved_scope.allows(&source.id))
            .count(),
    }
}

pub fn requested_source_ids(request: &MemorySearchRequest) -> Vec<String> {
    match &request.source_scope {
        crate::retrieval::source_scope::SourceScope::All => Vec::new(),
        crate::retrieval::source_scope::SourceScope::Explicit(ids) => ids.clone(),
        crate::retrieval::source_scope::SourceScope::None => Vec::new(),
    }
}

pub fn filter_semantic_candidates_by_scope(
    candidates: &[SemanticCandidateEnvelope],
    links: &[SemanticLinkRow],
    resolved_scope: &ResolvedSourceScope,
    source_titles: &HashMap<String, String>,
    limit: usize,
) -> (Vec<MemorySearchCandidate>, Vec<String>, Vec<String>) {
    if resolved_scope.is_none() {
        return (Vec::new(), Vec::new(), Vec::new());
    }

    let mut links_by_doc: HashMap<&str, Vec<&SemanticLinkRow>> = HashMap::new();
    for link in links {
        if link.sync_status != "synced" {
            continue;
        }
        let Some(doc_id) = link
            .sm_document_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        let Some(_chunk_id) = link
            .sm_chunk_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
        else {
            continue;
        };
        if link.content_digest.trim().is_empty() {
            continue;
        }
        links_by_doc.entry(doc_id).or_default().push(link);
    }

    let mut out = Vec::new();
    let mut source_scope_violations = Vec::new();
    let mut unmapped_semantic_candidates = Vec::new();
    let mut seen_chunks = HashSet::new();

    for candidate in candidates {
        let Some(doc_links) = links_by_doc.get(candidate.sm_document_id.as_str()) else {
            unmapped_semantic_candidates.push(format!(
                "semantic candidate document {} has no synced Gloss link",
                candidate.sm_document_id
            ));
            continue;
        };

        if doc_links
            .iter()
            .all(|link| !resolved_scope.allows(&link.source_id))
        {
            source_scope_violations.push(format!(
                "semantic candidate document {} only matched sources outside selected scope",
                candidate.sm_document_id
            ));
            continue;
        }

        let candidate_digest = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(candidate.content.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let matching_links = doc_links
            .iter()
            .copied()
            .filter(|link| {
                let exact_chunk = candidate
                    .sm_chunk_id
                    .as_deref()
                    .is_some_and(|chunk_id| link.sm_chunk_id.as_deref() == Some(chunk_id));
                let exact_digest = link.content_digest == candidate_digest;
                exact_chunk
                    && exact_digest
                    && resolved_scope.allows(&link.source_id)
                    && !seen_chunks.contains(&link.chunk_id)
            })
            .collect::<Vec<_>>();

        if matching_links.len() > 1 {
            unmapped_semantic_candidates.push(format!(
                "semantic candidate document {} matched multiple exact Gloss chunk links",
                candidate.sm_document_id
            ));
            continue;
        }
        let matching_link = matching_links.into_iter().next();

        let Some(link) = matching_link else {
            unmapped_semantic_candidates.push(format!(
                "semantic candidate document {} had no exact Gloss chunk link",
                candidate.sm_document_id
            ));
            continue;
        };

        if !resolved_scope.allows(&link.source_id) {
            source_scope_violations.push(format!(
                "semantic candidate chunk {} source {} outside selected scope",
                link.chunk_id, link.source_id
            ));
            continue;
        }

        if !seen_chunks.insert(link.chunk_id.clone()) {
            continue;
        }

        out.push(MemorySearchCandidate {
            chunk_id: link.chunk_id.clone(),
            source_id: link.source_id.clone(),
            notebook_id: None,
            source_title: source_titles.get(&link.source_id).cloned(),
            citation_anchor: Some(format!("{}#{}", link.source_id, link.chunk_id)),
            content: candidate.content.clone(),
            score: candidate.score,
            backend: crate::memory::types::MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            receipt_ref: None,
            degradation: Vec::new(),
        });

        if out.len() >= limit {
            break;
        }
    }

    (out, source_scope_violations, unmapped_semantic_candidates)
}

pub fn chunk_id_intersection(
    left: &[MemorySearchCandidate],
    right: &[MemorySearchCandidate],
) -> Vec<String> {
    let right_chunks = right
        .iter()
        .map(|candidate| candidate.chunk_id.as_str())
        .collect::<HashSet<_>>();
    left.iter()
        .filter(|candidate| right_chunks.contains(candidate.chunk_id.as_str()))
        .map(|candidate| candidate.chunk_id.clone())
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn compare_memory_backends_for_notebook(
    data_dir: &Path,
    notebook_id: &str,
    all_sources: &[Source],
    request: MemorySearchRequest,
    local_backend_result: MemorySearchResponse,
    local_latency_ms: u128,
    semantic_links: Vec<SemanticLinkRow>,
    #[cfg(feature = "semantic-memory-backend")] semantic_runtime_config: Option<
        crate::memory::semantic_memory_adapter::SemanticMemoryRuntimeConfig,
    >,
) -> Result<MemoryBackendComparison, GlossError> {
    let semantic_started = Instant::now();
    #[cfg(feature = "semantic-memory-backend")]
    let semantic_memory_result = semantic_memory_comparison_result(
        data_dir,
        notebook_id,
        semantic_links,
        all_sources,
        request.clone(),
        semantic_runtime_config,
    )
    .await?;
    #[cfg(not(feature = "semantic-memory-backend"))]
    let semantic_memory_result = semantic_memory_comparison_result(
        data_dir,
        notebook_id,
        semantic_links,
        all_sources,
        request.clone(),
    )
    .await?;
    let semantic_memory_latency_ms = Some(semantic_started.elapsed().as_millis());
    let intersection_by_chunk_id = chunk_id_intersection(
        &local_backend_result.candidates,
        &semantic_memory_result.candidates,
    );
    let (source_scope_violations, unmapped_semantic_candidates) =
        semantic_memory_comparison_findings(&semantic_memory_result);
    let receipt_ids = comparison_receipt_ids(&local_backend_result, &semantic_memory_result);
    let decision = comparison_decision(
        &semantic_memory_result,
        &intersection_by_chunk_id,
        &source_scope_violations,
        &unmapped_semantic_candidates,
    );

    Ok(MemoryBackendComparison {
        local_backend_result,
        semantic_memory_result: Some(semantic_memory_result),
        intersection_by_chunk_id,
        source_scope_violations,
        unmapped_semantic_candidates,
        local_latency_ms,
        semantic_memory_latency_ms,
        receipt_ids,
        decision,
    })
}

async fn semantic_memory_comparison_result(
    data_dir: &Path,
    notebook_id: &str,
    semantic_links: Vec<SemanticLinkRow>,
    all_sources: &[Source],
    request: MemorySearchRequest,
    #[cfg(feature = "semantic-memory-backend")] semantic_runtime_config: Option<
        crate::memory::semantic_memory_adapter::SemanticMemoryRuntimeConfig,
    >,
) -> Result<MemorySearchResponse, GlossError> {
    #[cfg(feature = "semantic-memory-backend")]
    {
        return match crate::memory::semantic_memory_adapter::search_preview(
            data_dir,
            notebook_id,
            semantic_links,
            all_sources,
            request.clone(),
            semantic_runtime_config,
        )
        .await
        {
            Ok(response) => Ok(response),
            Err(err) => Ok(semantic_memory_unavailable_response(
                all_sources,
                request,
                format!("semantic-memory preview unavailable: {err}"),
            )),
        };
    }

    #[cfg(not(feature = "semantic-memory-backend"))]
    {
        let _ = data_dir;
        let _ = semantic_links;
        let _ = notebook_id;
        Ok(semantic_memory_unavailable_response(
            all_sources,
            request,
            "semantic-memory-backend feature is not enabled".to_string(),
        ))
    }
}

fn semantic_memory_unavailable_response(
    all_sources: &[Source],
    request: MemorySearchRequest,
    reason: String,
) -> MemorySearchResponse {
    let resolved_scope = request.source_scope.resolve(all_sources);
    let requested_ids = requested_source_ids(&request);
    let invalid_source_ids = invalid_requested_source_ids(&requested_ids, &resolved_scope);
    let excluded_source_count = excluded_source_count(all_sources, &resolved_scope);
    let scope = scope_echo(requested_ids, &resolved_scope);
    let source_scope_preserved = invalid_source_ids.is_empty();
    let receipt_id = request
        .trace_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    MemorySearchResponse {
        backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        backend_requested: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        backend_used: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
        source_scope_mode: scope.mode.clone(),
        selected_source_ids: resolved_scope.source_ids().to_vec(),
        invalid_source_ids,
        excluded_source_count,
        scope,
        candidates: Vec::new(),
        receipt_id: receipt_id.clone(),
        provenance: serde_json::json!({
            "receipt_id": receipt_id,
            "backend": MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW,
            "source_scope_violations": [],
            "unmapped_semantic_candidates": [reason.clone()],
            "fallback_used": false,
            "degraded": true,
            "source_scope_preserved": source_scope_preserved
        }),
        fallback_reason: Some(reason),
        degradation_markers: vec!["semantic-memory-unavailable".to_string()],
        source_scope_preserved,
        fallback_used: false,
        degraded: true,
    }
}

fn semantic_memory_comparison_findings(
    response: &MemorySearchResponse,
) -> (Vec<String>, Vec<String>) {
    let source_scope_violations = response
        .provenance
        .get("source_scope_violations")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut unmapped_semantic_candidates = response
        .provenance
        .get("unmapped_semantic_candidates")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if response.degraded {
        unmapped_semantic_candidates.extend(response.degradation_markers.iter().cloned());
        if let Some(reason) = &response.fallback_reason {
            unmapped_semantic_candidates.push(reason.clone());
        }
    }
    (source_scope_violations, unmapped_semantic_candidates)
}

fn comparison_receipt_ids(
    local_backend_result: &MemorySearchResponse,
    semantic_memory_result: &MemorySearchResponse,
) -> Vec<String> {
    let mut receipt_ids = Vec::new();
    for receipt_id in [
        local_backend_result.receipt_id.as_str(),
        semantic_memory_result.receipt_id.as_str(),
    ] {
        if !receipt_ids.iter().any(|existing| existing == receipt_id) {
            receipt_ids.push(receipt_id.to_string());
        }
    }
    if let Some(receipt) = semantic_memory_result
        .provenance
        .get("semantic_memory_receipt")
    {
        let receipt_ref = if let Some(receipt_id) =
            receipt.get("receipt_id").and_then(|value| value.as_str())
        {
            receipt_id.to_string()
        } else if let Some(request_id) = receipt.get("request_id").and_then(|value| value.as_str())
        {
            request_id.to_string()
        } else {
            receipt.to_string()
        };
        if !receipt_ids.iter().any(|existing| existing == &receipt_ref) {
            receipt_ids.push(receipt_ref);
        }
    }
    receipt_ids
}

fn comparison_decision(
    semantic_memory_result: &MemorySearchResponse,
    intersection_by_chunk_id: &[String],
    source_scope_violations: &[String],
    unmapped_semantic_candidates: &[String],
) -> String {
    if !source_scope_violations.is_empty() {
        return "blocked-source-scope-violation".to_string();
    }
    if semantic_memory_result.degraded || !unmapped_semantic_candidates.is_empty() {
        return "degraded-semantic-memory".to_string();
    }
    if intersection_by_chunk_id.is_empty() {
        return "no-overlap".to_string();
    }
    "pass".to_string()
}

#[allow(dead_code)]
pub fn unavailable_status(active_backend: String, diagnostic: String) -> MemoryBackendStatus {
    MemoryBackendStatus {
        backend_id: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
        default_backend: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
        active_backend,
        backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
        available: true,
        semantic_memory_feature_enabled: cfg!(feature = "semantic-memory-backend"),
        semantic_memory_available: false,
        semantic_memory_path: Some("src-tauri/vendor/semantic-memory".to_string()),
        index_sync_status: "unavailable".to_string(),
        sync_status: "unavailable".to_string(),
        last_sync_at: None,
        last_sync_error: None,
        last_retrieval_receipt_id: None,
        last_receipt_ref: None,
        fallback_reason: Some(diagnostic.clone()),
        degradation_markers: vec!["semantic-memory-unavailable".to_string()],
        backend_version_or_digest: None,
        degraded: true,
        diagnostic: Some(diagnostic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "semantic-memory-backend"))]
    use crate::db::notebook_db::NotebookDb;
    use crate::db::notebook_db::Source;
    #[cfg(not(feature = "semantic-memory-backend"))]
    use crate::memory::gloss_local::GlossLocalMemoryBackend;
    use crate::retrieval::source_scope::SourceScope;
    #[cfg(not(feature = "semantic-memory-backend"))]
    use tempfile::tempdir;

    fn source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: id.to_string(),
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
            processing_state: None,
        }
    }

    #[cfg(not(feature = "semantic-memory-backend"))]
    #[tokio::test]
    async fn backend_comparison_returns_truthful_semantic_result_when_unavailable() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("notebook.db");
        let nb_db = NotebookDb::open(&db_path).unwrap();
        let all_sources = Vec::new();
        let request = MemorySearchRequest {
            notebook_id: "nb".to_string(),
            source_scope: SourceScope::All,
            query: "query".to_string(),
            limit: 5,
            trace_id: Some("trace-parity".to_string()),
            allow_fallback: false,
        };
        let local = GlossLocalMemoryBackend::new("nb".to_string(), &nb_db, &all_sources);
        let local_started = Instant::now();
        let local_backend_result = local.search(request.clone()).unwrap();
        let local_latency_ms = local_started.elapsed().as_millis();

        let comparison = compare_memory_backends_for_notebook(
            dir.path(),
            "nb",
            &all_sources,
            request,
            local_backend_result,
            local_latency_ms,
            Vec::new(),
        )
        .await
        .unwrap();

        let semantic = comparison
            .semantic_memory_result
            .as_ref()
            .expect("semantic comparison result should be explicit");
        assert_eq!(
            semantic.backend_requested,
            MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        );
        assert!(semantic.degraded);
        assert!(semantic
            .degradation_markers
            .contains(&"semantic-memory-unavailable".to_string()));
        assert!(comparison.semantic_memory_latency_ms.is_some());
        assert!(comparison.receipt_ids.contains(&semantic.receipt_id));
        assert!(!comparison.unmapped_semantic_candidates.is_empty());
        assert_eq!(comparison.decision, "degraded-semantic-memory");
    }

    #[cfg(feature = "semantic-memory-backend")]
    #[tokio::test]
    async fn backend_comparison_executes_feature_enabled_semantic_preview_path() {
        let all_sources = Vec::new();
        let request = MemorySearchRequest {
            notebook_id: "nb".to_string(),
            source_scope: SourceScope::None,
            query: "query".to_string(),
            limit: 5,
            trace_id: Some("trace-feature-parity".to_string()),
            allow_fallback: false,
        };
        let local_backend_result = MemorySearchResponse {
            backend_id: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_requested: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            source_scope_mode: "none".to_string(),
            selected_source_ids: Vec::new(),
            invalid_source_ids: Vec::new(),
            excluded_source_count: 0,
            scope: scope_echo(Vec::new(), &SourceScope::None.resolve(&all_sources)),
            candidates: Vec::new(),
            receipt_id: "local-feature-receipt".to_string(),
            provenance: serde_json::json!({"backend": MEMORY_BACKEND_GLOSS_LOCAL}),
            fallback_reason: None,
            degradation_markers: Vec::new(),
            source_scope_preserved: true,
            fallback_used: false,
            degraded: false,
        };

        let comparison = compare_memory_backends_for_notebook(
            std::path::Path::new("/tmp/gloss-feature-parity"),
            "nb",
            &all_sources,
            request,
            local_backend_result,
            0,
            Vec::new(),
            None,
        )
        .await
        .unwrap();

        let semantic = comparison
            .semantic_memory_result
            .as_ref()
            .expect("feature-enabled path should return semantic preview response");
        assert_eq!(
            semantic.backend_requested,
            MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW
        );
        assert_eq!(semantic.source_scope_mode, "none");
        assert!(!semantic.degraded);
        assert_eq!(comparison.decision, "no-overlap");
    }

    #[test]
    fn invalid_requested_ids_reports_all_invalid_when_explicit_scope_resolves_none() {
        let all_sources = vec![source("a"), source("b")];
        let requested = vec!["missing-a".to_string(), "missing-b".to_string()];
        let resolved = crate::retrieval::source_scope::SourceScope::Explicit(requested.clone())
            .resolve(&all_sources);

        let invalid = invalid_requested_source_ids(&requested, &resolved);

        assert_eq!(invalid.len(), 2);
        assert!(invalid.contains(&"missing-a".to_string()));
        assert!(invalid.contains(&"missing-b".to_string()));
    }

    fn link(chunk_id: &str, source_id: &str, doc_id: &str, status: &str) -> SemanticLinkRow {
        let content = format!("content for {doc_id}");
        let content_digest = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        SemanticLinkRow {
            chunk_id: chunk_id.to_string(),
            source_id: source_id.to_string(),
            sm_document_id: Some(doc_id.to_string()),
            sm_chunk_id: Some(doc_id.to_string()),
            content_digest,
            sync_status: status.to_string(),
        }
    }

    fn candidate(doc_id: &str, _content: &str) -> SemanticCandidateEnvelope {
        SemanticCandidateEnvelope {
            sm_document_id: doc_id.to_string(),
            sm_chunk_id: Some(doc_id.to_string()),
            content: format!("content for {doc_id}"),
            score: 1.0,
        }
    }

    fn scored_candidate(doc_id: &str, score: f64) -> SemanticCandidateEnvelope {
        SemanticCandidateEnvelope {
            sm_document_id: doc_id.to_string(),
            sm_chunk_id: Some(doc_id.to_string()),
            content: format!("content for {doc_id}"),
            score,
        }
    }

    #[test]
    fn hostile_scope_fixtures_fail_closed_for_semantic_candidates() {
        let all_sources = vec![source("selected"), source("other")];
        let titles = all_sources
            .iter()
            .map(|s| (s.id.clone(), s.title.clone()))
            .collect::<HashMap<_, _>>();
        let links = vec![
            link("c-selected", "selected", "doc-selected", "synced"),
            link("c-other", "other", "doc-other", "synced"),
            link("c-stale", "selected", "doc-stale", "stale"),
        ];
        let candidates = vec![
            candidate("doc-selected", "selected content"),
            candidate("doc-other", "lexically matching outside selected source"),
            candidate("doc-stale", "stale content"),
            candidate("doc-missing-link", "missing link"),
        ];

        let fixtures = vec![
            (
                SourceScope::Explicit(vec!["selected".to_string()]),
                vec!["c-selected"],
            ),
            (SourceScope::All, vec!["c-selected", "c-other"]),
            (SourceScope::None, vec![]),
            (SourceScope::Explicit(vec!["deleted".to_string()]), vec![]),
            (
                SourceScope::Explicit(vec!["other-notebook".to_string()]),
                vec![],
            ),
            (
                SourceScope::Explicit(vec!["selected".to_string(), "selected".to_string()]),
                vec!["c-selected"],
            ),
            (SourceScope::Explicit(vec!["".to_string()]), vec![]),
            (
                SourceScope::Explicit(vec!["00000000-0000-0000-0000-000000000000".to_string()]),
                vec![],
            ),
            (
                SourceScope::Explicit(vec!["selected".to_string(), "missing".to_string()]),
                vec!["c-selected"],
            ),
            (
                SourceScope::Explicit(vec!["selected'); DROP TABLE chunks; --".to_string()]),
                vec![],
            ),
        ];

        for (scope, expected_chunks) in fixtures {
            let resolved = scope.resolve(&all_sources);
            let (filtered, _source_scope_violations, _unmapped) =
                filter_semantic_candidates_by_scope(&candidates, &links, &resolved, &titles, 10);
            let actual = filtered
                .iter()
                .map(|candidate| candidate.chunk_id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected_chunks, "scope fixture failed: {scope:?}");
        }
    }

    #[test]
    fn citation_source_outside_scope_and_missing_or_stale_links_do_not_widen() {
        let all_sources = vec![source("selected"), source("outside")];
        let titles = HashMap::new();
        let resolved = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&all_sources);
        let links = vec![
            link("outside-citation", "outside", "doc-outside", "synced"),
            link("stale-selected", "selected", "doc-stale", "stale"),
        ];
        let candidates = vec![
            candidate("doc-outside", "citation source outside selected scope"),
            candidate("doc-stale", "semantic-memory indexed stale chunk"),
            candidate("doc-missing", "semantic-memory missing chunk link"),
        ];

        let (filtered, source_scope_violations, unmapped) =
            filter_semantic_candidates_by_scope(&candidates, &links, &resolved, &titles, 10);

        assert!(filtered.is_empty());
        assert!(!source_scope_violations.is_empty());
        assert!(!unmapped.is_empty());
    }

    #[test]
    fn deterministic_large_corpus_scope_parity_and_ranking_order() {
        let all_sources = (0..120)
            .map(|idx| source(&format!("source-{idx:03}")))
            .collect::<Vec<_>>();
        let titles = all_sources
            .iter()
            .map(|s| (s.id.clone(), s.title.clone()))
            .collect::<HashMap<_, _>>();
        let links = all_sources
            .iter()
            .enumerate()
            .map(|(idx, source)| {
                link(
                    &format!("chunk-{idx:03}"),
                    &source.id,
                    &format!("doc-{idx:03}"),
                    "synced",
                )
            })
            .collect::<Vec<_>>();
        let candidates = (0..120)
            .rev()
            .map(|idx| scored_candidate(&format!("doc-{idx:03}"), idx as f64))
            .collect::<Vec<_>>();
        let explicit_ids = vec![
            "source-119".to_string(),
            "source-041".to_string(),
            "source-005".to_string(),
            "source-missing".to_string(),
        ];
        let resolved = SourceScope::Explicit(explicit_ids).resolve(&all_sources);

        let (filtered, source_scope_violations, unmapped) =
            filter_semantic_candidates_by_scope(&candidates, &links, &resolved, &titles, 10);
        let chunk_ids = filtered
            .iter()
            .map(|candidate| candidate.chunk_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(chunk_ids, vec!["chunk-119", "chunk-041", "chunk-005"]);
        assert!(filtered
            .windows(2)
            .all(|pair| pair[0].score >= pair[1].score));
        assert!(!source_scope_violations.is_empty());
        assert!(unmapped.is_empty());
        assert!(filtered
            .iter()
            .all(|candidate| resolved.allows(&candidate.source_id)));
    }

    #[test]
    fn stale_delete_and_reindex_links_only_accept_synced_backpointers() {
        let all_sources = vec![source("s1")];
        let titles = HashMap::new();
        let resolved = SourceScope::Explicit(vec!["s1".to_string()]).resolve(&all_sources);
        let stale_links = vec![link("old-chunk", "s1", "old-doc", "stale")];
        let stale_candidates = vec![candidate("old-doc", "deleted content")];

        let (filtered_stale, _stale_scope_violations, stale_unmapped) =
            filter_semantic_candidates_by_scope(
                &stale_candidates,
                &stale_links,
                &resolved,
                &titles,
                10,
            );
        assert!(filtered_stale.is_empty());
        assert!(!stale_unmapped.is_empty());

        let reindexed_links = vec![
            link("old-chunk", "s1", "old-doc", "stale"),
            link("new-chunk", "s1", "new-doc", "synced"),
        ];
        let reindexed_candidates = vec![
            scored_candidate("old-doc", 99.0),
            scored_candidate("new-doc", 10.0),
        ];

        let (filtered_reindexed, _reindexed_scope_violations, reindexed_unmapped) =
            filter_semantic_candidates_by_scope(
                &reindexed_candidates,
                &reindexed_links,
                &resolved,
                &titles,
                10,
            );

        assert_eq!(filtered_reindexed.len(), 1);
        assert_eq!(filtered_reindexed[0].chunk_id, "new-chunk");
        assert!(!reindexed_unmapped.is_empty());
    }

    #[test]
    fn semantic_candidates_require_exact_chunk_id_and_digest() {
        let all_sources = vec![source("s1")];
        let titles = HashMap::new();
        let resolved = SourceScope::Explicit(vec!["s1".to_string()]).resolve(&all_sources);
        let links = vec![SemanticLinkRow {
            chunk_id: "gloss-chunk".to_string(),
            source_id: "s1".to_string(),
            sm_document_id: Some("doc".to_string()),
            sm_chunk_id: Some("sm-1".to_string()),
            content_digest: {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update("exact content".as_bytes());
                format!("{:x}", hasher.finalize())
            },
            sync_status: "synced".to_string(),
        }];

        let (exact_match, exact_scope_violations, exact_unmapped) =
            filter_semantic_candidates_by_scope(
                &[SemanticCandidateEnvelope {
                    sm_document_id: "doc".to_string(),
                    sm_chunk_id: Some("sm-1".to_string()),
                    content: "exact content".to_string(),
                    score: 1.0,
                }],
                &links,
                &resolved,
                &titles,
                10,
            );
        assert_eq!(exact_match.len(), 1);
        assert!(exact_scope_violations.is_empty());
        assert!(exact_unmapped.is_empty());

        let (missing_chunk, _missing_chunk_scope_violations, missing_chunk_unmapped) =
            filter_semantic_candidates_by_scope(
                &[SemanticCandidateEnvelope {
                    sm_document_id: "doc".to_string(),
                    sm_chunk_id: None,
                    content: "different content".to_string(),
                    score: 1.0,
                }],
                &links,
                &resolved,
                &titles,
                10,
            );
        assert!(missing_chunk.is_empty());
        assert!(!missing_chunk_unmapped.is_empty());

        let (missing_digest, _missing_digest_scope_violations, missing_digest_unmapped) =
            filter_semantic_candidates_by_scope(
                &[SemanticCandidateEnvelope {
                    sm_document_id: "doc".to_string(),
                    sm_chunk_id: Some("sm-1".to_string()),
                    content: "different content".to_string(),
                    score: 1.0,
                }],
                &links,
                &resolved,
                &titles,
                10,
            );
        assert!(missing_digest.is_empty());
        assert!(!missing_digest_unmapped.is_empty());
    }

    #[test]
    fn ambiguous_exact_semantic_matches_are_suppressed() {
        let all_sources = vec![source("s1")];
        let titles = HashMap::new();
        let resolved = SourceScope::Explicit(vec!["s1".to_string()]).resolve(&all_sources);
        let digest = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update("duplicate content".as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let duplicate_chunk_links = vec![
            SemanticLinkRow {
                chunk_id: "gloss-a".to_string(),
                source_id: "s1".to_string(),
                sm_document_id: Some("doc".to_string()),
                sm_chunk_id: Some("sm-1".to_string()),
                content_digest: digest.clone(),
                sync_status: "synced".to_string(),
            },
            SemanticLinkRow {
                chunk_id: "gloss-b".to_string(),
                source_id: "s1".to_string(),
                sm_document_id: Some("doc".to_string()),
                sm_chunk_id: Some("sm-1".to_string()),
                content_digest: digest,
                sync_status: "synced".to_string(),
            },
        ];

        let (chunk_matches, _chunk_scope_violations, chunk_unmapped) =
            filter_semantic_candidates_by_scope(
                &[SemanticCandidateEnvelope {
                    sm_document_id: "doc".to_string(),
                    sm_chunk_id: Some("sm-1".to_string()),
                    content: "duplicate content".to_string(),
                    score: 1.0,
                }],
                &duplicate_chunk_links,
                &resolved,
                &titles,
                10,
            );
        assert!(chunk_matches.is_empty());
        assert!(chunk_unmapped
            .iter()
            .any(|violation| violation.contains("multiple exact Gloss chunk links")));

        let malformed_links = vec![SemanticLinkRow {
            chunk_id: "gloss-a".to_string(),
            source_id: "s1".to_string(),
            sm_document_id: Some("doc".to_string()),
            sm_chunk_id: None,
            content_digest: {
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update("duplicate content".as_bytes());
                format!("{:x}", hasher.finalize())
            },
            sync_status: "synced".to_string(),
        }];

        let (malformed_matches, _malformed_scope_violations, malformed_unmapped) =
            filter_semantic_candidates_by_scope(
                &[SemanticCandidateEnvelope {
                    sm_document_id: "doc".to_string(),
                    sm_chunk_id: None,
                    content: "duplicate content".to_string(),
                    score: 1.0,
                }],
                &malformed_links,
                &resolved,
                &titles,
                10,
            );
        assert!(malformed_matches.is_empty());
        assert!(!malformed_unmapped.is_empty());
    }

    #[test]
    fn candidate_set_intersection_is_deterministic_for_parity_reports() {
        let left = vec![
            MemorySearchCandidate {
                chunk_id: "a".to_string(),
                source_id: "s1".to_string(),
                notebook_id: Some("nb".to_string()),
                source_title: None,
                citation_anchor: Some("s1#a".to_string()),
                content: "a".to_string(),
                score: 3.0,
                backend: "left".to_string(),
                receipt_ref: None,
                degradation: Vec::new(),
            },
            MemorySearchCandidate {
                chunk_id: "b".to_string(),
                source_id: "s1".to_string(),
                notebook_id: Some("nb".to_string()),
                source_title: None,
                citation_anchor: Some("s1#b".to_string()),
                content: "b".to_string(),
                score: 2.0,
                backend: "left".to_string(),
                receipt_ref: None,
                degradation: Vec::new(),
            },
        ];
        let right = vec![MemorySearchCandidate {
            chunk_id: "b".to_string(),
            source_id: "s1".to_string(),
            notebook_id: Some("nb".to_string()),
            source_title: None,
            citation_anchor: Some("s1#b".to_string()),
            content: "b".to_string(),
            score: 9.0,
            backend: "right".to_string(),
            receipt_ref: None,
            degradation: Vec::new(),
        }];

        assert_eq!(chunk_id_intersection(&left, &right), vec!["b".to_string()]);
    }

    #[test]
    fn comparison_receipt_ids_include_nested_semantic_memory_receipt() {
        let local = MemorySearchResponse {
            backend_id: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_requested: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            backend_used: MEMORY_BACKEND_GLOSS_LOCAL.to_string(),
            source_scope_mode: "all".to_string(),
            selected_source_ids: Vec::new(),
            invalid_source_ids: Vec::new(),
            excluded_source_count: 0,
            scope: MemoryScopeEcho {
                mode: "all".to_string(),
                requested_source_ids: Vec::new(),
                effective_source_ids: Vec::new(),
            },
            candidates: Vec::new(),
            receipt_id: "shared-request-receipt".to_string(),
            provenance: serde_json::json!({}),
            fallback_reason: None,
            degradation_markers: Vec::new(),
            source_scope_preserved: true,
            fallback_used: false,
            degraded: false,
        };
        let semantic = MemorySearchResponse {
            backend_id: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            backend_requested: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            backend_used: MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW.to_string(),
            source_scope_mode: "all".to_string(),
            selected_source_ids: Vec::new(),
            invalid_source_ids: Vec::new(),
            excluded_source_count: 0,
            scope: local.scope.clone(),
            candidates: Vec::new(),
            receipt_id: "shared-request-receipt".to_string(),
            provenance: serde_json::json!({
                "semantic_memory_receipt": {
                    "receipt_id": "semantic-engine-receipt"
                }
            }),
            fallback_reason: None,
            degradation_markers: Vec::new(),
            source_scope_preserved: true,
            fallback_used: false,
            degraded: false,
        };

        assert_eq!(
            comparison_receipt_ids(&local, &semantic),
            vec![
                "shared-request-receipt".to_string(),
                "semantic-engine-receipt".to_string()
            ]
        );
    }
}
