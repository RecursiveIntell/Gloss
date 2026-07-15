use crate::error::MemoryError;
use crate::types::{
    ProjectionClaimVersion, ProjectionEntityAlias, ProjectionEpisode, ProjectionEvidenceRef,
    ProjectionQuery, ProjectionRelationVersion,
};
use stack_ids::{
    ClaimId, ClaimVersionId, EntityId, EnvelopeId, EpisodeId, RelationVersionId, ScopeKey,
};

fn projection_scan_limit(query: &ProjectionQuery) -> usize {
    let base = query.limit.max(1);
    base.saturating_mul(8).min(256).max(base)
}

fn query_terms(text_query: Option<&str>) -> Vec<String> {
    text_query
        .unwrap_or("")
        .split_whitespace()
        .map(|term| term.trim().to_lowercase())
        .filter(|term| !term.is_empty())
        .collect()
}

fn optional_search_token(value: Option<&str>) -> &str {
    value.unwrap_or("")
}

fn text_score(terms: &[String], searchable: &str) -> usize {
    if terms.is_empty() {
        return 1;
    }
    let haystack = searchable.to_lowercase();
    terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count()
}

fn decode_scope_key(
    namespace: String,
    domain: Option<String>,
    workspace_id: Option<String>,
    repo_id: Option<String>,
) -> ScopeKey {
    ScopeKey {
        namespace,
        domain,
        workspace_id,
        repo_id,
    }
}

fn decode_json_value(
    table: &'static str,
    row_id: &str,
    column: &'static str,
    raw: &str,
) -> Result<serde_json::Value, MemoryError> {
    serde_json::from_str(raw).map_err(|err| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid {column} JSON: {err}"),
    })
}

fn decode_optional_json_value(
    table: &'static str,
    row_id: &str,
    column: &'static str,
    raw: Option<String>,
) -> Result<Option<serde_json::Value>, MemoryError> {
    raw.map(|value| decode_json_value(table, row_id, column, &value))
        .transpose()
}

fn decode_string_vec(
    table: &'static str,
    row_id: &str,
    column: &'static str,
    raw: &str,
) -> Result<Vec<String>, MemoryError> {
    serde_json::from_str(raw).map_err(|err| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid {column} JSON: {err}"),
    })
}

