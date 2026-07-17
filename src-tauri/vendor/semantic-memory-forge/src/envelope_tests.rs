use super::*;
use proptest::prelude::*;

fn lift_records_to_v3(records: &[ExportRecord]) -> Vec<ExportRecordV3> {
    records
        .iter()
        .cloned()
        .map(|record| ExportRecordV3 {
            record,
            semantics: None,
        })
        .collect()
}
use crate::bundle::{CausalQuestion, OutcomeSpec, TreatmentSpec};

fn make_test_envelope() -> ExportEnvelopeV1 {
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: None,
        claim_version_id: None,
        subject_entity_id: EntityId::new("ent-1"),
        predicate: "has_type".into(),
        object_anchor: serde_json::json!("function"),
        valid_from: None,
        valid_to: None,
        confidence: 0.95,
        content: "Entity ent-1 is a function".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: None,
    })];
    let scope_key = ScopeKey::namespace_only("test-ns");
    let digest = ExportEnvelopeV1::compute_digest("forge", &scope_key, &records).unwrap();

    ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-001"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records,
    }
}

fn make_test_evidence_bundle() -> EvidenceBundle {
    let mut bundle = EvidenceBundle::new(
        CausalQuestion {
            description: "Does patch X improve outcome Y?".into(),
            unit_definition: "patch".into(),
        },
        TreatmentSpec {
            description: "apply patch".into(),
            baseline_description: "baseline".into(),
            paired_trials: true,
        },
        OutcomeSpec {
            description: "tests pass".into(),
            measurement_method: "integration suite".into(),
            outcome_type: "binary".into(),
        },
        "diff_in_diff",
        "1.0.0",
        0.4,
    );
    bundle.id = crate::bundle::EvidenceBundleId::new("bundle-test-001");
    bundle.created_at = "2026-03-07T00:00:00Z".into();
    bundle
}

#[test]
fn export_authority_display() {
    assert_eq!(ExportAuthority::Forge.as_str(), "forge");
    assert_eq!(
        ExportAuthority::External {
            name: "manual".into()
        }
        .as_str(),
        "manual"
    );
}

#[test]
fn forge_export_meta_serde() {
    let meta = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-123".into()),
        direct_write: false,
        comparability_snapshot_version: Some("v1".into()),
        exported_at: "2024-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&meta).unwrap();
    let back: ForgeExportMeta = serde_json::from_str(&json).unwrap();
    assert!(!back.direct_write);
}

#[test]
fn valid_envelope_passes_validation() {
    let env = make_test_envelope();
    env.validate().unwrap();
}

#[test]
fn empty_envelope_id_rejected() {
    assert!(EnvelopeId::try_new("").is_err());
}

#[test]
fn wrong_schema_version_rejected() {
    let mut env = make_test_envelope();
    env.schema_version = "wrong_version".into();
    assert!(env.validate().is_err());
}

#[test]
fn empty_records_rejected() {
    let scope_key = ScopeKey::namespace_only("test");
    let digest = ContentDigest::compute(b"empty");
    let env = ExportEnvelopeV1 {
        envelope_id: EnvelopeId::new("env-002"),
        schema_version: EXPORT_ENVELOPE_V1_SCHEMA.into(),
        content_digest: digest,
        source_authority: "forge".into(),
        scope_key,
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        records: vec![],
    };
    assert!(env.validate().is_err());
}

#[test]
fn tampered_digest_rejected() {
    let mut env = make_test_envelope();
    env.content_digest = ContentDigest::compute(b"tampered");
    let err = env.validate().unwrap_err();
    assert!(matches!(err, ExportEnvelopeError::DigestMismatch { .. }));
}

#[test]
fn digest_is_deterministic() {
    let scope = ScopeKey::namespace_only("ns");
    let records = vec![ExportRecord::Claim(ExportClaim {
        claim_id: None,
        claim_version_id: None,
        subject_entity_id: EntityId::new("e1"),
        predicate: "p".into(),
        object_anchor: serde_json::json!("v"),
        valid_from: None,
        valid_to: None,
        confidence: 1.0,
        content: "test".into(),
        projection_family: "f".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: None,
    })];
    let d1 = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();
    let d2 = ExportEnvelopeV1::compute_digest("forge", &scope, &records).unwrap();
    assert_eq!(d1, d2);
}

