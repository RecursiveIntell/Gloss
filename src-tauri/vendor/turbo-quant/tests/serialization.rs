use turbo_quant::{
    bitpack, CodecProfileV1, CompressionReceiptV1, PolarCode, QjlSketch, TurboCode, TurboQuantizer,
};

#[test]
fn packed_codes_serialize_without_unpacked_vectors() {
    let q = TurboQuantizer::new(16, 8, 8, 42).unwrap();
    let code = q.encode(&[0.1; 16]).unwrap();
    let json = serde_json::to_string(&code).unwrap();
    assert!(json.contains("packed_angle_indices"));
    assert!(json.contains("packed_signs"));
    assert!(!json.contains("\"angle_indices\":"));
    assert!(!json.contains("\"signs\""));
    let restored: TurboCode = serde_json::from_str(&json).unwrap();
    assert_eq!(code, restored);
}

#[test]
fn profile_and_receipt_serialize_roundtrip() {
    let q = TurboQuantizer::new(16, 8, 8, 42).unwrap();
    let (_code, receipt) = q
        .encode_with_receipt(&[0.1; 16], Some("source:test".into()))
        .unwrap();
    let json = serde_json::to_string(&receipt).unwrap();
    let restored: CompressionReceiptV1 = serde_json::from_str(&json).unwrap();
    let profile: CodecProfileV1 =
        serde_json::from_value(serde_json::to_value(&restored.profile).unwrap()).unwrap();
    assert_eq!(restored, receipt);
    assert_eq!(profile.profile_digest, q.profile().profile_digest);
}

#[test]
fn direct_packed_structs_validate_after_roundtrip() {
    let polar = PolarCode {
        dim: 8,
        bits: 4,
        radii: vec![1.0; 4],
        packed_angle_indices: bitpack::pack_indices(&[1, 2, 3, 4], 4).unwrap(),
    };
    let qjl = QjlSketch {
        dim: 8,
        projections: 8,
        packed_signs: bitpack::pack_signs(&[1, -1, 1, -1, 1, -1, 1, -1]).unwrap(),
        norm: 1.0,
    };
    polar.validate_for(8, 4).unwrap();
    qjl.validate_for(8, 8).unwrap();
}
