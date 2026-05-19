use super::*;
use semantic_memory_forge::{ExportClaim, ExportEntityAlias, ExportEvidenceRef, ExportRelation};
use stack_ids::{EntityId, EnvelopeId, ScopeKey};

fn make_claim_envelope() -> ExportEnvelopeV1 {
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: Some(ClaimId::new("claim-1")),
        claim_version_id: None,
        subject_entity_id: EntityId::new("ent-1"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        valid_from: Some("2026-01-01T00:00:00Z".into()),
        valid_to: None,
        confidence: 0.95,
        content: "Entity ent-1 is a function".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: None,
    })];
    let scope = ScopeKey::namespace_only("test-ns");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-001"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: Some(TraceCtx::generate()),
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    }
}

fn make_claim_envelope_with_metadata(metadata: serde_json::Value) -> ExportEnvelopeV1 {
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: Some(ClaimId::new("claim-verification")),
        claim_version_id: Some(ClaimVersionId::new("claim-verification-v1")),
        subject_entity_id: EntityId::new("ent-verification"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("verification"),
        valid_from: Some("2026-01-01T00:00:00Z".into()),
        valid_to: None,
        confidence: 0.95,
        content: "Entity ent-verification is verification-backed".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: Some(metadata),
    })];
    let scope = ScopeKey::namespace_only("test-ns");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-verification-summary"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: Some(TraceCtx::generate()),
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    }
}

#[test]
fn transform_claim_envelope() {
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();

    assert_eq!(batch.source_envelope_id, env.envelope_id);
    assert_eq!(batch.source_authority, "forge");
    assert_eq!(batch.records.len(), 1);

    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(cv.claim_id.as_str(), "claim-1");
            assert!(!cv.claim_version_id.is_empty());
            assert_eq!(cv.claim_state, ClaimState::Active);
            assert_eq!(cv.predicate, "has_type");
            assert!(cv.preferred_open); // valid_to is None
            assert_eq!(cv.freshness, ProjectionFreshness::Current);
            assert_eq!(cv.source_envelope_id, env.envelope_id);
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn transform_preserves_trace_ctx() {
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();
    assert!(batch.trace_ctx.is_some());
    assert_eq!(
        batch.trace_ctx.as_ref().unwrap().trace_id,
        env.trace_ctx.as_ref().unwrap().trace_id
    );
}

#[test]
fn transform_assigns_version_ids() {
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();

    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert!(!cv.claim_version_id.is_empty());
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn transform_rejects_incompatible_version() {
    let mut env = make_claim_envelope();
    env.schema_version = "wrong_v2".into();
    let err = transform_envelope(&env).unwrap_err();
    assert!(matches!(err, BridgeError::IncompatibleVersion { .. }));
}

#[test]
fn transform_rejects_tampered_digest() {
    let mut env = make_claim_envelope();
    env.content_digest = stack_ids::ContentDigest::compute(b"tampered");
    let err = transform_envelope(&env).unwrap_err();
    assert!(matches!(err, BridgeError::DigestMismatch { .. }));
}

#[test]
fn transform_multi_record_envelope() {
    let records = vec![
        ExportRecord::Claim(ExportClaim {
            claim_id: None,
            claim_version_id: None,
            subject_entity_id: EntityId::new("e1"),
            predicate: "p1".into(),
            object_anchor: serde_json::json!("v1"),
            valid_from: None,
            valid_to: None,
            confidence: 0.9,
            content: "claim one".into(),
            projection_family: "forge".into(),
            supersedes_claim_id: None,
            supersedes_claim_version_id: None,
            metadata: None,
        }),
        ExportRecord::Relation(ExportRelation {
            relation_version_id: None,
            subject_entity_id: EntityId::new("e1"),
            predicate: "depends_on".into(),
            object_anchor: serde_json::json!("e2"),
            valid_from: None,
            valid_to: None,
            confidence: 0.8,
            projection_family: "forge".into(),
            source_claim_id: None,
            source_episode_id: None,
            supersedes_relation_version_id: None,
            metadata: None,
        }),
        ExportRecord::EvidenceRef(ExportEvidenceRef {
            claim_id: ClaimId::new("claim-x"),
            claim_version_id: None,
            fetch_handle: "forge://evidence/run-42/artifact-7".into(),
            source_authority: "forge".into(),
            metadata: None,
        }),
    ];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-multi"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    assert_eq!(batch.records.len(), 3);

    assert!(matches!(
        &batch.records[0],
        ImportProjectionRecord::ClaimVersion(_)
    ));
    assert!(matches!(
        &batch.records[1],
        ImportProjectionRecord::RelationVersion(_)
    ));
    assert!(matches!(
        &batch.records[2],
        ImportProjectionRecord::EvidenceRef(_)
    ));
}

#[test]
fn is_compatible_version_check() {
    assert!(is_compatible_version(EXPORT_ENVELOPE_V1_SCHEMA));
    assert!(is_compatible_version(EXPORT_ENVELOPE_V2_SCHEMA));
    assert!(!is_compatible_version(""));
}

#[test]
fn entity_alias_never_sets_human_confirmed_final() {
    let records = vec![ExportRecord::EntityAlias(ExportEntityAlias {
        canonical_entity_id: EntityId::new("e1"),
        alias_text: "Entity One".into(),
        alias_source: "forge_extraction".into(),
        match_evidence: None,
        confidence: 0.9,
        scope: None,
        superseded_by_entity_id: None,
        split_from_entity_id: None,
    })];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-alias"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::EntityAlias(alias) => {
            // Automated flows must never mark human_confirmed_final
            assert!(!alias.is_human_confirmed_final);
            assert!(!alias.is_human_confirmed);
            assert_eq!(alias.review_state, ReviewState::PendingReview);
        }
        _ => panic!("expected EntityAlias"),
    }
}

