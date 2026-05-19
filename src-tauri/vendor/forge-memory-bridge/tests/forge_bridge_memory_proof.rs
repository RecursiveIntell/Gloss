#![allow(clippy::expect_used)]

//! Cross-crate proof test: Forge -> Bridge -> Memory projection pipeline.
//!
//! ## I003 proof
//!
//! Demonstrates that `supersedes_claim_version_id` is `None` when
//! `supersedes_claim_id` is present. The bridge does not synthesize
//! a fake `ClaimVersionId` from a claim-level supersession reference.
//!
//! ## I033 proof
//!
//! End-to-end integration tests proving that the canonical `ExportEnvelopeV3`
//! lane works and the V1/V2 lanes remain compatibility-only with preserved
//! provenance, lineage, and metadata.

#![allow(deprecated)]

use forge_memory_bridge::{
    transform_envelope, transform_envelope_v2, transform_envelope_v3, ClaimState,
    ImportProjectionRecord, ProjectionFreshness, ProjectionImportBatchV1,
    PROJECTION_IMPORT_BATCH_V1_SCHEMA, PROJECTION_IMPORT_BATCH_V2_SCHEMA,
    PROJECTION_IMPORT_BATCH_V3_SCHEMA,
};
use semantic_memory_forge::{
    BilatticeTruthV1, CausalQuestion, CausalRoleHint, ClaimStateV13, ConstraintSeedKind,
    ContradictionWitnessV1, DegradationKindV1, EvidenceAdmissibilityV1, EvidenceBundle,
    EvidenceBundleId, ExactnessLevelV1, ExportAuthority, ExportClaim, ExportConfidenceClass,
    ExportEntityAlias, ExportEnvelopeV1, ExportEnvelopeV2, ExportEnvelopeV3, ExportEpisode,
    ExportEvidenceRef, ExportRecord, ExportRecordSemanticsV3, ExportRecordV3, ExportRelation,
    ForgeExportMeta, NuisanceSnapshot, OutcomeSpec, ProjectionVisibilityClass, PromotionState,
    QualityVectorV1, RetractionRecordV1, SemanticViewV1, SupportExprV1, SupportPolarityV1,
    SupportProvenanceKindV1, SupportSetV1, SupportTokenV1, TreatmentSpec,
    VerificationLifecycleState, VerificationSummary, CLAIM_STATE_V13_SCHEMA,
    CONTRADICTION_WITNESS_V1_SCHEMA, EXPORT_ENVELOPE_V1_SCHEMA, EXPORT_ENVELOPE_V2_SCHEMA,
    EXPORT_ENVELOPE_V3_SCHEMA, RETRACTION_RECORD_V1_SCHEMA, SUPPORT_SET_V1_SCHEMA,
};
use stack_ids::{
    AssertionGroupId, ClaimFamilyId, ClaimId, ClaimStateId, ClaimVersionId, ContradictionGroupId,
    ContradictionWitnessId, EntityId, EnvelopeId, EpisodeId, JointEvidenceGroupId, RelationGroupId,
    RelationVersionId, RetractionRecordId, ScopeKey, SemanticsProfileId, SupportSetId, TraceCtx,
};