/// Query imported claim versions with full scope and temporal filters.
pub(crate) fn query_claim_versions(
    conn: &rusqlite::Connection,
    query: &ProjectionQuery,
) -> Result<Vec<ProjectionClaimVersion>, MemoryError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }

    let scan_limit = projection_scan_limit(query);
    let terms = query_terms(query.text_query.as_deref());
    let mut stmt = conn.prepare(
        "SELECT cv.claim_version_id, cv.claim_id, cv.claim_state, cv.projection_family,
                cv.subject_entity_id, cv.predicate, cv.object_anchor,
                cv.scope_namespace, cv.scope_domain, cv.scope_workspace_id, cv.scope_repo_id,
                cv.valid_from, cv.valid_to, cv.recorded_at, cv.preferred_open,
                cv.source_envelope_id, cv.source_authority, cv.trace_id,
                cv.freshness, cv.contradiction_status, cv.supersedes_claim_version_id,
                cv.content, cv.confidence, cv.metadata,
                pil.source_exported_at, pil.transformed_at
         FROM claim_versions cv
         LEFT JOIN projection_import_log pil
           ON pil.source_envelope_id = cv.source_envelope_id
          AND pil.imported_at = cv.recorded_at
         WHERE cv.scope_namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgetting_artifact_invalidations fai
               WHERE (fai.surface_kind = 'claim_version' AND fai.artifact_id = cv.claim_version_id)
                  OR (fai.surface_kind = 'claim' AND fai.artifact_id = cv.claim_id)
           )
           AND ((?2 IS NULL AND cv.scope_domain IS NULL) OR cv.scope_domain = ?2)
           AND ((?3 IS NULL AND cv.scope_workspace_id IS NULL) OR cv.scope_workspace_id = ?3)
           AND ((?4 IS NULL AND cv.scope_repo_id IS NULL) OR cv.scope_repo_id = ?4)
           AND (?5 IS NULL OR cv.subject_entity_id = ?5)
           AND (?6 IS NULL OR cv.claim_state = ?6)
           AND (?7 IS NULL OR cv.claim_id = ?7)
           AND (?8 IS NULL OR cv.claim_version_id = ?8)
           AND (?9 IS NULL OR cv.recorded_at <= ?9)
           AND (?10 IS NULL OR ((cv.valid_from IS NULL OR cv.valid_from <= ?10)
                            AND (cv.valid_to IS NULL OR cv.valid_to > ?10)))
         ORDER BY cv.preferred_open DESC, cv.recorded_at DESC
         LIMIT ?11",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        query.scope.namespace.as_str(),
        query.scope.domain.as_deref(),
        query.scope.workspace_id.as_deref(),
        query.scope.repo_id.as_deref(),
        query.subject_entity_id.as_ref().map(|id| id.as_str()),
        query.claim_state.as_deref(),
        query.claim_id.as_ref().map(|id| id.as_str()),
        query.claim_version_id.as_ref().map(|id| id.as_str()),
        query.recorded_at_or_before.as_deref(),
        query.valid_at.as_deref(),
        scan_limit as i64,
    ])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let claim_version_id = row.get::<_, String>(0)?;
        let claim_id = row.get::<_, String>(1)?;
        let subject_entity_id = row.get::<_, String>(4)?;
        let object_anchor_raw = row.get::<_, String>(6)?;
        let searchable = format!(
            "{} {} {} {} {}",
            row.get::<_, String>(21)?,
            row.get::<_, String>(5)?,
            subject_entity_id,
            object_anchor_raw,
            claim_id
        );
        let score = text_score(&terms, &searchable);
        if !terms.is_empty() && score == 0 {
            continue;
        }

        results.push((
            score,
            ProjectionClaimVersion {
                claim_version_id: ClaimVersionId::new(claim_version_id.clone()),
                claim_id: ClaimId::new(claim_id.clone()),
                claim_state: row.get(2)?,
                projection_family: row.get(3)?,
                subject_entity_id: EntityId::new(subject_entity_id.clone()),
                predicate: row.get(5)?,
                object_anchor: decode_json_value(
                    "claim_versions",
                    &claim_version_id,
                    "object_anchor",
                    &object_anchor_raw,
                )?,
                scope_key: decode_scope_key(row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?),
                valid_from: row.get(11)?,
                valid_to: row.get(12)?,
                recorded_at: row.get(13)?,
                preferred_open: row.get::<_, i32>(14)? != 0,
                source_envelope_id: EnvelopeId::new(row.get::<_, String>(15)?),
                source_authority: row.get(16)?,
                trace_id: row.get(17)?,
                freshness: row.get(18)?,
                contradiction_status: row.get(19)?,
                supersedes_claim_version_id: row
                    .get::<_, Option<String>>(20)?
                    .map(ClaimVersionId::new),
                content: row.get(21)?,
                confidence: row.get(22)?,
                metadata: decode_optional_json_value(
                    "claim_versions",
                    &claim_version_id,
                    "metadata",
                    row.get(23)?,
                )?,
                source_exported_at: row.get(24)?,
                transformed_at: row.get(25)?,
            },
        ));
    }

    if !terms.is_empty() {
        results.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.preferred_open.cmp(&a.1.preferred_open))
                .then_with(|| b.1.recorded_at.cmp(&a.1.recorded_at))
        });
    }

    Ok(results
        .into_iter()
        .map(|(_, item)| item)
        .take(query.limit)
        .collect())
}