#[test]
fn valid_v2_envelope_passes_validation() {
    let v1 = make_test_envelope();
    let env = ExportEnvelopeV2 {
        envelope_id: v1.envelope_id,
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: ExportEnvelopeV2::compute_digest(
            &v1.source_authority,
            &v1.scope_key,
            &v1.records,
            None,
            None,
        )
        .unwrap(),
        source_authority: v1.source_authority,
        scope_key: v1.scope_key,
        trace_ctx: v1.trace_ctx,
        exported_at: v1.exported_at,
        export_meta: None,
        evidence_bundle: None,
        records: v1.records,
    };

    env.validate().unwrap();
}

#[test]
fn v2_envelope_with_bundle_passes_validation() {
    let v1 = make_test_envelope();
    let evidence_bundle = make_test_evidence_bundle();
    let digest = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        None,
        Some(&evidence_bundle),
    )
    .unwrap();
    let env = ExportEnvelopeV2 {
        envelope_id: v1.envelope_id,
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: digest,
        source_authority: v1.source_authority,
        scope_key: v1.scope_key,
        trace_ctx: v1.trace_ctx,
        exported_at: v1.exported_at,
        export_meta: None,
        evidence_bundle: Some(evidence_bundle),
        records: v1.records,
    };

    env.validate().unwrap();
}

#[test]
fn v2_digest_changes_when_bundle_changes() {
    let v1 = make_test_envelope();
    let bundle_a = make_test_evidence_bundle();
    let mut bundle_b = make_test_evidence_bundle();
    bundle_b.estimate = bundle_a.estimate + 0.1;

    let digest_a = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        None,
        Some(&bundle_a),
    )
    .unwrap();
    let digest_b = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        None,
        Some(&bundle_b),
    )
    .unwrap();

    assert_ne!(digest_a, digest_b);
}

#[test]
fn v2_digest_ignores_bundle_created_at_metadata() {
    let v1 = make_test_envelope();
    let bundle_a = make_test_evidence_bundle();
    let mut bundle_b = bundle_a.clone();
    bundle_b.created_at = "2030-01-01T00:00:00Z".into();

    let digest_a = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        None,
        Some(&bundle_a),
    )
    .unwrap();
    let digest_b = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        None,
        Some(&bundle_b),
    )
    .unwrap();

    assert_eq!(digest_a, digest_b);
}

#[test]
fn v2_digest_ignores_export_meta_exported_at_metadata() {
    let v1 = make_test_envelope();
    let meta_a = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-1".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-1".into()),
        exported_at: "2026-03-07T00:00:00Z".into(),
    };
    let meta_b = ForgeExportMeta {
        exported_at: "2030-01-01T00:00:00Z".into(),
        ..meta_a.clone()
    };

    let digest_a = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        Some(&meta_a),
        None,
    )
    .unwrap();
    let digest_b = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        Some(&meta_b),
        None,
    )
    .unwrap();

    assert_eq!(digest_a, digest_b);
}

#[test]
fn v3_digest_changes_when_bundle_changes() {
    let v1 = make_test_envelope();
    let records = lift_records_to_v3(&v1.records);
    let bundle_a = make_test_evidence_bundle();
    let mut bundle_b = make_test_evidence_bundle();
    bundle_b.estimate = bundle_a.estimate + 0.1;

    let digest_a = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        None,
        Some(&bundle_a),
    )
    .unwrap();
    let digest_b = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        None,
        Some(&bundle_b),
    )
    .unwrap();

    assert_ne!(digest_a, digest_b);
}

#[test]
fn v3_digest_ignores_bundle_created_at_metadata() {
    let v1 = make_test_envelope();
    let records = lift_records_to_v3(&v1.records);
    let bundle_a = make_test_evidence_bundle();
    let mut bundle_b = bundle_a.clone();
    bundle_b.created_at = "2030-01-01T00:00:00Z".into();

    let digest_a = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        None,
        Some(&bundle_a),
    )
    .unwrap();
    let digest_b = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        None,
        Some(&bundle_b),
    )
    .unwrap();

    assert_eq!(digest_a, digest_b);
}

#[test]
fn v3_digest_ignores_export_meta_exported_at_metadata() {
    let v1 = make_test_envelope();
    let records = lift_records_to_v3(&v1.records);
    let meta_a = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-1".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-1".into()),
        exported_at: "2026-03-07T00:00:00Z".into(),
    };
    let meta_b = ForgeExportMeta {
        exported_at: "2030-01-01T00:00:00Z".into(),
        ..meta_a.clone()
    };

    let digest_a = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        Some(&meta_a),
        None,
    )
    .unwrap();
    let digest_b = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &records,
        Some(&meta_b),
        None,
    )
    .unwrap();

    assert_eq!(digest_a, digest_b);
}