/// Build a realistic `ExportEnvelopeV1` with all five record types.
fn build_full_envelope() -> ExportEnvelopeV1 {
    let scope = ScopeKey::namespace_only("proof-ns");
    let trace = TraceCtx::generate();

    let prior_claim_id = ClaimId::new("claim-old-version");
    let prior_rel_ver_id = RelationVersionId::new("rel-ver-old");

    let records = vec![
        // 1. Claim with supersedes_claim_id set
        ExportRecord::Claim(ExportClaim {
            claim_id: Some(ClaimId::new("claim-new")),
            claim_version_id: None,
            subject_entity_id: EntityId::new("entity-rust-lang"),
            predicate: "has_paradigm".into(),
            object_anchor: serde_json::json!("systems programming"),
            valid_from: Some("2026-01-01T00:00:00Z".into()),
            valid_to: None,
            confidence: 0.97,
            content: "Rust is a systems programming language".into(),
            projection_family: "forge_verification".into(),
            supersedes_claim_id: Some(prior_claim_id),
            supersedes_claim_version_id: None,
            metadata: Some(serde_json::json!({"source": "docs"})),
        }),
        // 2. Relation with supersedes_relation_version_id set
        ExportRecord::Relation(ExportRelation {
            relation_version_id: None,
            subject_entity_id: EntityId::new("entity-rust-lang"),
            predicate: "depends_on".into(),
            object_anchor: serde_json::json!("entity-llvm"),
            valid_from: None,
            valid_to: None,
            confidence: 0.85,
            projection_family: "forge_verification".into(),
            source_claim_id: Some(ClaimId::new("claim-new")),
            source_episode_id: None,
            supersedes_relation_version_id: Some(prior_rel_ver_id.clone()),
            metadata: None,
        }),
        // 3. Evidence ref
        ExportRecord::EvidenceRef(ExportEvidenceRef {
            claim_id: ClaimId::new("claim-new"),
            claim_version_id: None,
            fetch_handle: "forge://evidence/run-99/artifact-3".into(),
            source_authority: "forge".into(),
            metadata: Some(serde_json::json!({"format": "json"})),
        }),
        // 4. Entity alias
        ExportRecord::EntityAlias(ExportEntityAlias {
            canonical_entity_id: EntityId::new("entity-rust-lang"),
            alias_text: "Rust programming language".into(),
            alias_source: "forge_extraction".into(),
            match_evidence: Some(serde_json::json!({"score": 0.99})),
            confidence: 0.99,
            scope: None, // inherits envelope scope
            superseded_by_entity_id: Some(EntityId::new("entity-old-rust-lang")),
            split_from_entity_id: Some(EntityId::new("entity-rust-root")),
        }),
        // 5. Episode
        ExportRecord::Episode(ExportEpisode {
            episode_id: Some(EpisodeId::new("ep-verification-42")),
            document_id: "doc-rust-survey-2026".into(),
            cause_ids: vec!["chunk-01".into(), "chunk-02".into()],
            effect_type: "verification".into(),
            outcome: "confirmed".into(),
            confidence: 0.92,
            experiment_id: Some("exp-lang-survey".into()),
            metadata: None,
        }),
    ];

    let digest = ExportEnvelopeV1::compute_digest("forge", &scope, &records)
        .expect("digest computation must succeed");

    ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-proof-001"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key: scope,
        trace_ctx: Some(trace),
        exported_at: "2026-03-08T12:00:00Z".into(),
        records,
    }
}

fn build_canonical_evidence_bundle() -> EvidenceBundle {
    let mut bundle = EvidenceBundle::new(
        CausalQuestion {
            description: "Does the patch preserve the proof fixture?".into(),
            unit_definition: "paired proof run".into(),
        },
        TreatmentSpec {
            description: "apply proof fixture patch".into(),
            baseline_description: "baseline proof fixture".into(),
            paired_trials: true,
        },
        OutcomeSpec {
            description: "proof fixture remains verified".into(),
            measurement_method: "cross crate proof".into(),
            outcome_type: "binary".into(),
        },
        "proof_estimator",
        "1.0.0",
        0.97,
    );
    bundle.id = EvidenceBundleId::new("evidence-proof-001");
    bundle.claim_ids = vec![ClaimId::new("claim-new")];
    bundle.comparability_snapshot_version = Some("cmp-proof-001".into());
    bundle.verification_summary = Some(VerificationSummary {
        lifecycle_state: VerificationLifecycleState::Verified,
        promotion_state: PromotionState::Eligible,
        completed_trial_count: 2,
        passed_refutation_count: 1,
        failed_refutation_count: 0,
        comparability_snapshot_version: Some("cmp-proof-001".into()),
        notes: vec!["paired proof verified".into()],
    });
    bundle.metadata = Some(serde_json::json!({
        "canonical_owner": "semantic_memory_forge::EvidenceBundle",
    }));
    bundle
}

fn build_full_envelope_v2() -> ExportEnvelopeV2 {
    let envelope_v1 = build_full_envelope();
    let evidence_bundle = build_canonical_evidence_bundle();
    let export_meta = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-proof-001".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-proof-001".into()),
        exported_at: envelope_v1.exported_at.clone(),
    };
    let content_digest = ExportEnvelopeV2::compute_digest(
        &envelope_v1.source_authority,
        &envelope_v1.scope_key,
        &envelope_v1.records,
        Some(&export_meta),
        Some(&evidence_bundle),
    )
    .expect("v2 digest computation must succeed");

    ExportEnvelopeV2 {
        envelope_id: envelope_v1.envelope_id,
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest,
        source_authority: envelope_v1.source_authority,
        scope_key: envelope_v1.scope_key,
        trace_ctx: envelope_v1.trace_ctx,
        exported_at: envelope_v1.exported_at,
        export_meta: Some(export_meta),
        evidence_bundle: Some(evidence_bundle),
        records: envelope_v1.records,
    }
}