#[test]
fn bridge_trace_ctx_chains_from_parent() {
    let parent = TraceCtx::generate();
    let child = bridge_trace_ctx(Some(&parent));
    assert_eq!(child.trace_id, parent.trace_id);
    assert!(child.parent_id.is_some());
}

#[test]
fn bridge_trace_ctx_generates_fresh_without_parent() {
    let ctx = bridge_trace_ctx(None);
    assert!(!ctx.trace_id.is_empty());
    assert!(ctx.parent_id.is_none());
}

#[test]
fn bridge_only_stamps_batch_transformed_at() {
    // The bridge contributes batch-level transform provenance only.
    // Authoritative row `recorded_at` is assigned later by the importer.
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();
    assert!(!batch.transformed_at.is_empty());
}

#[test]
fn supersession_does_not_synthesize_version_id() {
    // I004: Proves the bridge does NOT mint a fake ClaimVersionId when
    // supersedes_claim_id is present but version lineage is still absent.
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: Some(ClaimId::new("claim-superseding")),
        claim_version_id: None,
        subject_entity_id: EntityId::new("ent-1"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        valid_from: None,
        valid_to: None,
        confidence: 0.9,
        content: "Updated claim".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: Some(ClaimId::new("claim-old")),
        supersedes_claim_version_id: None,
        metadata: None,
    })];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-supersede"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            // Must NOT have a synthesized supersedes_claim_version_id
            assert!(
                cv.supersedes_claim_version_id.is_none(),
                "bridge must not mint synthetic supersedes_claim_version_id"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn supersession_preserves_real_version_id() {
    let prior_version = ClaimVersionId::new("claim-old-v3");
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: Some(ClaimId::new("claim-superseding")),
        claim_version_id: None,
        subject_entity_id: EntityId::new("ent-1"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        valid_from: None,
        valid_to: None,
        confidence: 0.9,
        content: "Updated claim".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: Some(ClaimId::new("claim-old")),
        supersedes_claim_version_id: Some(prior_version.clone()),
        metadata: None,
    })];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-supersede-versioned"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(
                cv.supersedes_claim_version_id,
                Some(prior_version),
                "bridge must preserve real version lineage verbatim"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn schema_version_uses_bridge_import_contract() {
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();
    assert_eq!(
        batch.schema_version, PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "bridge batches must advertise the import-side schema contract"
    );
}

