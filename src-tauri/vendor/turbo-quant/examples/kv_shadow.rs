use turbo_quant::{KvCacheCompressor, KvCacheConfig, KvQuantPolicy};

fn main() -> turbo_quant::Result<()> {
    let dim = 64;
    let mut cache = KvCacheCompressor::new(KvCacheConfig {
        head_dim: dim,
        key_policy: KvQuantPolicy::quantized(8, 16),
        value_policy: KvQuantPolicy::Exact,
        seed: 42,
        keep_exact_shadow: true,
    })?;
    for token in 0..8 {
        let key = (0..dim)
            .map(|index| ((token * dim + index) as f32 * 0.017).sin())
            .collect::<Vec<_>>();
        let value = (0..dim)
            .map(|index| ((token * dim + index) as f32 * 0.019).cos())
            .collect::<Vec<_>>();
        cache.compress_token(&key, &value)?;
    }
    let query = (0..dim)
        .map(|index| (index as f32 * 0.023).sin())
        .collect::<Vec<_>>();
    let shadow = cache.shadow_attention_scores(&query)?;
    let mean_abs_error = shadow.iter().map(|row| row.abs_error).sum::<f32>() / shadow.len() as f32;
    println!(
        "{}",
        serde_json::json!({
            "schema": "KvShadowExampleV1",
            "tokens": cache.len(),
            "mean_abs_score_error": mean_abs_error,
            "compressed_bytes": cache.compressed_bytes(),
            "uncompressed_bytes": cache.uncompressed_bytes()
        })
    );
    Ok(())
}