fn build_full_envelope_v3() -> ExportEnvelopeV3 {
    let envelope_v2 = build_full_envelope_v2();
    let records = envelope_v2
        .records
        .iter()
        .cloned()
        .enumerate()
        .map(|(idx, record)| ExportRecordV3 {
            record,
            semantics: (idx == 0).then(|| ExportRecordSemanticsV3 {
                claim_family_id: Some(ClaimFamilyId::new("claim-family-1")),
                assertion_group_id: Some(AssertionGroupId::new("assertion-group-1")),
                relation_group_id: Some(RelationGroupId::new("relation-group-1")),
                joint_evidence_group_id: Some(JointEvidenceGroupId::new("joint-evidence-1")),
                constraint_seed_kind: Some(ConstraintSeedKind::Hyperedge),
                treatment_hint: Some(CausalRoleHint::Treatment),
                outcome_hint: Some(CausalRoleHint::Outcome),
                confounder_hint: Some(CausalRoleHint::Confounder),
                instrument_hint: None,
                effect_modifier_hint: None,
                contradiction_candidate_group_id: Some(ContradictionGroupId::new(
                    "contradiction-group-1",
                )),
                mutual_exclusion_group_id: None,
                comparability_snapshot_version: Some("cmp-proof-001".into()),
                nuisance_snapshot: Some(NuisanceSnapshot {
                    environment_fingerprint: Some("env-fingerprint-1".into()),
                    toolchain_version: Some("rustc-1.85.0".into()),
                    dependency_set_hash: Some("deps-1".into()),
                    scope_mismatch_markers: vec![],
                    measurement_notes: vec!["paired measurement".into()],
                    selection_bias_markers: vec!["reviewed sample".into()],
                }),
                projection_visibility_class: ProjectionVisibilityClass::Standard,
                export_confidence_class: ExportConfidenceClass::Verified,
                derivation_seed_ids: vec!["seed-1".into()],
                review_priority_hint: Some("high".into()),
            }),
        })
        .collect::<Vec<_>>();

    let content_digest = ExportEnvelopeV3::compute_digest(
        &envelope_v2.source_authority,
        &envelope_v2.scope_key,
        &records,
        envelope_v2.export_meta.as_ref(),
        envelope_v2.evidence_bundle.as_ref(),
    )
    .expect("v3 digest computation must succeed");
    let support_sets = vec![SupportSetV1 {
        schema_version: SUPPORT_SET_V1_SCHEMA.into(),
        support_set_id: SupportSetId::new("support-set-7"),
        claim_id: ClaimId::new("claim-new"),
        semantics_profile_id: SemanticsProfileId::new("semantics-profile-v13"),
        support_tokens: vec![
            SupportTokenV1 {
                token_id: "tok-a".into(),
                kind: SupportProvenanceKindV1::EvidenceRef,
                reference: "evidence-ref:abc".into(),
                polarity: SupportPolarityV1::Supports,
            },
            SupportTokenV1 {
                token_id: "tok-b".into(),
                kind: SupportProvenanceKindV1::ClaimVersion,
                reference: "claim-version:legacy-9".into(),
                polarity: SupportPolarityV1::Refutes,
            },
        ],
        support_expr: SupportExprV1::AnyOf {
            children: vec![
                SupportExprV1::Token {
                    token_id: "tok-a".into(),
                },
                SupportExprV1::Token {
                    token_id: "tok-b".into(),
                },
            ],
        },
        content_digest: stack_ids::ContentDigest::compute(b"support-set-7"),
    }];
    let contradiction_witnesses = vec![ContradictionWitnessV1 {
        schema_version: CONTRADICTION_WITNESS_V1_SCHEMA.into(),
        contradiction_witness_id: ContradictionWitnessId::new("contradiction-witness-9"),
        claim_id: ClaimId::new("claim-new"),
        conflicting_token_ids: vec!["tok-a".into(), "tok-b".into()],
        summary: Some("one source supports while another refutes".into()),
    }];
    let retraction_records = vec![RetractionRecordV1 {
        schema_version: RETRACTION_RECORD_V1_SCHEMA.into(),
        retraction_record_id: RetractionRecordId::new("retraction-record-1"),
        claim_id: ClaimId::new("claim-new"),
        retracted_claim_version_id: ClaimVersionId::new("claim-version-5"),
        superseded_by_claim_version_id: Some(ClaimVersionId::new("claim-version-6")),
        effective_recorded_at: "2026-03-14T12:05:00Z".into(),
        reason: "refuter collapsed the estimated effect to approximately zero".into(),
        cascade_required: true,
        delta_summary: Some("close tx interval and recompute downstream views".into()),
    }];
    let claim_states_v13 = vec![ClaimStateV13 {
        schema_version: CLAIM_STATE_V13_SCHEMA.into(),
        claim_state_id: ClaimStateId::new("claim-state-123"),
        claim_id: ClaimId::new("claim-new"),
        claim_version_id: Some(ClaimVersionId::new("claim-version-6")),
        semantics_profile_id: SemanticsProfileId::new("semantics-profile-v13"),
        view: SemanticViewV1::Canonical,
        bilattice_truth: BilatticeTruthV1::Both,
        support_set_id: Some(SupportSetId::new("support-set-7")),
        support_set_digest: Some(stack_ids::ContentDigest::compute(b"support-set-7")),
        quality_vector: QualityVectorV1 {
            exactness: ExactnessLevelV1::Conservative,
            degradation: vec![DegradationKindV1::ExactnessDowngraded],
            freshness: Some("current".into()),
            replay_limited: false,
            execution_contaminated: true,
        },
        evidence_admissibility: EvidenceAdmissibilityV1::Admissible,
        contradiction_witness_id: Some(ContradictionWitnessId::new("contradiction-witness-9")),
        valid_from: Some("2026-03-01T00:00:00Z".into()),
        valid_to: None,
        tx_from: "2026-03-14T12:05:00Z".into(),
        tx_to: None,
        proof_obligations_remaining: vec!["run exact bounded oracle".into()],
        policy_action_allowed: false,
    }];
    let content_digest = ExportEnvelopeV3::compute_digest_with_v13(
        content_digest,
        &support_sets,
        &contradiction_witnesses,
        &retraction_records,
        &claim_states_v13,
    )
    .expect("v3 digest computation with v13 artifacts must succeed");
    let decision_traces_v14 = vec![serde_json::json!({
        "schema_version": "decision_trace_v1",
        "decision_trace_id": "decision-trace-1",
        "selected_decision": "promote"
    })];
    let remote_slice_results_v15 = vec![serde_json::json!({
        "schema_version": "remote_slice_result_v1",
        "remote_slice_result_id": "remote-slice-result-1",
        "local_admission_recommendation": "admit_with_disclosure_constraints"
    })];
    let content_digest = ExportEnvelopeV3::compute_digest_with_endgame(
        content_digest,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &decision_traces_v14,
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &[],
        &remote_slice_results_v15,
        &[],
        &[],
        &[],
        &[],
    )
    .expect("v3 digest computation with endgame artifacts must succeed");

    ExportEnvelopeV3 {
        envelope_id: envelope_v2.envelope_id,
        schema_version: EXPORT_ENVELOPE_V3_SCHEMA.into(),
        content_digest,
        source_authority: envelope_v2.source_authority,
        scope_key: envelope_v2.scope_key,
        trace_ctx: envelope_v2.trace_ctx,
        exported_at: envelope_v2.exported_at,
        export_meta: envelope_v2.export_meta,
        evidence_bundle: envelope_v2.evidence_bundle,
        support_sets,
        contradiction_witnesses,
        retraction_records,
        claim_states_v13,
        intervention_bundles_v14: vec![],
        outcome_schemas_v14: vec![],
        cohort_contracts_v14: vec![],
        counterfactual_slices_v14: vec![],
        experiment_cases_v14: vec![],
        comparability_matrices_v14: vec![],
        decision_traces_v14,
        refuter_suites_v14: vec![],
        refuter_results_v14: vec![],
        experiment_budgets_v14: vec![],
        rollout_decisions_v14: vec![],
        rollback_decisions_v14: vec![],
        attestation_envelopes_v15: vec![],
        trust_root_sets_v15: vec![],
        artifact_admission_policies_v15: vec![],
        transparency_receipts_v15: vec![],
        attestation_revocations_v15: vec![],
        attestation_supersessions_v15: vec![],
        remote_oracle_leases_v15: vec![],
        remote_slice_requests_v15: vec![],
        remote_slice_results_v15,
        cross_runtime_replay_tickets_v15: vec![],
        dispute_bundles_v15: vec![],
        disclosure_policies_v15: vec![],
        disclosure_budgets_v15: vec![],
        records,
    }
}