// ─── BRG-001: Version vocabulary proof ────────────────────────

#[test]
fn brg001_import_and_export_schema_versions_are_distinct_fields() {
    // BRG-001: Proves the batch carries both import-side `schema_version`
    // (used by bridge compatibility checks) and `export_schema_version`
    // (recorded for provenance).
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();

    // schema_version is the import-side version the batch conforms to
    assert_eq!(batch.schema_version, PROJECTION_IMPORT_BATCH_V1_SCHEMA);
    // export_schema_version is the provenance record of what the source sent
    assert_eq!(
        batch.export_schema_version.as_deref(),
        Some(EXPORT_ENVELOPE_V1_SCHEMA),
        "export_schema_version preserves the original export version for provenance"
    );
    assert_ne!(
        batch.schema_version,
        batch.export_schema_version.unwrap(),
        "import schema and export provenance must remain distinct"
    );
}

// ─── BRG-002: Supersession freeze proof ───────────────────────

#[test]
fn brg002_supersession_is_explicitly_deferred_not_synthetic() {
    // When only claim-level supersession is present, the bridge preserves
    // the absence of version lineage rather than inventing it.
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: Some(ClaimId::new("claim-new")),
        claim_version_id: None,
        subject_entity_id: EntityId::new("ent-1"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        valid_from: None,
        valid_to: None,
        confidence: 0.9,
        content: "New claim superseding old".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: Some(ClaimId::new("claim-old")),
        supersedes_claim_version_id: None,
        metadata: None,
    })];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-brg002"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            // Claim-level supersession is present (carried from export)
            // but version-level supersession is explicitly None (deferred)
            assert!(
                cv.supersedes_claim_version_id.is_none(),
                "BRG-002: version-level supersession must remain None until \
                     export schema carries real ClaimVersionId"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }
}

// ─── BRG-003: Bridge-assigned alias safety defaults proof ─────