#[test]
fn v2_digest_matches_pinned_golden_fixture() {
    let v1 = make_test_envelope();
    let bundle = make_test_evidence_bundle();
    let meta = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-1".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-1".into()),
        exported_at: "2026-03-07T00:00:00Z".into(),
    };

    let digest = ExportEnvelopeV2::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &v1.records,
        Some(&meta),
        Some(&bundle),
    )
    .unwrap();

    assert_eq!(
        digest.hex(),
        "1ba7068f34d138d76eae89eaca3f3a98940b7314ea3f0e1ab4e518a539082680"
    );
}

#[test]
fn v3_digest_matches_pinned_golden_fixture() {
    let v1 = make_test_envelope();
    let bundle = make_test_evidence_bundle();
    let meta = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-1".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-1".into()),
        exported_at: "2026-03-07T00:00:00Z".into(),
    };

    let digest = ExportEnvelopeV3::compute_digest(
        &v1.source_authority,
        &v1.scope_key,
        &lift_records_to_v3(&v1.records),
        Some(&meta),
        Some(&bundle),
    )
    .unwrap();

    assert_eq!(
        digest.hex(),
        "6e44bd7d6ccafd2046f51c826899f09b81f29feb621c46c0781d67b0bee58fea"
    );
}

#[test]
fn v2_and_v3_digest_ordering_rules_match() {
    let scope = ScopeKey::namespace_only("ns-ordering");
    let first = ExportRecord::Claim(ExportClaim {
        claim_id: None,
        claim_version_id: Some(ClaimVersionId::new("claim-order-1")),
        subject_entity_id: EntityId::new("e1"),
        predicate: "supports".into(),
        object_anchor: serde_json::json!("a"),
        valid_from: None,
        valid_to: None,
        confidence: 1.0,
        content: "first".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: None,
    });
    let second = ExportRecord::Claim(ExportClaim {
        claim_id: None,
        claim_version_id: Some(ClaimVersionId::new("claim-order-2")),
        subject_entity_id: EntityId::new("e2"),
        predicate: "supports".into(),
        object_anchor: serde_json::json!("b"),
        valid_from: None,
        valid_to: None,
        confidence: 1.0,
        content: "second".into(),
        projection_family: "forge_verification".into(),
        supersedes_claim_id: None,
        supersedes_claim_version_id: None,
        metadata: None,
    });
    let records = vec![first.clone(), second.clone()];
    let reversed = vec![second, first];
    let v3_records = lift_records_to_v3(&records);
    let reversed_v3 = lift_records_to_v3(&reversed);

    let v2_forward =
        ExportEnvelopeV2::compute_digest("forge", &scope, &records, None, None).unwrap();
    let v2_reverse =
        ExportEnvelopeV2::compute_digest("forge", &scope, &reversed, None, None).unwrap();
    let v3_forward =
        ExportEnvelopeV3::compute_digest("forge", &scope, &v3_records, None, None).unwrap();
    let v3_reverse =
        ExportEnvelopeV3::compute_digest("forge", &scope, &reversed_v3, None, None).unwrap();

    assert_ne!(v2_forward, v2_reverse);
    assert_ne!(v3_forward, v3_reverse);
}

#[test]
fn enrich_to_v3_degrades_unknown_enum_without_failing() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    if let ExportRecord::Claim(claim) = &mut v2.records[0] {
        claim.metadata = Some(serde_json::json!({
            "kernel_semantics_v3": {
                "constraint_seed_kind": "not_a_constraint_kind",
                "treatment_hint": "treatment",
                "projection_visibility_class": "standard",
                "export_confidence_class": "verified",
                "derivation_seed_ids": ["seed-1"]
            }
        }));
    }

    v2.content_digest = ExportEnvelopeV2::compute_digest(
        &v2.source_authority,
        &v2.scope_key,
        &v2.records,
        v2.export_meta.as_ref(),
        v2.evidence_bundle.as_ref(),
    )
    .unwrap();

    let v3 = v2.enrich_to_v3().unwrap();
    let semantics = v3.records[0]
        .semantics
        .as_ref()
        .expect("semantics should still lift");

    assert_eq!(semantics.constraint_seed_kind, None);
    assert_eq!(semantics.treatment_hint, Some(CausalRoleHint::Treatment));
    assert_eq!(
        semantics.projection_visibility_class,
        ProjectionVisibilityClass::Standard
    );
    assert_eq!(
        semantics.export_confidence_class,
        ExportConfidenceClass::Verified
    );
    assert_eq!(semantics.derivation_seed_ids, vec!["seed-1".to_string()]);
}