#[test]
fn compat_v2_transform_preserves_records_and_v2_provenance() {
    let envelope = build_full_envelope_v2();
    let batch = transform_envelope_v2(&envelope).expect("v2 transform must succeed");

    assert_eq!(
        batch.records.len(),
        5,
        "v2 batch must contain all record types"
    );
    assert_eq!(batch.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_eq!(
        batch.export_schema_version.as_deref(),
        Some(EXPORT_ENVELOPE_V2_SCHEMA),
        "compatibility V2 path must preserve the source export schema version"
    );
    assert_eq!(
        batch
            .export_meta
            .as_ref()
            .and_then(|meta| meta.run_id.as_deref()),
        Some("run-proof-001")
    );
    assert_eq!(
        batch
            .evidence_bundle
            .as_ref()
            .map(|bundle| bundle.id.as_str()),
        Some("evidence-proof-001"),
        "compatibility V2 should still preserve the canonical evidence bundle payload verbatim"
    );
    assert_eq!(
        batch
            .episode_bundle
            .as_ref()
            .map(|bundle| bundle.episode_id.as_str()),
        Some("ep-verification-42")
    );
    assert!(batch.execution_context.is_some());
    assert!(matches!(
        &batch.records[0],
        ImportProjectionRecord::ClaimVersion(_)
    ));
}

#[test]
fn canonical_v3_transform_preserves_rich_semantics_without_invention() {
    let envelope = build_full_envelope_v3();
    let batch = transform_envelope_v3(&envelope).expect("v3 transform must succeed");

    assert_eq!(batch.schema_version, PROJECTION_IMPORT_BATCH_V3_SCHEMA);
    assert_eq!(
        batch.export_schema_version.as_deref(),
        Some(EXPORT_ENVELOPE_V3_SCHEMA)
    );
    assert_eq!(batch.records.len(), 5);

    let first = &batch.records[0];
    let semantics = first
        .semantics
        .as_ref()
        .expect("first record should retain exported semantics");
    assert_eq!(
        semantics.claim_family_id.as_ref().map(|id| id.as_str()),
        Some("claim-family-1")
    );
    assert_eq!(
        semantics.relation_group_id.as_ref().map(|id| id.as_str()),
        Some("relation-group-1")
    );
    assert_eq!(
        semantics
            .contradiction_candidate_group_id
            .as_ref()
            .map(|id| id.as_str()),
        Some("contradiction-group-1")
    );

    assert!(
        batch.records[1].semantics.is_none(),
        "bridge must preserve absent semantics as absent rather than invent them"
    );
    assert_eq!(
        batch
            .evidence_bundle
            .as_ref()
            .map(|bundle| bundle.id.as_str()),
        Some("evidence-proof-001"),
        "canonical V3 must carry the canonical evidence bundle alongside rich semantics"
    );
    assert_eq!(
        batch
            .episode_bundle
            .as_ref()
            .map(|bundle| bundle.episode_id.as_str()),
        Some("ep-verification-42")
    );
    assert!(batch.execution_context.is_some());
}

#[test]
fn canonical_v3_transform_preserves_v13_claim_algebra_artifacts_verbatim() {
    let envelope = build_full_envelope_v3();
    let batch = transform_envelope_v3(&envelope).expect("v3 transform must succeed");

    assert_eq!(batch.support_sets, envelope.support_sets);
    assert_eq!(
        batch.contradiction_witnesses,
        envelope.contradiction_witnesses
    );
    assert_eq!(batch.retraction_records, envelope.retraction_records);
    assert_eq!(batch.claim_states_v13, envelope.claim_states_v13);
}

#[test]
fn canonical_v3_transform_preserves_endgame_artifact_vectors_verbatim() {
    let envelope = build_full_envelope_v3();
    let batch = transform_envelope_v3(&envelope).expect("v3 transform must succeed");

    assert_eq!(batch.decision_traces_v14, envelope.decision_traces_v14);
    assert_eq!(
        batch.remote_slice_results_v15,
        envelope.remote_slice_results_v15
    );
}

#[test]
fn canonical_v3_transform_rejects_missing_episode_id_for_bundle_lane() {
    let mut envelope = build_full_envelope_v3();
    for record in &mut envelope.records {
        if let ExportRecord::Episode(episode) = &mut record.record {
            episode.episode_id = None;
        }
    }
    envelope.content_digest = ExportEnvelopeV3::compute_digest_with_v13(
        ExportEnvelopeV3::compute_digest(
            &envelope.source_authority,
            &envelope.scope_key,
            &envelope.records,
            envelope.export_meta.as_ref(),
            envelope.evidence_bundle.as_ref(),
        )
        .unwrap(),
        &envelope.support_sets,
        &envelope.contradiction_witnesses,
        &envelope.retraction_records,
        &envelope.claim_states_v13,
    )
    .unwrap();
    envelope.content_digest = ExportEnvelopeV3::compute_digest_with_endgame(
        envelope.content_digest.clone(),
        &envelope.intervention_bundles_v14,
        &envelope.outcome_schemas_v14,
        &envelope.cohort_contracts_v14,
        &envelope.counterfactual_slices_v14,
        &envelope.experiment_cases_v14,
        &envelope.comparability_matrices_v14,
        &envelope.decision_traces_v14,
        &envelope.refuter_suites_v14,
        &envelope.refuter_results_v14,
        &envelope.experiment_budgets_v14,
        &envelope.rollout_decisions_v14,
        &envelope.rollback_decisions_v14,
        &envelope.attestation_envelopes_v15,
        &envelope.trust_root_sets_v15,
        &envelope.artifact_admission_policies_v15,
        &envelope.transparency_receipts_v15,
        &envelope.attestation_revocations_v15,
        &envelope.attestation_supersessions_v15,
        &envelope.remote_oracle_leases_v15,
        &envelope.remote_slice_requests_v15,
        &envelope.remote_slice_results_v15,
        &envelope.cross_runtime_replay_tickets_v15,
        &envelope.dispute_bundles_v15,
        &envelope.disclosure_policies_v15,
        &envelope.disclosure_budgets_v15,
    )
    .unwrap();

    let err = transform_envelope_v3(&envelope).expect_err("missing episode_id must fail");
    assert_eq!(err.kind(), "missing_episode_identity");
}

#[test]
fn brg001_v2_transform_is_compatibility_only_shape() {
    let envelope = build_full_envelope_v2();
    let batch = transform_envelope_v2(&envelope).expect("v2 transform must succeed");

    assert_eq!(batch.schema_version, PROJECTION_IMPORT_BATCH_V2_SCHEMA);
    assert_ne!(
        batch.schema_version, PROJECTION_IMPORT_BATCH_V3_SCHEMA,
        "compatibility V2 transform must not be the canonical schema"
    );
}

#[test]
fn brg001_canonical_v3_path_is_normal_default_and_legacy_paths_are_explicit() {
    let envelope_v1 = build_full_envelope();
    let envelope_v2: ExportEnvelopeV2 = envelope_v1.clone().into();
    let envelope_v3 = ExportEnvelopeV3::try_from(envelope_v2.clone())
        .expect("compatibility upgrade to V3 should remain supported");

    let canonical_batch = transform_envelope_v3(&envelope_v3).unwrap();
    let legacy_batch_v1 = transform_envelope(&envelope_v1).unwrap();
    let legacy_batch_v2 = transform_envelope_v2(&envelope_v2).unwrap();

    // Canonical path for normal imports
    assert_eq!(
        canonical_batch.schema_version,
        PROJECTION_IMPORT_BATCH_V3_SCHEMA
    );
    assert_eq!(
        canonical_batch.export_schema_version.as_deref(),
        Some(EXPORT_ENVELOPE_V3_SCHEMA),
        "canonical path preserves the source V3 export vocabulary separately"
    );
    assert!(
        canonical_batch
            .records
            .iter()
            .any(|record| record.semantics.is_some()),
        "V3 canonical batches retain enriched kernel-semantics when available"
    );
    assert_eq!(canonical_batch.records.len(), legacy_batch_v1.records.len());
    assert_eq!(canonical_batch.records.len(), legacy_batch_v2.records.len());

    // Compatibility-only paths remain migration lanes and are still callable explicitly
    assert_eq!(
        legacy_batch_v1.schema_version,
        PROJECTION_IMPORT_BATCH_V1_SCHEMA
    );
    assert_eq!(
        legacy_batch_v1
            .export_schema_version
            .as_deref()
            .expect("legacy V1 batch keeps source schema"),
        EXPORT_ENVELOPE_V1_SCHEMA
    );
    assert_eq!(
        legacy_batch_v2.schema_version,
        PROJECTION_IMPORT_BATCH_V2_SCHEMA
    );
    assert_eq!(
        legacy_batch_v2
            .export_schema_version
            .as_deref()
            .expect("legacy V2 batch keeps source schema"),
        EXPORT_ENVELOPE_V2_SCHEMA
    );
}

#[test]
fn transform_full_envelope_produces_all_record_types() {
    let envelope = build_full_envelope();
    let batch =
        transform_envelope(&envelope).expect("transform must succeed for a valid full envelope");

    // All 5 records present
    assert_eq!(
        batch.records.len(),
        5,
        "batch must contain all 5 record types"
    );

    // Verify each record type is present
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
    assert!(matches!(
        &batch.records[3],
        ImportProjectionRecord::EntityAlias(_)
    ));
    assert!(matches!(
        &batch.records[4],
        ImportProjectionRecord::Episode(_)
    ));
}

#[test]
fn i003_supersedes_claim_version_id_is_none() {
    // I003 proof: The bridge must NOT mint a synthetic ClaimVersionId when
    // the export only carries supersedes_claim_id (claim-level lineage).
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert!(
                cv.supersedes_claim_version_id.is_none(),
                "I003: supersedes_claim_version_id MUST be None when the export \
                 schema only carries supersedes_claim_id (no fake lineage)"
            );
        }
        other => panic!("expected ClaimVersion, got {other:?}"),
    }
}