/// Query imported relation versions with full scope and temporal filters.
pub(crate) fn query_relation_versions(
    conn: &rusqlite::Connection,
    query: &ProjectionQuery,
) -> Result<Vec<ProjectionRelationVersion>, MemoryError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }

    let scan_limit = projection_scan_limit(query);
    let terms = query_terms(query.text_query.as_deref());
    let mut stmt = conn.prepare(
        "SELECT rv.relation_version_id, rv.subject_entity_id, rv.predicate, rv.object_anchor,
                rv.scope_namespace, rv.scope_domain, rv.scope_workspace_id, rv.scope_repo_id,
                rv.claim_id, rv.source_episode_id, rv.valid_from, rv.valid_to,
                rv.recorded_at, rv.preferred_open, rv.supersedes_relation_version_id,
                rv.contradiction_status, rv.source_confidence, rv.projection_family,
                rv.source_envelope_id, rv.source_authority, rv.trace_id,
                rv.freshness, rv.metadata,
                pil.source_exported_at, pil.transformed_at
         FROM relation_versions rv
         LEFT JOIN projection_import_log pil
           ON pil.source_envelope_id = rv.source_envelope_id
          AND pil.imported_at = rv.recorded_at
         WHERE rv.scope_namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgetting_artifact_invalidations fai
               WHERE fai.surface_kind = 'relation_version'
                 AND fai.artifact_id = rv.relation_version_id
           )
           AND ((?2 IS NULL AND rv.scope_domain IS NULL) OR rv.scope_domain = ?2)
           AND ((?3 IS NULL AND rv.scope_workspace_id IS NULL) OR rv.scope_workspace_id = ?3)
           AND ((?4 IS NULL AND rv.scope_repo_id IS NULL) OR rv.scope_repo_id = ?4)
           AND (?5 IS NULL OR rv.subject_entity_id = ?5)
           AND (?6 IS NULL OR rv.recorded_at <= ?6)
           AND (?7 IS NULL OR ((rv.valid_from IS NULL OR rv.valid_from <= ?7)
                           AND (rv.valid_to IS NULL OR rv.valid_to > ?7)))
         ORDER BY rv.preferred_open DESC, rv.recorded_at DESC
         LIMIT ?8",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        query.scope.namespace.as_str(),
        query.scope.domain.as_deref(),
        query.scope.workspace_id.as_deref(),
        query.scope.repo_id.as_deref(),
        query.subject_entity_id.as_ref().map(|id| id.as_str()),
        query.recorded_at_or_before.as_deref(),
        query.valid_at.as_deref(),
        scan_limit as i64,
    ])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let relation_version_id = row.get::<_, String>(0)?;
        let subject_entity_id = row.get::<_, String>(1)?;
        let object_anchor_raw = row.get::<_, String>(3)?;
        let claim_id = row.get::<_, Option<String>>(8)?;
        let source_episode_id = row.get::<_, Option<String>>(9)?;
        let searchable = format!(
            "{} {} {} {} {}",
            row.get::<_, String>(2)?,
            subject_entity_id,
            object_anchor_raw,
            optional_search_token(claim_id.as_deref()),
            optional_search_token(source_episode_id.as_deref())
        );
        let score = text_score(&terms, &searchable);
        if !terms.is_empty() && score == 0 {
            continue;
        }

        results.push((
            score,
            ProjectionRelationVersion {
                relation_version_id: RelationVersionId::new(relation_version_id.clone()),
                subject_entity_id: EntityId::new(subject_entity_id),
                predicate: row.get(2)?,
                object_anchor: decode_json_value(
                    "relation_versions",
                    &relation_version_id,
                    "object_anchor",
                    &object_anchor_raw,
                )?,
                scope_key: decode_scope_key(row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?),
                claim_id: claim_id.map(ClaimId::new),
                source_episode_id: source_episode_id.map(EpisodeId::new),
                valid_from: row.get(10)?,
                valid_to: row.get(11)?,
                recorded_at: row.get(12)?,
                preferred_open: row.get::<_, i32>(13)? != 0,
                supersedes_relation_version_id: row
                    .get::<_, Option<String>>(14)?
                    .map(RelationVersionId::new),
                contradiction_status: row.get(15)?,
                source_confidence: row.get(16)?,
                projection_family: row.get(17)?,
                source_envelope_id: EnvelopeId::new(row.get::<_, String>(18)?),
                source_authority: row.get(19)?,
                trace_id: row.get(20)?,
                freshness: row.get(21)?,
                metadata: decode_optional_json_value(
                    "relation_versions",
                    &relation_version_id,
                    "metadata",
                    row.get(22)?,
                )?,
                source_exported_at: row.get(23)?,
                transformed_at: row.get(24)?,
            },
        ));
    }

    if !terms.is_empty() {
        results.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.preferred_open.cmp(&a.1.preferred_open))
                .then_with(|| b.1.recorded_at.cmp(&a.1.recorded_at))
        });
    }

    Ok(results
        .into_iter()
        .map(|(_, item)| item)
        .take(query.limit)
        .collect())
}

