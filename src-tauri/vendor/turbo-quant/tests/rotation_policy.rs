use turbo_quant::{PolarQuantizer, RotationKind, TurboCodeWireV1, TurboMode, TurboQuantizer};

#[test]
fn auto_uses_fast_hadamard_for_power_of_two_dimensions() {
    let q = TurboQuantizer::new(128, 8, 32, 42).unwrap();
    assert_eq!(q.rotation_kind(), RotationKind::FastHadamard);
    assert_eq!(q.profile().rotation_kind, "fast_hadamard");
}

#[test]
fn auto_uses_stored_qr_for_non_power_of_two_dimensions() {
    let q = TurboQuantizer::new(384, 8, 96, 42).unwrap();
    assert_eq!(q.rotation_kind(), RotationKind::StoredQr);
    assert_eq!(q.profile().rotation_kind, "stored_qr_reference");
}

#[test]
fn explicit_fast_hadamard_rejects_unsupported_dimension() {
    let err = PolarQuantizer::new_with_rotation(384, 8, 42, RotationKind::FastHadamard)
        .expect_err("384 is not a power-of-two dimension");
    assert!(err.to_string().contains("power-of-two"));
}

#[test]
fn stored_rotation_remains_explicitly_available() {
    let q = TurboQuantizer::new_with_stored_rotation(128, 8, 32, 42).unwrap();
    assert_eq!(q.rotation_kind(), RotationKind::StoredQr);
    let code = q.encode(&vec![0.1; 128]).unwrap();
    assert!(q
        .inner_product_estimate(&code, &vec![0.2; 128])
        .unwrap()
        .is_finite());
}

#[test]
fn wire_decode_rejects_rotation_mismatch() {
    let fast = TurboQuantizer::new_with_mode_and_rotation(
        128,
        8,
        32,
        42,
        TurboMode::PolarWithQjl,
        RotationKind::FastHadamard,
    )
    .unwrap();
    let stored = TurboQuantizer::new_with_stored_rotation(128, 8, 32, 42).unwrap();
    let bytes = fast.encode_to_bytes(&vec![0.1; 128]).unwrap();
    assert!(TurboCodeWireV1::decode(&bytes, &stored).is_err());
}