#[test]
fn brg003_bridge_assigned_defaults_vs_exporter_truth() {
    // BRG-003: Documents which fields in the import batch are
    // exporter-owned truth vs bridge-assigned transform defaults.
    //
    // Exporter truth (carried through from envelope):
    //   - source_envelope_id, source_authority, scope_key, trace_ctx
    //   - claim content, predicate, object_anchor, subject_entity_id
    //   - confidence, valid_from, valid_to, projection_family
    //   - supersedes_claim_id / supersedes_claim_version_id when present
    //
    // Bridge-assigned transform defaults when the exporter does NOT carry an
    // explicit verification summary:
    //   - claim_state: Active
    //   - freshness: Current
    //   - contradiction_status: None
    //   - preferred_open: derived from valid_to (bridge rule)
    //   - claim_version_id: generated (bridge mints)
    //   - relation_version_id: generated (bridge mints)
    //   - entity alias review_state: PendingReview (bridge safety default)
    //   - entity alias merge_decision: PendingReview (bridge safety default)
    //   - entity alias is_human_confirmed: false (bridge safety)
    let env = make_claim_envelope();
    let batch = transform_envelope(&env).unwrap();

    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            // Bridge-assigned defaults
            assert_eq!(
                cv.claim_state,
                ClaimState::Active,
                "BRG-003: claim_state is bridge-assigned, not exporter truth"
            );
            assert_eq!(
                cv.freshness,
                ProjectionFreshness::Current,
                "BRG-003: freshness is bridge-assigned, not exporter truth"
            );
            assert_eq!(
                cv.contradiction_status,
                ContradictionStatus::None,
                "BRG-003: contradiction_status is bridge-assigned, not exporter truth"
            );
            assert!(
                cv.preferred_open,
                "BRG-003: preferred_open is derived from valid_to by bridge rule"
            );

            // Exporter truth (carried through)
            assert_eq!(
                cv.source_authority, "forge",
                "source_authority is exporter truth"
            );
            assert_eq!(
                cv.source_envelope_id, env.envelope_id,
                "source_envelope_id is exporter truth"
            );
            assert_eq!(
                cv.content, "Entity ent-1 is a function",
                "content is exporter truth"
            );
            assert!(
                (cv.confidence - 0.95).abs() < f32::EPSILON,
                "confidence is exporter truth"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn bridge_uses_exported_verification_summary_for_claim_state() {
    let contradicted = make_claim_envelope_with_metadata(serde_json::json!({
        "verification_summary": {
            "lifecycle_state": "contradicted",
            "promotion_state": { "state": "blocked", "reason": "placebo_failed" },
            "completed_trial_count": 2,
            "passed_refutation_count": 0,
            "failed_refutation_count": 1,
            "notes": ["placebo failed hard"]
        }
    }));
    let batch = transform_envelope(&contradicted).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(cv.claim_state, ClaimState::Disputed);
            assert_eq!(cv.freshness, ProjectionFreshness::Current);
            assert!(
                matches!(
                    &cv.contradiction_status,
                    ContradictionStatus::PossibleContradiction { description }
                        if description.contains("placebo failed hard")
                ),
                "bridge should derive contradiction status from exported verification summary"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }

    let superseded = make_claim_envelope_with_metadata(serde_json::json!({
        "verification_summary": {
            "lifecycle_state": "superseded",
            "promotion_state": { "state": "not_promoted" },
            "completed_trial_count": 0,
            "passed_refutation_count": 0,
            "failed_refutation_count": 0,
            "notes": []
        }
    }));
    let batch = transform_envelope(&superseded).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(cv.claim_state, ClaimState::Superseded);
            assert_eq!(cv.freshness, ProjectionFreshness::Superseded);
            assert_eq!(cv.contradiction_status, ContradictionStatus::None);
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn brg003_entity_alias_bridge_defaults() {
    // BRG-003: Entity alias defaults are bridge-assigned transform defaults,
    // not exporter-owned truth.
    let records = vec![ExportRecord::EntityAlias(ExportEntityAlias {
        canonical_entity_id: EntityId::new("e1"),
        alias_text: "Entity One".into(),
        alias_source: "forge_extraction".into(),
        match_evidence: None,
        confidence: 0.9,
        scope: None,
        superseded_by_entity_id: Some(EntityId::new("ent-old")),
        split_from_entity_id: Some(EntityId::new("ent-split")),
    })];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();

    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-brg003-alias"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::EntityAlias(alias) => {
            // Bridge-assigned transform defaults (documented for upgrade notes)
            assert_eq!(
                alias.review_state,
                ReviewState::PendingReview,
                "BRG-003: review_state is bridge safety default, not exporter truth"
            );
            assert!(
                !alias.is_human_confirmed,
                "BRG-003: is_human_confirmed is bridge safety default (always false for automated)"
            );
            assert!(
                !alias.is_human_confirmed_final,
                "BRG-003: is_human_confirmed_final is bridge safety default"
            );
            assert!(
                matches!(alias.merge_decision, MergeDecision::PendingReview),
                "BRG-003: merge_decision is bridge safety default, not exporter truth"
            );
            assert_eq!(
                alias.superseded_by_entity_id.as_ref().map(|id| id.as_str()),
                Some("ent-old")
            );
            assert_eq!(
                alias.split_from_entity_id.as_ref().map(|id| id.as_str()),
                Some("ent-split")
            );
        }
        _ => panic!("expected EntityAlias"),
    }
}

#[test]
fn transform_v1_preserves_exporter_supplied_version_ids() {
    let claim_version_id = ClaimVersionId::new("claim-version-explicit");
    let relation_version_id = RelationVersionId::new("relation-version-explicit");
    let records = vec![
        ExportRecord::Claim(ExportClaim {
            claim_id: Some(ClaimId::new("claim-explicit")),
            claim_version_id: Some(claim_version_id.clone()),
            subject_entity_id: EntityId::new("ent-explicit"),
            predicate: "has_type".into(),
            object_anchor: serde_json::json!("module"),
            valid_from: None,
            valid_to: None,
            confidence: 0.9,
            content: "explicit claim".into(),
            projection_family: "forge".into(),
            supersedes_claim_id: None,
            supersedes_claim_version_id: None,
            metadata: None,
        }),
        ExportRecord::Relation(ExportRelation {
            relation_version_id: Some(relation_version_id.clone()),
            subject_entity_id: EntityId::new("ent-explicit"),
            predicate: "depends_on".into(),
            object_anchor: serde_json::json!("ent-other"),
            valid_from: None,
            valid_to: None,
            confidence: 0.8,
            projection_family: "forge".into(),
            source_claim_id: None,
            source_episode_id: None,
            supersedes_relation_version_id: None,
            metadata: None,
        }),
    ];
    let scope = ScopeKey::namespace_only("test");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();
    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-explicit-version-ids"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    };

    let batch = transform_envelope(&env).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(cv.claim_version_id, claim_version_id);
        }
        _ => panic!("expected ClaimVersion"),
    }
    match &batch.records[1] {
        ImportProjectionRecord::RelationVersion(rv) => {
            assert_eq!(rv.relation_version_id, relation_version_id);
        }
        _ => panic!("expected RelationVersion"),
    }
}