/// Query imported episode rows with full scope and recorded-time filters.
pub(crate) fn query_episode_rows(
    conn: &rusqlite::Connection,
    query: &ProjectionQuery,
) -> Result<Vec<ProjectionEpisode>, MemoryError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }

    let scan_limit = projection_scan_limit(query);
    let terms = query_terms(query.text_query.as_deref());
    let mut stmt = conn.prepare(
        "SELECT el.episode_id, el.document_id, el.cause_ids, el.effect_type, el.outcome,
                el.confidence, el.experiment_id, el.source_envelope_id, el.source_authority,
                el.trace_id, el.recorded_at, el.metadata,
                pil.scope_namespace, pil.scope_domain, pil.scope_workspace_id, pil.scope_repo_id,
                pil.source_exported_at, pil.transformed_at
         FROM episode_links el
         JOIN projection_import_log pil
           ON pil.source_envelope_id = el.source_envelope_id
          AND pil.imported_at = el.recorded_at
         WHERE pil.scope_namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgetting_artifact_invalidations fai
               WHERE fai.surface_kind = 'episode' AND fai.artifact_id = el.episode_id
           )
           AND ((?2 IS NULL AND pil.scope_domain IS NULL) OR pil.scope_domain = ?2)
           AND ((?3 IS NULL AND pil.scope_workspace_id IS NULL) OR pil.scope_workspace_id = ?3)
           AND ((?4 IS NULL AND pil.scope_repo_id IS NULL) OR pil.scope_repo_id = ?4)
           AND (?5 IS NULL OR el.recorded_at <= ?5)
         ORDER BY el.recorded_at DESC
         LIMIT ?6",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        query.scope.namespace.as_str(),
        query.scope.domain.as_deref(),
        query.scope.workspace_id.as_deref(),
        query.scope.repo_id.as_deref(),
        query.recorded_at_or_before.as_deref(),
        scan_limit as i64,
    ])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let episode_id = row.get::<_, String>(0)?;
        let cause_ids_raw = row.get::<_, String>(2)?;
        let searchable = format!(
            "{} {} {} {}",
            row.get::<_, String>(1)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            cause_ids_raw
        );
        let score = text_score(&terms, &searchable);
        if !terms.is_empty() && score == 0 {
            continue;
        }

        results.push((
            score,
            ProjectionEpisode {
                episode_id: EpisodeId::new(episode_id.clone()),
                document_id: row.get(1)?,
                cause_ids: decode_string_vec(
                    "episode_links",
                    &episode_id,
                    "cause_ids",
                    &cause_ids_raw,
                )?,
                effect_type: row.get(3)?,
                outcome: row.get(4)?,
                confidence: row.get(5)?,
                experiment_id: row.get(6)?,
                scope_key: decode_scope_key(row.get(12)?, row.get(13)?, row.get(14)?, row.get(15)?),
                source_envelope_id: EnvelopeId::new(row.get::<_, String>(7)?),
                source_authority: row.get(8)?,
                trace_id: row.get(9)?,
                recorded_at: row.get(10)?,
                metadata: decode_optional_json_value(
                    "episode_links",
                    &episode_id,
                    "metadata",
                    row.get(11)?,
                )?,
                source_exported_at: row.get(16)?,
                transformed_at: row.get(17)?,
            },
        ));
    }

    if !terms.is_empty() {
        results.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.recorded_at.cmp(&a.1.recorded_at))
        });
    }

    Ok(results
        .into_iter()
        .map(|(_, item)| item)
        .take(query.limit)
        .collect())
}

