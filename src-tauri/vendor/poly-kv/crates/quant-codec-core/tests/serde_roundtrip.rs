use quant_codec_core::*;

#[test]
fn serde_roundtrips_public_types() {
    let shape =
        KvTensorShape::gqa(2, 1, 4, 8, 16, KvLayout::LayersTokensHeadsDim, DType::F16).unwrap();
    let json = serde_json::to_string(&shape).unwrap();
    let decoded: KvTensorShape = serde_json::from_str(&json).unwrap();
    assert_eq!(shape, decoded);

    let codec_id = CodecId::new("q8-key:test").unwrap();
    let json = serde_json::to_string(&codec_id).unwrap();
    let decoded: CodecId = serde_json::from_str(&json).unwrap();
    assert_eq!(codec_id, decoded);
}