#[test]
fn transform_v2_copies_export_meta_and_uses_v2_schema() {
    let records = vec![
        ExportRecord::Claim(ExportClaim {
            claim_id: Some(ClaimId::new("claim-stable")),
            claim_version_id: None,
            subject_entity_id: EntityId::new("ent-stable"),
            predicate: "has_type".into(),
            object_anchor: serde_json::json!("service"),
            valid_from: None,
            valid_to: None,
            confidence: 0.9,
            content: "stable claim".into(),
            projection_family: "forge".into(),
            supersedes_claim_id: None,
            supersedes_claim_version_id: None,
            metadata: None,
        }),
        ExportRecord::Relation(ExportRelation {
            relation_version_id: None,
            subject_entity_id: EntityId::new("ent-stable"),
            predicate: "depends_on".into(),
            object_anchor: serde_json::json!("ent-db"),
            valid_from: None,
            valid_to: None,
            confidence: 0.8,
            projection_family: "forge".into(),
            source_claim_id: None,
            source_episode_id: None,
            supersedes_relation_version_id: None,
            metadata: None,
        }),
    ];
    let scope = ScopeKey::namespace_only("test");
    let export_meta = semantic_memory_forge::ForgeExportMeta {
        authority: semantic_memory_forge::ExportAuthority::Forge,
        run_id: Some("run-v2".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-v2".into()),
        exported_at: "2026-03-09T00:00:00Z".into(),
    };
    let digest =
        ExportEnvelopeV2::compute_digest("forge", &scope, &records, Some(&export_meta), None)
            .unwrap();
    let env = ExportEnvelopeV2 {
        envelope_id: EnvelopeId::new("env-v2-stable"),
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: Some(TraceCtx::generate()),
        exported_at: "2026-03-09T00:00:00Z".into(),
        export_meta: Some(export_meta),
        evidence_bundle: None,
        records,
    };

    let batch = transform_envelope_v2(&env).unwrap();
    assert_eq!(batch.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(
        batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.run_id.as_deref()),
        Some("run-v2")
    );
    assert_eq!(
        batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.comparability_snapshot_version.as_deref()),
        Some("cmp-v2")
    );
}