#[test]
fn enrich_to_v3_degrades_invalid_nested_object_without_failing() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    if let ExportRecord::Claim(claim) = &mut v2.records[0] {
        claim.metadata = Some(serde_json::json!({
            "kernel_semantics_v3": {
                "nuisance_snapshot": "must_be_object",
                "projection_visibility_class": "standard",
                "export_confidence_class": "verified"
            }
        }));
    }

    v2.content_digest = ExportEnvelopeV2::compute_digest(
        &v2.source_authority,
        &v2.scope_key,
        &v2.records,
        v2.export_meta.as_ref(),
        v2.evidence_bundle.as_ref(),
    )
    .unwrap();

    let v3 = v2.enrich_to_v3().unwrap();
    let semantics = v3.records[0]
        .semantics
        .as_ref()
        .expect("semantics should still lift");

    assert_eq!(semantics.nuisance_snapshot, None);
    assert_eq!(
        semantics.projection_visibility_class,
        ProjectionVisibilityClass::Standard
    );
    assert_eq!(
        semantics.export_confidence_class,
        ExportConfidenceClass::Verified
    );
}

#[test]
fn enrich_to_v3_lifts_existing_kernel_metadata_without_invention() {
    let v1 = make_test_envelope();
    let export_meta = ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-rich-1".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-rich-1".into()),
        exported_at: "2026-03-10T00:00:00Z".into(),
    };
    let mut records = v1.records.clone();
    if let ExportRecord::Claim(claim) = &mut records[0] {
        claim.metadata = Some(serde_json::json!({
            "kernel_semantics_v3": {
                "claim_family_id": "family-1",
                "assertion_group_id": "assertion-1",
                "constraint_seed_kind": "hyperedge",
                "treatment_hint": "treatment",
                "projection_visibility_class": "standard",
                "export_confidence_class": "verified",
                "derivation_seed_ids": ["seed-1"]
            }
        }));
    }

    let v2 = ExportEnvelopeV2 {
        envelope_id: v1.envelope_id,
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: ExportEnvelopeV2::compute_digest(
            &v1.source_authority,
            &v1.scope_key,
            &records,
            Some(&export_meta),
            None,
        )
        .unwrap(),
        source_authority: v1.source_authority,
        scope_key: v1.scope_key,
        trace_ctx: v1.trace_ctx,
        exported_at: v1.exported_at,
        export_meta: Some(export_meta),
        evidence_bundle: None,
        records,
    };

    let v3 = v2.enrich_to_v3().unwrap();
    let semantics = v3.records[0].semantics.as_ref().expect("rich semantics");
    assert_eq!(
        semantics.claim_family_id.as_ref().map(|id| id.as_str()),
        Some("claim-family:family-1")
    );
    assert_eq!(
        semantics.assertion_group_id.as_ref().map(|id| id.as_str()),
        Some("assertion-group:assertion-1")
    );
    assert_eq!(
        semantics.comparability_snapshot_version.as_deref(),
        Some("cmp-rich-1")
    );
}

#[test]
fn enrich_to_v3_preserves_absence_when_metadata_is_thin() {
    let v2 = ExportEnvelopeV2::from(make_test_envelope());
    let v3 = v2.enrich_to_v3().unwrap();
    assert!(
        v3.records.iter().all(|record| record.semantics.is_none()),
        "thin records must remain thin rather than gaining invented semantics"
    );
}

#[test]
fn enrich_to_v3_rejects_invalid_source_envelope() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    v2.content_digest = ContentDigest::compute(b"tampered");

    let err = v2.enrich_to_v3().unwrap_err();
    assert!(matches!(err, ExportEnvelopeError::DigestMismatch { .. }));
}

#[test]
fn try_from_v2_to_v3_matches_enrich_to_v3() {
    let v2 = ExportEnvelopeV2::from(make_test_envelope());
    let via_method = v2.enrich_to_v3().unwrap();
    let via_try_from = ExportEnvelopeV3::try_from(v2).unwrap();

    assert_eq!(
        serde_json::to_value(&via_method).unwrap(),
        serde_json::to_value(&via_try_from).unwrap()
    );
}