#[test]
fn supersedes_relation_version_id_is_preserved() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    match &batch.records[1] {
        ImportProjectionRecord::RelationVersion(rv) => {
            assert_eq!(
                rv.supersedes_relation_version_id,
                Some(RelationVersionId::new("rel-ver-old")),
                "relation supersession lineage must be preserved verbatim"
            );
        }
        other => panic!("expected RelationVersion, got {other:?}"),
    }
}

#[test]
fn provenance_propagates_correctly() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    // source_envelope_id
    assert_eq!(
        batch.source_envelope_id,
        EnvelopeId::new("env-proof-001"),
        "source_envelope_id must propagate from the envelope"
    );

    // source_authority
    assert_eq!(
        batch.source_authority, "forge",
        "source_authority must propagate"
    );

    assert_eq!(
        batch.schema_version, PROJECTION_IMPORT_BATCH_V1_SCHEMA,
        "bridge batches must advertise the import schema separately"
    );
    assert_eq!(
        batch.export_schema_version.as_deref(),
        Some(EXPORT_ENVELOPE_V1_SCHEMA),
        "source export schema must be preserved as provenance"
    );

    // trace_ctx
    assert!(
        batch.trace_ctx.is_some(),
        "trace_ctx must be carried through"
    );
    assert_eq!(
        batch.trace_ctx.as_ref().unwrap().trace_id,
        envelope.trace_ctx.as_ref().unwrap().trace_id,
        "trace_id must be identical to envelope trace_id"
    );
}

