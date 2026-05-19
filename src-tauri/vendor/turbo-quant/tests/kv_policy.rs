use turbo_quant::{KvCacheCompressor, KvCacheConfig, KvQuantPolicy, RotationKind, TurboMode};

#[test]
fn key_and_value_policies_can_be_asymmetric() {
    let mut cache = KvCacheCompressor::new(KvCacheConfig {
        head_dim: 16,
        key_policy: KvQuantPolicy::Quantized {
            bits: 8,
            projections: 4,
            mode: TurboMode::PolarWithQjl,
            rotation_kind: RotationKind::Auto,
        },
        value_policy: KvQuantPolicy::Exact,
        seed: 42,
        keep_exact_shadow: true,
    })
    .unwrap();
    let value = vec![0.2; 16];
    cache.compress_token(&[0.1; 16], &value).unwrap();
    assert_eq!(cache.decode_values(&[0]).unwrap()[0], value);
    assert_eq!(cache.shadow_attention_scores(&[0.1; 16]).unwrap().len(), 1);
}

#[test]
fn polar_only_key_policy_disables_qjl_for_keys() {
    let mut cache = KvCacheCompressor::new(KvCacheConfig {
        head_dim: 16,
        key_policy: KvQuantPolicy::Quantized {
            bits: 8,
            projections: 0,
            mode: TurboMode::PolarOnly,
            rotation_kind: RotationKind::Auto,
        },
        value_policy: KvQuantPolicy::Exact,
        seed: 42,
        keep_exact_shadow: true,
    })
    .unwrap();
    cache.compress_token(&[0.1; 16], &[0.2; 16]).unwrap();
    assert!(cache.attention_scores(&[0.1; 16]).unwrap()[0].is_finite());
}