#[test]
fn try_from_v2_to_v3_rejects_invalid_source_instead_of_minting_digest() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    v2.schema_version = "export_envelope_v99".into();

    let err = ExportEnvelopeV3::try_from(v2).unwrap_err();
    assert!(matches!(
        err,
        ExportEnvelopeError::IncompatibleVersion { .. }
    ));
}

#[test]
fn try_from_v3_to_v2_preserves_base_records() {
    let v2 = ExportEnvelopeV2::from(make_test_envelope());
    let v3 = v2.enrich_to_v3().unwrap();
    let downgraded = ExportEnvelopeV2::try_from(v3.clone()).unwrap();
    let downgraded_records = downgraded.records.clone();
    let v3_records = v3
        .records
        .iter()
        .map(|record| record.record.clone())
        .collect::<Vec<_>>();

    downgraded.validate().unwrap();
    assert_eq!(
        serde_json::to_value(&downgraded_records).unwrap(),
        serde_json::to_value(&v3_records).unwrap()
    );
}

#[test]
fn try_from_v3_to_v2_rejects_invalid_source_instead_of_minting_digest() {
    let v2 = ExportEnvelopeV2::from(make_test_envelope());
    let mut v3 = v2.enrich_to_v3().unwrap();
    v3.content_digest = ContentDigest::compute(b"tampered");

    let err = ExportEnvelopeV2::try_from(v3).unwrap_err();
    assert!(matches!(err, ExportEnvelopeError::DigestMismatch { .. }));
}

#[test]
fn try_from_v2_to_v1_recomputes_digest() {
    let mut v2 = ExportEnvelopeV2 {
        envelope_id: EnvelopeId::new("env-from-v2-no-meta"),
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: ContentDigest::compute(b"placeholder"),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("test-ns"),
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        export_meta: None,
        evidence_bundle: None,
        records: vec![ExportRecord::Claim(ExportClaim {
            claim_id: Some(ClaimId::new("claim-recompute")),
            claim_version_id: None,
            subject_entity_id: EntityId::new("ent-recompute"),
            predicate: "is_a".into(),
            object_anchor: serde_json::json!("entity"),
            valid_from: None,
            valid_to: None,
            confidence: 0.9,
            content: "Recompute this checksum".into(),
            projection_family: "forge_verification".into(),
            supersedes_claim_id: None,
            supersedes_claim_version_id: None,
            metadata: None,
        })],
    };
    v2.content_digest = ExportEnvelopeV2::compute_digest(
        &v2.source_authority,
        &v2.scope_key,
        &v2.records,
        v2.export_meta.as_ref(),
        v2.evidence_bundle.as_ref(),
    )
    .unwrap();

    let v1 = ExportEnvelopeV1::try_from(v2).unwrap();
    let expected =
        ExportEnvelopeV1::compute_digest(&v1.source_authority, &v1.scope_key, &v1.records).unwrap();

    assert_eq!(v1.content_digest, expected);
    v1.validate().unwrap();
}

#[test]
fn try_from_v2_to_v1_recomputes_digest_when_export_meta_is_present() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    v2.export_meta = Some(ForgeExportMeta {
        authority: ExportAuthority::Forge,
        run_id: Some("run-2".into()),
        direct_write: false,
        comparability_snapshot_version: Some("cmp-2".into()),
        exported_at: "2026-03-08T00:00:00Z".into(),
    });
    v2.content_digest = ExportEnvelopeV2::compute_digest(
        &v2.source_authority,
        &v2.scope_key,
        &v2.records,
        v2.export_meta.as_ref(),
        v2.evidence_bundle.as_ref(),
    )
    .unwrap();
    let legacy_digest = v2.content_digest.clone();

    let v1 = ExportEnvelopeV1::try_from(v2).unwrap();
    let expected =
        ExportEnvelopeV1::compute_digest(&v1.source_authority, &v1.scope_key, &v1.records).unwrap();

    assert_eq!(v1.content_digest, expected);
    assert_ne!(
        v1.content_digest, legacy_digest,
        "legacy v2 digest should include export metadata and differ when downgraded to V1"
    );
    v1.validate().unwrap();
}

#[test]
fn try_from_v2_to_v1_recomputes_digest_when_evidence_bundle_is_present() {
    let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
    v2.evidence_bundle = Some(make_test_evidence_bundle());
    v2.content_digest = ExportEnvelopeV2::compute_digest(
        &v2.source_authority,
        &v2.scope_key,
        &v2.records,
        v2.export_meta.as_ref(),
        v2.evidence_bundle.as_ref(),
    )
    .unwrap();
    let legacy_digest = v2.content_digest.clone();

    let v1 = ExportEnvelopeV1::try_from(v2).unwrap();
    let expected =
        ExportEnvelopeV1::compute_digest(&v1.source_authority, &v1.scope_key, &v1.records).unwrap();

    assert_eq!(v1.content_digest, expected);
    assert_ne!(
        v1.content_digest, legacy_digest,
        "legacy v2 digest should include evidence bundle and differ when downgraded to V1"
    );
    v1.validate().unwrap();
}