/// Query imported alias rows with full scope and recorded-time filters.
pub(crate) fn query_entity_aliases(
    conn: &rusqlite::Connection,
    query: &ProjectionQuery,
) -> Result<Vec<ProjectionEntityAlias>, MemoryError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }

    let scan_limit = projection_scan_limit(query);
    let terms = query_terms(query.text_query.as_deref());
    let mut stmt = conn.prepare(
        "SELECT ea.canonical_entity_id, ea.alias_text, ea.alias_source, ea.match_evidence,
                ea.confidence, ea.merge_decision, ea.scope_namespace, ea.scope_domain,
                ea.scope_workspace_id, ea.scope_repo_id, ea.review_state,
                ea.is_human_confirmed, ea.is_human_confirmed_final,
                ea.superseded_by_entity_id, ea.split_from_entity_id,
                ea.source_envelope_id, ea.recorded_at,
                pil.source_exported_at, pil.transformed_at
         FROM entity_aliases ea
         LEFT JOIN projection_import_log pil
           ON pil.source_envelope_id = ea.source_envelope_id
          AND pil.imported_at = ea.recorded_at
         WHERE ea.scope_namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgetting_artifact_invalidations fai
               WHERE fai.surface_kind IN ('entity', 'entity_alias')
                 AND fai.artifact_id = ea.canonical_entity_id
           )
           AND ((?2 IS NULL AND ea.scope_domain IS NULL) OR ea.scope_domain = ?2)
           AND ((?3 IS NULL AND ea.scope_workspace_id IS NULL) OR ea.scope_workspace_id = ?3)
           AND ((?4 IS NULL AND ea.scope_repo_id IS NULL) OR ea.scope_repo_id = ?4)
           AND (?5 IS NULL OR ea.canonical_entity_id = ?5)
           AND (?6 IS NULL OR ea.recorded_at <= ?6)
         ORDER BY ea.is_human_confirmed_final DESC, ea.recorded_at DESC
         LIMIT ?7",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        query.scope.namespace.as_str(),
        query.scope.domain.as_deref(),
        query.scope.workspace_id.as_deref(),
        query.scope.repo_id.as_deref(),
        query.canonical_entity_id.as_ref().map(|id| id.as_str()),
        query.recorded_at_or_before.as_deref(),
        scan_limit as i64,
    ])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let canonical_entity_id = row.get::<_, String>(0)?;
        let searchable = format!(
            "{} {} {}",
            row.get::<_, String>(1)?,
            canonical_entity_id,
            row.get::<_, String>(2)?
        );
        let score = text_score(&terms, &searchable);
        if !terms.is_empty() && score == 0 {
            continue;
        }

        results.push((
            score,
            ProjectionEntityAlias {
                canonical_entity_id: EntityId::new(canonical_entity_id.clone()),
                alias_text: row.get(1)?,
                alias_source: row.get(2)?,
                match_evidence: decode_optional_json_value(
                    "entity_aliases",
                    &canonical_entity_id,
                    "match_evidence",
                    row.get(3)?,
                )?,
                confidence: row.get(4)?,
                merge_decision: row.get(5)?,
                scope_key: decode_scope_key(row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?),
                review_state: row.get(10)?,
                is_human_confirmed: row.get::<_, i32>(11)? != 0,
                is_human_confirmed_final: row.get::<_, i32>(12)? != 0,
                superseded_by_entity_id: row.get::<_, Option<String>>(13)?.map(EntityId::new),
                split_from_entity_id: row.get::<_, Option<String>>(14)?.map(EntityId::new),
                source_envelope_id: EnvelopeId::new(row.get::<_, String>(15)?),
                recorded_at: row.get(16)?,
                source_exported_at: row.get(17)?,
                transformed_at: row.get(18)?,
            },
        ));
    }

    if !terms.is_empty() {
        results.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    b.1.is_human_confirmed_final
                        .cmp(&a.1.is_human_confirmed_final)
                })
                .then_with(|| b.1.recorded_at.cmp(&a.1.recorded_at))
        });
    }

    Ok(results
        .into_iter()
        .map(|(_, item)| item)
        .take(query.limit)
        .collect())
}

/// Query imported evidence-reference rows with full scope and recorded-time filters.
pub(crate) fn query_evidence_refs(
    conn: &rusqlite::Connection,
    query: &ProjectionQuery,
) -> Result<Vec<ProjectionEvidenceRef>, MemoryError> {
    if query.limit == 0 {
        return Ok(Vec::new());
    }

    let scan_limit = projection_scan_limit(query);
    let terms = query_terms(query.text_query.as_deref());
    let mut stmt = conn.prepare(
        "SELECT er.claim_id, er.claim_version_id, er.fetch_handle, er.source_authority,
                er.source_envelope_id, er.recorded_at, er.metadata,
                pil.scope_namespace, pil.scope_domain, pil.scope_workspace_id, pil.scope_repo_id,
                pil.source_exported_at, pil.transformed_at
         FROM evidence_refs er
         JOIN projection_import_log pil
           ON pil.source_envelope_id = er.source_envelope_id
          AND pil.imported_at = er.recorded_at
         WHERE pil.scope_namespace = ?1
           AND NOT EXISTS (
               SELECT 1 FROM forgetting_artifact_invalidations fai
               WHERE fai.surface_kind = 'evidence_ref'
                 AND fai.artifact_id = er.fetch_handle
           )
           AND ((?2 IS NULL AND pil.scope_domain IS NULL) OR pil.scope_domain = ?2)
           AND ((?3 IS NULL AND pil.scope_workspace_id IS NULL) OR pil.scope_workspace_id = ?3)
           AND ((?4 IS NULL AND pil.scope_repo_id IS NULL) OR pil.scope_repo_id = ?4)
           AND (?5 IS NULL OR er.claim_id = ?5)
           AND (?6 IS NULL OR er.claim_version_id = ?6)
           AND (?7 IS NULL OR er.recorded_at <= ?7)
         ORDER BY er.recorded_at DESC
         LIMIT ?8",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        query.scope.namespace.as_str(),
        query.scope.domain.as_deref(),
        query.scope.workspace_id.as_deref(),
        query.scope.repo_id.as_deref(),
        query.claim_id.as_ref().map(|id| id.as_str()),
        query.claim_version_id.as_ref().map(|id| id.as_str()),
        query.recorded_at_or_before.as_deref(),
        scan_limit as i64,
    ])?;

    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        let claim_id = row.get::<_, String>(0)?;
        let claim_version_id = row.get::<_, Option<String>>(1)?;
        let fetch_handle = row.get::<_, String>(2)?;
        let searchable = format!(
            "{} {} {}",
            claim_id,
            optional_search_token(claim_version_id.as_deref()),
            fetch_handle
        );
        let score = text_score(&terms, &searchable);
        if !terms.is_empty() && score == 0 {
            continue;
        }

        results.push((
            score,
            ProjectionEvidenceRef {
                claim_id: ClaimId::new(claim_id),
                claim_version_id: claim_version_id.map(ClaimVersionId::new),
                fetch_handle,
                source_authority: row.get(3)?,
                source_envelope_id: EnvelopeId::new(row.get::<_, String>(4)?),
                scope_key: decode_scope_key(row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?),
                recorded_at: row.get(5)?,
                metadata: decode_optional_json_value(
                    "evidence_refs",
                    &row.get::<_, String>(4)?,
                    "metadata",
                    row.get(6)?,
                )?,
                source_exported_at: row.get(11)?,
                transformed_at: row.get(12)?,
            },
        ));
    }

    if !terms.is_empty() {
        results.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.recorded_at.cmp(&a.1.recorded_at))
        });
    }

    Ok(results
        .into_iter()
        .map(|(_, item)| item)
        .take(query.limit)
        .collect())
}

