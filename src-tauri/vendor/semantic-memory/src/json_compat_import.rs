//! JSON compatibility normalization for legacy projection batch payloads.

use crate::error::MemoryError;
use crate::projection_storage;
use forge_memory_bridge::{
    ContradictionStatus, MergeDecision, ProjectionImportBatchV2, ReviewState,
    PROJECTION_IMPORT_BATCH_V1_SCHEMA, PROJECTION_IMPORT_BATCH_V2_SCHEMA,
};
use stack_ids::ContentDigest;

pub(crate) const EXPORT_ENVELOPE_V1_JSON_COMPAT: &str = "export_envelope_v1";
pub(crate) const EXPORT_ENVELOPE_V2_JSON_COMPAT: &str = "export_envelope_v2";
pub(crate) const JSON_COMPAT_DEFAULT_TIMESTAMP: &str = "1970-01-01T00:00:00Z";

pub(crate) fn json_compat_invalid(reason: impl Into<String>) -> MemoryError {
    MemoryError::ImportInvalid {
        reason: format!("invalid batch JSON: {}", reason.into()),
    }
}

pub(crate) fn build_json_compat_failure_log_row(
    batch_json: &str,
    imported_at: String,
) -> projection_storage::ProjectionImportLogRow {
    let parsed = serde_json::from_str::<serde_json::Value>(batch_json).ok();
    let root = parsed.as_ref().and_then(|value| value.as_object());
    let scope_key = root
        .and_then(|obj| obj.get("scope_key"))
        .and_then(|value| value.as_object());
    let export_meta = root
        .and_then(|obj| obj.get("export_meta"))
        .and_then(|value| value.as_object());
    let evidence_bundle = root.and_then(|obj| obj.get("evidence_bundle"));
    let content_digest = root
        .and_then(|obj| obj.get("content_digest"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| ContentDigest::compute_str(batch_json).hex().to_string());
    let original_schema_version = root
        .and_then(|obj| obj.get("schema_version"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let export_schema_version = root
        .and_then(|obj| obj.get("export_schema_version"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    let (schema_version, export_schema_version) = match original_schema_version.as_deref() {
        Some(EXPORT_ENVELOPE_V1_JSON_COMPAT) => (
            PROJECTION_IMPORT_BATCH_V2_SCHEMA.to_string(),
            Some(EXPORT_ENVELOPE_V1_JSON_COMPAT.to_string()),
        ),
        Some(EXPORT_ENVELOPE_V2_JSON_COMPAT) => (
            PROJECTION_IMPORT_BATCH_V2_SCHEMA.to_string(),
            Some(EXPORT_ENVELOPE_V2_JSON_COMPAT.to_string()),
        ),
        Some(PROJECTION_IMPORT_BATCH_V1_SCHEMA) => (
            PROJECTION_IMPORT_BATCH_V2_SCHEMA.to_string(),
            export_schema_version,
        ),
        Some(other) => (other.to_string(), export_schema_version),
        None => ("json_compat_unknown".into(), export_schema_version),
    };
    let source_envelope_id = root
        .and_then(|obj| obj.get("source_envelope_id"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| format!("json-compat-invalid:{content_digest}"));

    projection_storage::ProjectionImportLogRow {
        batch_id: crate::projection_import_failure_id(
            &source_envelope_id,
            &schema_version,
            &content_digest,
        ),
        source_envelope_id,
        schema_version,
        export_schema_version,
        content_digest,
        source_authority: root
            .and_then(|obj| obj.get("source_authority"))
            .and_then(|value| value.as_str())
            .unwrap_or("unknown")
            .to_string(),
        scope_namespace: scope_key
            .and_then(|obj| obj.get("namespace"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                root.and_then(|obj| obj.get("namespace"))
                    .and_then(|value| value.as_str())
            })
            .unwrap_or("json-compat-invalid")
            .to_string(),
        scope_domain: scope_key
            .and_then(|obj| obj.get("domain"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        scope_workspace_id: scope_key
            .and_then(|obj| obj.get("workspace_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        scope_repo_id: scope_key
            .and_then(|obj| obj.get("repo_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        trace_id: root
            .and_then(|obj| obj.get("trace_ctx"))
            .and_then(|value| value.get("trace_id"))
            .and_then(|value| value.as_str())
            .or_else(|| {
                root.and_then(|obj| obj.get("trace_id"))
                    .and_then(|value| value.as_str())
            })
            .map(|value| value.to_string()),
        record_count: root
            .and_then(|obj| obj.get("records"))
            .and_then(|value| value.as_array())
            .map(|records| records.len())
            .unwrap_or(0),
        claim_count: 0,
        relation_count: 0,
        episode_count: 0,
        alias_count: 0,
        evidence_count: 0,
        status: "failed".into(),
        source_exported_at: root
            .and_then(|obj| obj.get("source_exported_at"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        transformed_at: root
            .and_then(|obj| obj.get("transformed_at"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        imported_at,
        source_run_id: export_meta
            .and_then(|obj| obj.get("run_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        comparability_snapshot_version: export_meta
            .and_then(|obj| obj.get("comparability_snapshot_version"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        direct_write: export_meta
            .and_then(|obj| obj.get("direct_write"))
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        failure_reason: None,
        evidence_bundle_id: evidence_bundle
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        evidence_bundle_json: evidence_bundle.map(|value| value.to_string()),
        episode_bundle_id: None,
        episode_bundle_json: None,
        execution_context_json: None,
        kernel_payload_json: None,
    }
}

pub(crate) fn decode_projection_batch_json_compat(
    batch_json: &str,
) -> Result<ProjectionImportBatchV2, MemoryError> {
    let mut value: serde_json::Value =
        serde_json::from_str(batch_json).map_err(|e| json_compat_invalid(e.to_string()))?;
    let root = value
        .as_object_mut()
        .ok_or_else(|| json_compat_invalid("top-level payload must be an object"))?;

    let original_schema_version = root
        .get("schema_version")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    match original_schema_version.as_deref() {
        Some(EXPORT_ENVELOPE_V1_JSON_COMPAT) => {
            root.entry("export_schema_version".to_string())
                .or_insert_with(|| serde_json::json!(EXPORT_ENVELOPE_V1_JSON_COMPAT));
            root.insert(
                "schema_version".to_string(),
                serde_json::json!(PROJECTION_IMPORT_BATCH_V2_SCHEMA),
            );
        }
        Some(EXPORT_ENVELOPE_V2_JSON_COMPAT) => {
            root.entry("export_schema_version".to_string())
                .or_insert_with(|| serde_json::json!(EXPORT_ENVELOPE_V2_JSON_COMPAT));
            root.insert(
                "schema_version".to_string(),
                serde_json::json!(PROJECTION_IMPORT_BATCH_V2_SCHEMA),
            );
        }
        Some(PROJECTION_IMPORT_BATCH_V1_SCHEMA) => {
            root.insert(
                "schema_version".to_string(),
                serde_json::json!(PROJECTION_IMPORT_BATCH_V2_SCHEMA),
            );
        }
        Some(PROJECTION_IMPORT_BATCH_V2_SCHEMA) | None => {}
        Some(other) => {
            return Err(MemoryError::ImportInvalid {
                reason: format!(
                    "unsupported schema_version: {}; expected {}, {}, {}, or {}",
                    other,
                    PROJECTION_IMPORT_BATCH_V2_SCHEMA,
                    PROJECTION_IMPORT_BATCH_V1_SCHEMA,
                    EXPORT_ENVELOPE_V2_JSON_COMPAT,
                    EXPORT_ENVELOPE_V1_JSON_COMPAT
                ),
            });
        }
    }

    root.entry("source_exported_at".to_string())
        .or_insert_with(|| serde_json::json!(JSON_COMPAT_DEFAULT_TIMESTAMP));
    root.entry("transformed_at".to_string())
        .or_insert_with(|| serde_json::json!(JSON_COMPAT_DEFAULT_TIMESTAMP));

    let default_source_envelope_id = root.get("source_envelope_id").cloned();
    let default_source_authority = root.get("source_authority").cloned();
    let default_scope_key = root.get("scope_key").cloned();
    let default_trace_ctx = root.get("trace_ctx").cloned();

    if let Some(records) = root
        .get_mut("records")
        .and_then(|value| value.as_array_mut())
    {
        for record in records {
            let Some(obj) = record.as_object_mut() else {
                continue;
            };

            match obj.get("kind").and_then(|value| value.as_str()) {
                Some("claim_version") => {
                    insert_default_json_field(obj, "scope_key", default_scope_key.as_ref());
                    insert_default_json_field(
                        obj,
                        "source_envelope_id",
                        default_source_envelope_id.as_ref(),
                    );
                    insert_default_json_field(
                        obj,
                        "source_authority",
                        default_source_authority.as_ref(),
                    );
                    insert_default_json_field(obj, "trace_ctx", default_trace_ctx.as_ref());
                    insert_default_json_field(
                        obj,
                        "contradiction_status",
                        Some(&serde_json::json!(ContradictionStatus::None)),
                    );
                }
                Some("relation_version") => {
                    insert_default_json_field(obj, "scope_key", default_scope_key.as_ref());
                    insert_default_json_field(
                        obj,
                        "source_envelope_id",
                        default_source_envelope_id.as_ref(),
                    );
                    insert_default_json_field(
                        obj,
                        "source_authority",
                        default_source_authority.as_ref(),
                    );
                    insert_default_json_field(obj, "trace_ctx", default_trace_ctx.as_ref());
                    insert_default_json_field(
                        obj,
                        "contradiction_status",
                        Some(&serde_json::json!(ContradictionStatus::None)),
                    );
                }
                Some("episode") => {
                    insert_default_json_field(obj, "cause_ids", Some(&serde_json::json!([])));
                    insert_default_json_field(
                        obj,
                        "source_envelope_id",
                        default_source_envelope_id.as_ref(),
                    );
                    insert_default_json_field(
                        obj,
                        "source_authority",
                        default_source_authority.as_ref(),
                    );
                    insert_default_json_field(obj, "trace_ctx", default_trace_ctx.as_ref());
                }
                Some("entity_alias") => {
                    insert_default_json_field(obj, "scope", default_scope_key.as_ref());
                    insert_default_json_field(
                        obj,
                        "source_envelope_id",
                        default_source_envelope_id.as_ref(),
                    );
                    insert_default_json_field(
                        obj,
                        "review_state",
                        Some(&serde_json::json!(ReviewState::Unreviewed)),
                    );
                    insert_default_json_field(
                        obj,
                        "is_human_confirmed",
                        Some(&serde_json::json!(false)),
                    );
                    insert_default_json_field(
                        obj,
                        "is_human_confirmed_final",
                        Some(&serde_json::json!(false)),
                    );
                    validate_json_compat_field::<MergeDecision>(obj, "merge_decision")?;
                    validate_json_compat_field::<ReviewState>(obj, "review_state")?;
                }
                Some("evidence_ref") => {
                    insert_default_json_field(
                        obj,
                        "source_envelope_id",
                        default_source_envelope_id.as_ref(),
                    );
                }
                _ => {}
            }
        }
    }

    serde_json::from_value(value).map_err(|e| json_compat_invalid(e.to_string()))
}

fn insert_default_json_field(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: Option<&serde_json::Value>,
) {
    if !obj.contains_key(key) {
        if let Some(value) = default {
            obj.insert(key.to_string(), value.clone());
        }
    }
}

fn validate_json_compat_field<T>(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<(), MemoryError>
where
    T: serde::de::DeserializeOwned,
{
    if let Some(value) = obj.get(field) {
        serde_json::from_value::<T>(value.clone())
            .map(|_| ())
            .map_err(|err| json_compat_invalid(format!("{field}: {err}")))?;
    }
    Ok(())
}