#[test]
fn bridge_batch_carries_transform_provenance_only() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    let transformed_at = &batch.transformed_at;
    assert!(!transformed_at.is_empty());
    assert_eq!(batch.source_exported_at, envelope.exported_at);
}

#[test]
fn real_claim_version_lineage_is_preserved_when_present() {
    let mut envelope = build_full_envelope();
    let prior_version = stack_ids::ClaimVersionId::new("claim-old-version-v7");
    match &mut envelope.records[0] {
        ExportRecord::Claim(claim) => {
            claim.supersedes_claim_version_id = Some(prior_version.clone());
        }
        other => panic!("expected claim record, got {other:?}"),
    }
    envelope.content_digest = ExportEnvelopeV1::compute_digest(
        &envelope.source_authority,
        &envelope.scope_key,
        &envelope.records,
    )
    .unwrap();

    let batch = transform_envelope(&envelope).unwrap();
    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(cv.supersedes_claim_version_id, Some(prior_version));
        }
        _ => panic!("expected ClaimVersion"),
    }
}

#[test]
fn freshness_is_current_and_claim_state_is_active() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    match &batch.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert_eq!(
                cv.freshness,
                ProjectionFreshness::Current,
                "freshly imported claims must have Current freshness"
            );
            assert_eq!(
                cv.claim_state,
                ClaimState::Active,
                "freshly imported claims must have Active state"
            );
        }
        _ => panic!("expected ClaimVersion"),
    }

    match &batch.records[1] {
        ImportProjectionRecord::RelationVersion(rv) => {
            assert_eq!(
                rv.freshness,
                ProjectionFreshness::Current,
                "freshly imported relations must have Current freshness"
            );
        }
        _ => panic!("expected RelationVersion"),
    }
}