/// Query projection import log entries.
pub(crate) fn query_projection_import_log(
    conn: &rusqlite::Connection,
    scope_namespace: Option<&str>,
    limit: usize,
) -> Result<Vec<ProjectionImportLogRow>, MemoryError> {
    let sql = if let Some(ns) = scope_namespace {
        let mut stmt = conn.prepare(
            "SELECT batch_id, source_envelope_id, schema_version, export_schema_version,
                    content_digest, source_authority, scope_namespace, scope_domain,
                    scope_workspace_id, scope_repo_id, trace_id, record_count,
                    claim_count, relation_count, episode_count, alias_count, evidence_count,
                    status, source_exported_at, transformed_at, imported_at,
                    source_run_id, comparability_snapshot_version, direct_write, failure_reason,
                    evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
                    execution_context_json, kernel_payload_json
             FROM projection_import_log
             WHERE scope_namespace = ?1
             ORDER BY imported_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![ns, limit as i64], map_import_log_row)?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(rows);
    } else {
        "SELECT batch_id, source_envelope_id, schema_version, export_schema_version,
                content_digest, source_authority, scope_namespace, scope_domain,
                scope_workspace_id, scope_repo_id, trace_id, record_count,
                claim_count, relation_count, episode_count, alias_count, evidence_count,
                status, source_exported_at, transformed_at, imported_at,
                source_run_id, comparability_snapshot_version, direct_write, failure_reason,
                evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
                execution_context_json, kernel_payload_json
         FROM projection_import_log
         ORDER BY imported_at DESC LIMIT ?1"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], map_import_log_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn latest_rebuildable_kernel_projection_import(
    conn: &rusqlite::Connection,
    scope: &ScopeKey,
) -> Result<Option<ProjectionImportLogRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT batch_id, source_envelope_id, schema_version, export_schema_version,
                content_digest, source_authority, scope_namespace, scope_domain,
                scope_workspace_id, scope_repo_id, trace_id, record_count,
                claim_count, relation_count, episode_count, alias_count, evidence_count,
                status, source_exported_at, transformed_at, imported_at,
                source_run_id, comparability_snapshot_version, direct_write, failure_reason,
                evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
                execution_context_json, kernel_payload_json
         FROM projection_import_log
         WHERE scope_namespace = ?1
           AND ((?2 IS NULL AND scope_domain IS NULL) OR scope_domain = ?2)
           AND ((?3 IS NULL AND scope_workspace_id IS NULL) OR scope_workspace_id = ?3)
           AND ((?4 IS NULL AND scope_repo_id IS NULL) OR scope_repo_id = ?4)
           AND status = 'complete'
           AND kernel_payload_json IS NOT NULL
         ORDER BY imported_at DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(rusqlite::params![
        scope.namespace.as_str(),
        scope.domain.as_deref(),
        scope.workspace_id.as_deref(),
        scope.repo_id.as_deref(),
    ])?;
    match rows.next()? {
        Some(row) => Ok(Some(map_import_log_row(row)?)),
        None => Ok(None),
    }
}

