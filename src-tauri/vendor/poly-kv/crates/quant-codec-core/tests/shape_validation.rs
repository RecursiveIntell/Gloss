use quant_codec_core::*;

#[test]
fn validates_shape_and_spans() {
    assert!(KvTensorShape::gqa(2, 2, 4, 8, 16, KvLayout::LayersHeadsTokensDim, DType::F32).is_ok());

    assert!(
        KvTensorShape::gqa(0, 2, 4, 8, 16, KvLayout::LayersHeadsTokensDim, DType::F32).is_err()
    );

    assert!(TokenSpan::new(4, 4).is_err());
    assert!(TokenSpan::new(5, 4).is_err());
}

#[test]
fn rejects_out_of_bounds_slice_request() {
    let shape =
        KvTensorShape::gqa(2, 2, 4, 8, 16, KvLayout::LayersHeadsTokensDim, DType::F32).unwrap();
    let req = KvSliceRequest::layer_span(LayerId(2), TokenSpan::new(0, 1).unwrap());
    assert!(matches!(
        req.validate_for_shape(&shape),
        Err(QuantCodecError::ShapeMismatch { .. })
    ));
}

#[test]
fn shape_v2_accepts_mha_mqa_and_gqa_contracts() {
    assert!(
        KvCacheShapeV2::mha(1, 2, 8, 16, 64, KvLayout::LayersHeadsTokensDim, DType::F32).is_ok()
    );
    assert!(
        KvCacheShapeV2::mqa(1, 2, 8, 16, 64, KvLayout::LayersHeadsTokensDim, DType::F32).is_ok()
    );
    assert!(KvCacheShapeV2::gqa(
        1,
        2,
        8,
        2,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32
    )
    .is_ok());
}

#[test]
fn shape_v2_rejects_invalid_attention_contracts() {
    assert!(KvCacheShapeV2::new(
        0,
        2,
        8,
        8,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32,
        KvAttentionKind::Mha
    )
    .is_err());
    assert!(KvCacheShapeV2::new(
        1,
        2,
        8,
        4,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32,
        KvAttentionKind::Mha
    )
    .is_err());
    assert!(KvCacheShapeV2::new(
        1,
        2,
        1,
        1,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32,
        KvAttentionKind::Mqa
    )
    .is_err());
    assert!(KvCacheShapeV2::new(
        1,
        2,
        10,
        4,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32,
        KvAttentionKind::Gqa
    )
    .is_err());
}

#[test]
fn shape_v2_unsupported_attention_fails_closed() {
    let err = KvCacheShapeV2::new(
        1,
        2,
        8,
        8,
        16,
        64,
        KvLayout::LayersHeadsTokensDim,
        DType::F32,
        KvAttentionKind::Unsupported("mla".to_string()),
    )
    .unwrap_err();
    assert!(matches!(err, QuantCodecError::InvalidShape { .. }));
}
