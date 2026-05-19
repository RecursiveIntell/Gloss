use turbo_quant::{
    KvCacheCompressor, KvCacheConfig, KvQuantPolicy, TurboQuantError, TurboQuantizer,
};

#[test]
fn nonfinite_encode_input_is_rejected() {
    let q = TurboQuantizer::new(8, 8, 8, 42).unwrap();
    let mut vector = vec![0.0; 8];
    vector[4] = f32::NAN;
    assert!(matches!(
        q.encode(&vector),
        Err(TurboQuantError::NonFiniteInput { index: 4 })
    ));
}

#[test]
fn odd_dimensions_and_bad_bits_are_rejected() {
    assert!(matches!(
        TurboQuantizer::new(7, 8, 8, 42),
        Err(TurboQuantError::OddDimension { got: 7 })
    ));
    assert!(matches!(
        TurboQuantizer::new(8, 1, 8, 42),
        Err(TurboQuantError::InvalidBitWidth { got: 1 })
    ));
}

#[test]
fn exact_fallback_without_shadow_is_reported() {
    let mut cache = KvCacheCompressor::new(KvCacheConfig {
        head_dim: 8,
        key_policy: KvQuantPolicy::Exact,
        value_policy: KvQuantPolicy::Exact,
        seed: 42,
        keep_exact_shadow: false,
    })
    .unwrap();
    cache.compress_token(&[0.1; 8], &[0.2; 8]).unwrap();
    assert!(cache.attention_scores(&[0.1; 8]).is_err());
}