pub(crate) fn query_projection_import_failures(
    conn: &rusqlite::Connection,
    scope_namespace: Option<&str>,
    limit: usize,
) -> Result<Vec<ProjectionImportFailureRow>, MemoryError> {
    let sql = if let Some(ns) = scope_namespace {
        let mut stmt = conn.prepare(
            "SELECT failure_id, source_envelope_id, schema_version, export_schema_version,
                    content_digest, source_authority, scope_namespace, scope_domain,
                    scope_workspace_id, scope_repo_id, trace_id, record_count,
                    error_kind, error_message, source_exported_at, transformed_at, failed_at,
                    source_run_id, comparability_snapshot_version, direct_write,
                    evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
                    execution_context_json, kernel_payload_json
             FROM projection_import_failures
             WHERE scope_namespace = ?1
             ORDER BY failed_at DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![ns, limit as i64], map_import_failure_row)?
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(rows);
    } else {
        "SELECT failure_id, source_envelope_id, schema_version, export_schema_version,
                content_digest, source_authority, scope_namespace, scope_domain,
                scope_workspace_id, scope_repo_id, trace_id, record_count,
                error_kind, error_message, source_exported_at, transformed_at, failed_at,
                source_run_id, comparability_snapshot_version, direct_write,
                evidence_bundle_id, evidence_bundle_json, episode_bundle_id, episode_bundle_json,
                execution_context_json, kernel_payload_json
         FROM projection_import_failures
         ORDER BY failed_at DESC LIMIT ?1"
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(rusqlite::params![limit as i64], map_import_failure_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn map_import_log_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectionImportLogRow> {
    Ok(ProjectionImportLogRow {
        batch_id: row.get(0)?,
        source_envelope_id: row.get(1)?,
        schema_version: row.get(2)?,
        export_schema_version: row.get(3)?,
        content_digest: row.get(4)?,
        source_authority: row.get(5)?,
        scope_namespace: row.get(6)?,
        scope_domain: row.get(7)?,
        scope_workspace_id: row.get(8)?,
        scope_repo_id: row.get(9)?,
        trace_id: row.get(10)?,
        record_count: row.get::<_, i64>(11)? as usize,
        claim_count: row.get::<_, i64>(12)? as usize,
        relation_count: row.get::<_, i64>(13)? as usize,
        episode_count: row.get::<_, i64>(14)? as usize,
        alias_count: row.get::<_, i64>(15)? as usize,
        evidence_count: row.get::<_, i64>(16)? as usize,
        status: row.get(17)?,
        source_exported_at: row.get(18)?,
        transformed_at: row.get(19)?,
        imported_at: row.get(20)?,
        source_run_id: row.get(21)?,
        comparability_snapshot_version: row.get(22)?,
        direct_write: row.get::<_, i64>(23)? != 0,
        failure_reason: row.get(24)?,
        evidence_bundle_id: row.get(25)?,
        evidence_bundle_json: row.get(26)?,
        episode_bundle_id: row.get(27)?,
        episode_bundle_json: row.get(28)?,
        execution_context_json: row.get(29)?,
        kernel_payload_json: row.get(30)?,
    })
}

fn map_import_failure_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectionImportFailureRow> {
    Ok(ProjectionImportFailureRow {
        failure_id: row.get(0)?,
        source_envelope_id: row.get(1)?,
        schema_version: row.get(2)?,
        export_schema_version: row.get(3)?,
        content_digest: row.get(4)?,
        source_authority: row.get(5)?,
        scope_namespace: row.get(6)?,
        scope_domain: row.get(7)?,
        scope_workspace_id: row.get(8)?,
        scope_repo_id: row.get(9)?,
        trace_id: row.get(10)?,
        record_count: row.get::<_, i64>(11)? as usize,
        error_kind: row.get(12)?,
        error_message: row.get(13)?,
        source_exported_at: row.get(14)?,
        transformed_at: row.get(15)?,
        failed_at: row.get(16)?,
        source_run_id: row.get(17)?,
        comparability_snapshot_version: row.get(18)?,
        direct_write: row.get::<_, i64>(19)? != 0,
        evidence_bundle_id: row.get(20)?,
        evidence_bundle_json: row.get(21)?,
        episode_bundle_id: row.get(22)?,
        episode_bundle_json: row.get(23)?,
        execution_context_json: row.get(24)?,
        kernel_payload_json: row.get(25)?,
    })
}

/// Flat row struct for inserting claim versions.
pub(crate) struct ClaimVersionRow {
    pub claim_version_id: String,
    pub claim_id: String,
    pub claim_state: String,
    pub projection_family: String,
    pub subject_entity_id: String,
    pub predicate: String,
    pub object_anchor: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub recorded_at: String,
    pub preferred_open: bool,
    pub source_envelope_id: String,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub freshness: String,
    pub contradiction_status: String,
    pub supersedes_claim_version_id: Option<String>,
    pub content: String,
    pub confidence: f32,
    pub content_digest: Option<String>,
    pub metadata: Option<String>,
}

/// Flat row struct for inserting relation versions.
pub(crate) struct RelationVersionRow {
    pub relation_version_id: String,
    pub subject_entity_id: String,
    pub predicate: String,
    pub object_anchor: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub claim_id: Option<String>,
    pub source_episode_id: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub recorded_at: String,
    pub preferred_open: bool,
    pub supersedes_relation_version_id: Option<String>,
    pub contradiction_status: String,
    pub source_confidence: f32,
    pub projection_family: String,
    pub source_envelope_id: String,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub freshness: String,
    pub metadata: Option<String>,
}

/// Flat row struct for inserting entity aliases.
pub(crate) struct EntityAliasRow {
    pub canonical_entity_id: String,
    pub alias_text: String,
    pub alias_source: String,
    pub match_evidence: Option<String>,
    pub confidence: f32,
    pub merge_decision: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub review_state: String,
    pub is_human_confirmed: bool,
    pub is_human_confirmed_final: bool,
    pub superseded_by_entity_id: Option<String>,
    pub split_from_entity_id: Option<String>,
    pub source_envelope_id: String,
    pub recorded_at: String,
}

/// Flat row struct for inserting evidence refs.
pub(crate) struct EvidenceRefRow {
    pub claim_id: String,
    pub claim_version_id: Option<String>,
    pub fetch_handle: String,
    pub source_authority: String,
    pub source_envelope_id: String,
    pub recorded_at: String,
    pub metadata: Option<String>,
}

/// Flat row struct for episode links.
pub(crate) struct EpisodeLinkRow {
    pub episode_id: String,
    pub document_id: String,
    pub cause_ids: String,
    pub effect_type: String,
    pub outcome: String,
    pub confidence: f32,
    pub experiment_id: Option<String>,
    pub source_envelope_id: String,
    pub source_authority: String,
    pub trace_id: Option<String>,
    pub recorded_at: String,
    pub metadata: Option<String>,
}

/// Flat row struct for projection import log.
#[derive(Clone)]
pub(crate) struct ProjectionImportLogRow {
    pub batch_id: String,
    pub source_envelope_id: String,
    pub schema_version: String,
    pub export_schema_version: Option<String>,
    pub content_digest: String,
    pub source_authority: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub trace_id: Option<String>,
    pub record_count: usize,
    pub claim_count: usize,
    pub relation_count: usize,
    pub episode_count: usize,
    pub alias_count: usize,
    pub evidence_count: usize,
    pub status: String,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
    pub imported_at: String,
    pub source_run_id: Option<String>,
    pub comparability_snapshot_version: Option<String>,
    pub direct_write: bool,
    pub failure_reason: Option<String>,
    pub evidence_bundle_id: Option<String>,
    pub evidence_bundle_json: Option<String>,
    pub episode_bundle_id: Option<String>,
    pub episode_bundle_json: Option<String>,
    pub execution_context_json: Option<String>,
    pub kernel_payload_json: Option<String>,
}

/// Durable failed projection import receipt.
pub(crate) struct ProjectionImportFailureRow {
    pub failure_id: String,
    pub source_envelope_id: String,
    pub schema_version: String,
    pub export_schema_version: Option<String>,
    pub content_digest: String,
    pub source_authority: String,
    pub scope_namespace: String,
    pub scope_domain: Option<String>,
    pub scope_workspace_id: Option<String>,
    pub scope_repo_id: Option<String>,
    pub trace_id: Option<String>,
    pub record_count: usize,
    pub error_kind: String,
    pub error_message: String,
    pub source_exported_at: Option<String>,
    pub transformed_at: Option<String>,
    pub failed_at: String,
    pub source_run_id: Option<String>,
    pub comparability_snapshot_version: Option<String>,
    pub direct_write: bool,
    pub evidence_bundle_id: Option<String>,
    pub evidence_bundle_json: Option<String>,
    pub episode_bundle_id: Option<String>,
    pub episode_bundle_json: Option<String>,
    pub execution_context_json: Option<String>,
    pub kernel_payload_json: Option<String>,
}
