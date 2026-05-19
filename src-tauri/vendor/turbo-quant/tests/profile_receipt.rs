use turbo_quant::{CompressionPolicyV1, TurboQuantizer, ValidationState};

#[test]
fn profile_digest_is_stable_for_same_quantizer() {
    let a = TurboQuantizer::new(32, 8, 8, 42).unwrap().profile();
    let b = TurboQuantizer::new(32, 8, 8, 42).unwrap().profile();
    assert_eq!(a.profile_digest, b.profile_digest);
    assert!(a.qjl_enabled);
}

#[test]
fn compression_receipt_accounts_for_actual_code_payload() {
    let q = TurboQuantizer::new(32, 8, 8, 42).unwrap();
    let vector = vec![0.125; 32];
    let (code, receipt) = q.encode_with_receipt(&vector, None).unwrap();
    assert_eq!(receipt.schema, "CompressionReceiptV1");
    assert_eq!(receipt.validation_state, ValidationState::Validated);
    assert_eq!(receipt.encoded_bytes, code.encoded_bytes());
    assert_eq!(receipt.input_dim, 32);
}

#[test]
fn sidecar_policy_requires_exact_fallback() {
    let profile = TurboQuantizer::new(32, 8, 8, 42).unwrap().profile();
    let policy = CompressionPolicyV1::sidecar_shadow(profile);
    assert!(policy.canonical_vectors_required);
    assert!(!policy.lossy_default_allowed);
    assert!(policy.exact_fallback_required);
}