#[test]
fn evidence_ref_preserves_claim_id_and_fetch_handle() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    match &batch.records[2] {
        ImportProjectionRecord::EvidenceRef(er) => {
            assert_eq!(
                er.claim_id,
                ClaimId::new("claim-new"),
                "evidence ref must preserve claim_id"
            );
            assert_eq!(
                er.fetch_handle, "forge://evidence/run-99/artifact-3",
                "evidence ref must preserve fetch_handle"
            );
        }
        other => panic!("expected EvidenceRef, got {other:?}"),
    }
}

#[test]
fn alias_preserves_canonical_entity_id_and_alias_text() {
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    match &batch.records[3] {
        ImportProjectionRecord::EntityAlias(ea) => {
            assert_eq!(
                ea.canonical_entity_id,
                EntityId::new("entity-rust-lang"),
                "alias must preserve canonical_entity_id"
            );
            assert_eq!(
                ea.alias_text, "Rust programming language",
                "alias must preserve alias_text"
            );
        }
        other => panic!("expected EntityAlias, got {other:?}"),
    }
}

#[test]
fn batch_serializes_and_deserializes_to_json() {
    // Wire format proof: the batch must survive a JSON round-trip.
    let envelope = build_full_envelope();
    let batch = transform_envelope(&envelope).unwrap();

    let json = serde_json::to_string_pretty(&batch).expect("batch must serialize to JSON");

    let deserialized: ProjectionImportBatchV1 =
        serde_json::from_str(&json).expect("batch must deserialize from JSON");

    // Structural equality checks on the round-tripped batch
    assert_eq!(deserialized.source_envelope_id, batch.source_envelope_id);
    assert_eq!(deserialized.source_authority, batch.source_authority);
    assert_eq!(deserialized.schema_version, batch.schema_version);
    assert_eq!(
        deserialized.export_schema_version,
        batch.export_schema_version
    );
    assert_eq!(deserialized.content_digest, batch.content_digest);
    assert_eq!(deserialized.scope_key, batch.scope_key);
    assert_eq!(deserialized.trace_ctx, batch.trace_ctx);
    assert_eq!(deserialized.transformed_at, batch.transformed_at);
    assert_eq!(deserialized.source_exported_at, batch.source_exported_at);
    assert_eq!(deserialized.records.len(), batch.records.len());

    // Verify the claim round-trips correctly including supersession
    match &deserialized.records[0] {
        ImportProjectionRecord::ClaimVersion(cv) => {
            assert!(cv.supersedes_claim_version_id.is_none());
            assert_eq!(cv.claim_state, ClaimState::Active);
            assert_eq!(cv.freshness, ProjectionFreshness::Current);
        }
        _ => panic!("expected ClaimVersion after round-trip"),
    }

    // Verify the relation round-trips correctly including supersession
    match &deserialized.records[1] {
        ImportProjectionRecord::RelationVersion(rv) => {
            assert_eq!(
                rv.supersedes_relation_version_id,
                Some(RelationVersionId::new("rel-ver-old"))
            );
        }
        _ => panic!("expected RelationVersion after round-trip"),
    }
}