#[test]
fn try_from_v2_to_v1_returns_structured_error_instead_of_panicking() {
    let v2 = ExportEnvelopeV2 {
        envelope_id: EnvelopeId::new("env-bad-v2"),
        schema_version: EXPORT_ENVELOPE_V2_SCHEMA.into(),
        content_digest: ContentDigest::compute(b"ignored"),
        source_authority: "forge".into(),
        scope_key: ScopeKey::namespace_only("test-ns"),
        trace_ctx: None,
        exported_at: "2026-03-07T00:00:00Z".into(),
        export_meta: None,
        evidence_bundle: None,
        records: vec![ExportRecord::Claim(ExportClaim {
            claim_id: Some(ClaimId::new("claim-bad")),
            claim_version_id: None,
            subject_entity_id: EntityId::new("ent-bad"),
            predicate: "is_a".into(),
            object_anchor: serde_json::json!("entity"),
            valid_from: None,
            valid_to: None,
            confidence: f32::NAN,
            content: "non-finite confidence".into(),
            projection_family: "forge_verification".into(),
            supersedes_claim_id: None,
            supersedes_claim_version_id: None,
            metadata: None,
        })],
    };

    let err = ExportEnvelopeV1::try_from(v2).unwrap_err();
    assert!(matches!(err, ExportEnvelopeError::InvalidEnvelope { .. }));
}

proptest! {
    #[test]
    fn enrich_to_v3_tolerates_partial_or_malformed_semantic_combinations(
        include_visibility in any::<bool>(),
        include_confidence in any::<bool>(),
        include_seed_kind in any::<bool>(),
        valid_seed_kind in any::<bool>(),
        include_nuisance_snapshot in any::<bool>(),
        nuisance_snapshot_as_object in any::<bool>(),
    ) {
        let mut v2 = ExportEnvelopeV2::from(make_test_envelope());
        if let ExportRecord::Claim(claim) = &mut v2.records[0] {
            let mut semantics = serde_json::Map::new();
            if include_visibility {
                semantics.insert(
                    "projection_visibility_class".into(),
                    serde_json::json!("standard"),
                );
            }
            if include_confidence {
                semantics.insert(
                    "export_confidence_class".into(),
                    serde_json::json!("verified"),
                );
            }
            if include_seed_kind {
                semantics.insert(
                    "constraint_seed_kind".into(),
                    if valid_seed_kind {
                        serde_json::json!("hyperedge")
                    } else {
                        serde_json::json!("not_a_constraint_kind")
                    },
                );
            }
            if include_nuisance_snapshot {
                semantics.insert(
                    "nuisance_snapshot".into(),
                    if nuisance_snapshot_as_object {
                        serde_json::json!({"environment_fingerprint": "env-a"})
                    } else {
                        serde_json::json!("must_be_object")
                    },
                );
            }

            claim.metadata = Some(serde_json::json!({
                "kernel_semantics_v3": serde_json::Value::Object(semantics),
            }));
        }

        v2.content_digest = ExportEnvelopeV2::compute_digest(
            &v2.source_authority,
            &v2.scope_key,
            &v2.records,
            v2.export_meta.as_ref(),
            v2.evidence_bundle.as_ref(),
        )
        .unwrap();

        let v3 = v2.enrich_to_v3().unwrap();
        let semantics = v3.records[0]
            .semantics
            .as_ref()
            .expect("semantics should still lift");

        if include_visibility {
            prop_assert_eq!(
                semantics.projection_visibility_class.clone(),
                ProjectionVisibilityClass::Standard
            );
        }
        if include_confidence {
            prop_assert_eq!(
                semantics.export_confidence_class.clone(),
                ExportConfidenceClass::Verified
            );
        }
        if include_seed_kind && valid_seed_kind {
            prop_assert_eq!(
                semantics.constraint_seed_kind.clone(),
                Some(ConstraintSeedKind::Hyperedge)
            );
        } else if include_seed_kind {
            prop_assert_eq!(semantics.constraint_seed_kind.clone(), None);
        }
    }
}
