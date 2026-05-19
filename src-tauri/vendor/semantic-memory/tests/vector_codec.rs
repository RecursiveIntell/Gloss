#[cfg(feature = "turbo-quant-codec")]
use semantic_memory::TurboQuantCodec;
use semantic_memory::{
    MemoryError, RawF32Codec, Sq8Codec, VectorArtifactV1, VectorCodec, VectorCodecProfileV1,
};

fn sample_vector() -> Vec<f32> {
    vec![0.25, -0.5, 0.75, 1.0]
}

#[cfg(feature = "turbo-quant-codec")]
fn deterministic_vector(dim: usize, seed: u64) -> Vec<f32> {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut vector = Vec::with_capacity(dim);
    for _ in 0..dim {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = ((state as f64 / u64::MAX as f64) * 2.0 - 1.0) as f32;
        vector.push(value);
    }
    vector
}

#[cfg(feature = "turbo-quant-codec")]
fn inner_product(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

#[test]
fn raw_profile_digest_is_stable_and_identity_sensitive() -> Result<(), MemoryError> {
    let a = VectorCodecProfileV1::raw_f32(4)?;
    let b = VectorCodecProfileV1::raw_f32(4)?;
    let c = VectorCodecProfileV1::raw_f32(8)?;

    assert_eq!(a.digest(), b.digest());
    assert_ne!(a.digest(), c.digest());
    assert!(a.digest().starts_with("blake3:"));
    assert_eq!(a.dim, 4);
    Ok(())
}

#[test]
fn profile_digest_uses_blake3_and_changes_on_profile_fields() -> Result<(), MemoryError> {
    let base = VectorCodecProfileV1::raw_f32(4)?;
    let sq8 = VectorCodecProfileV1::sq8(4)?;
    let other_dim = VectorCodecProfileV1::raw_f32(8)?;

    assert!(base.digest().starts_with("blake3:"));
    assert_eq!(base.digest().len(), 71);
    assert_ne!(base.digest(), sq8.digest());
    assert_ne!(base.digest(), other_dim.digest());
    Ok(())
}

#[test]
fn raw_f32_codec_round_trips_exactly() -> Result<(), MemoryError> {
    let codec = RawF32Codec::new(4)?;
    let vector = sample_vector();
    let artifact = codec.encode(&vector)?;
    let decoded = codec.decode(&artifact)?;

    assert_eq!(artifact.profile_digest, codec.profile().digest());
    assert_eq!(artifact.artifact_digest, artifact.encoded_digest());
    assert!(artifact.artifact_digest.starts_with("blake3:"));
    assert_eq!(decoded, vector);
    Ok(())
}

#[test]
fn vector_artifact_digest_tampering_fails_closed() -> Result<(), MemoryError> {
    let codec = RawF32Codec::new(4)?;
    let mut artifact = codec.encode(&sample_vector())?;
    artifact.encoded[0] ^= 0x80;

    let err = match codec.decode(&artifact) {
        Ok(_) => panic!("tampered encoded bytes should fail closed"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "corrupt_data");
    Ok(())
}

#[test]
fn sq8_codec_round_trips_with_profile_identity() -> Result<(), MemoryError> {
    let codec = Sq8Codec::new(4)?;
    let vector = sample_vector();
    let artifact = codec.encode(&vector)?;
    let decoded = codec.decode(&artifact)?;

    assert_eq!(artifact.profile.codec, "sq8");
    assert_eq!(artifact.profile.bits, 8);
    assert_eq!(decoded.len(), vector.len());
    for value in decoded {
        assert!(value.is_finite());
    }
    Ok(())
}

#[test]
fn profile_mismatch_fails_closed() -> Result<(), MemoryError> {
    let raw = RawF32Codec::new(4)?;
    let sq8 = Sq8Codec::new(4)?;
    let artifact = raw.encode(&sample_vector())?;

    let err = match sq8.decode(&artifact) {
        Ok(_) => panic!("mismatched profile should fail closed"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "vector_codec_profile_mismatch");
    Ok(())
}

#[test]
fn artifact_profile_digest_tampering_fails_closed() -> Result<(), MemoryError> {
    let codec = RawF32Codec::new(4)?;
    let mut artifact: VectorArtifactV1 = codec.encode(&sample_vector())?;
    artifact.profile = VectorCodecProfileV1::raw_f32(8)?;

    let err = match codec.decode(&artifact) {
        Ok(_) => panic!("tampered artifact profile should fail closed"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "vector_codec_profile_mismatch");
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn turbo_quant_codec_is_deterministic_for_same_profile() -> Result<(), MemoryError> {
    let codec_a = TurboQuantCodec::new(16, 8, 16, 42)?;
    let codec_b = TurboQuantCodec::new(16, 8, 16, 42)?;
    let vector = deterministic_vector(16, 7);

    let artifact_a = codec_a.encode(&vector)?;
    let artifact_b = codec_b.encode(&vector)?;

    assert_eq!(artifact_a.profile_digest, artifact_b.profile_digest);
    assert_eq!(artifact_a.encoded_digest(), artifact_b.encoded_digest());
    assert_eq!(artifact_a.artifact_digest, artifact_a.encoded_digest());
    assert_eq!(artifact_a.profile.codec, "turbo_quant");
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn turbo_quant_seed_changes_code_digest() -> Result<(), MemoryError> {
    let codec_a = TurboQuantCodec::new(16, 8, 16, 42)?;
    let codec_b = TurboQuantCodec::new(16, 8, 16, 43)?;
    let vector = deterministic_vector(16, 7);

    let artifact_a = codec_a.encode(&vector)?;
    let artifact_b = codec_b.encode(&vector)?;

    assert_ne!(artifact_a.profile_digest, artifact_b.profile_digest);
    assert_ne!(artifact_a.encoded_digest(), artifact_b.encoded_digest());
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn profile_digest_uses_blake3_and_changes_on_seed() -> Result<(), MemoryError> {
    let codec_a = TurboQuantCodec::new(16, 8, 16, 42)?;
    let codec_b = TurboQuantCodec::new(16, 8, 16, 43)?;
    let codec_c = TurboQuantCodec::new(16, 7, 16, 42)?;
    let codec_d = TurboQuantCodec::new(16, 8, 8, 42)?;

    assert!(codec_a.profile().digest().starts_with("blake3:"));
    assert_ne!(codec_a.profile().digest(), codec_b.profile().digest());
    assert_ne!(codec_a.profile().digest(), codec_c.profile().digest());
    assert_ne!(codec_a.profile().digest(), codec_d.profile().digest());
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn encoded_digest_changes_when_any_byte_changes() -> Result<(), MemoryError> {
    let codec = TurboQuantCodec::new(16, 8, 16, 42)?;
    let mut artifact = codec.encode(&deterministic_vector(16, 7))?;
    let original = artifact.encoded_digest();
    artifact.encoded[0] ^= 0x01;
    assert_ne!(original, artifact.encoded_digest());
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn turbo_quant_tampered_encoded_digest_fails_closed() -> Result<(), MemoryError> {
    let codec = TurboQuantCodec::new(16, 8, 16, 42)?;
    let mut artifact = codec.encode(&deterministic_vector(16, 7))?;
    let last = artifact.encoded.len() - 1;
    artifact.encoded[last] ^= 0x01;
    let query = deterministic_vector(16, 99);

    let err = match codec.score_inner_product(&artifact, &query) {
        Ok(_) => panic!("tampered TurboQuant artifact should fail closed"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "corrupt_data");
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn turbo_quant_wrong_profile_rejects_scoring() -> Result<(), MemoryError> {
    let codec_a = TurboQuantCodec::new(16, 8, 16, 42)?;
    let codec_b = TurboQuantCodec::new(16, 8, 16, 43)?;
    let artifact = codec_a.encode(&deterministic_vector(16, 7))?;
    let query = deterministic_vector(16, 99);

    let err = match codec_b.score_inner_product(&artifact, &query) {
        Ok(_) => panic!("wrong TurboQuant profile should fail closed"),
        Err(err) => err,
    };
    assert_eq!(err.kind(), "vector_codec_profile_mismatch");
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[test]
fn turbo_quant_fixed_corpus_drift_harness_reports_metrics() -> Result<(), MemoryError> {
    let dim = 32;
    let db_len = 48;
    let k = 5;
    let codec = TurboQuantCodec::new(dim, 8, 16, 123)?;
    let query = deterministic_vector(dim, 10_000);
    let corpus: Vec<Vec<f32>> = (0..db_len)
        .map(|seed| deterministic_vector(dim, seed as u64 + 1))
        .collect();
    let artifacts: Vec<_> = corpus
        .iter()
        .map(|vector| codec.encode(vector))
        .collect::<Result<_, _>>()?;

    let mut exact: Vec<(usize, f32)> = corpus
        .iter()
        .enumerate()
        .map(|(idx, vector)| (idx, inner_product(vector, &query)))
        .collect();
    exact.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut approx = Vec::with_capacity(artifacts.len());
    for (idx, artifact) in artifacts.iter().enumerate() {
        approx.push((idx, codec.score_inner_product(artifact, &query)?));
    }
    approx.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let exact_top: std::collections::BTreeSet<_> =
        exact.iter().take(k).map(|(idx, _)| *idx).collect();
    let approx_top: std::collections::BTreeSet<_> =
        approx.iter().take(k).map(|(idx, _)| *idx).collect();
    let recall_at_k = exact_top.intersection(&approx_top).count() as f32 / k as f32;

    let mut exact_rank = vec![0usize; db_len];
    let mut approx_rank = vec![0usize; db_len];
    for (rank, (idx, _)) in exact.iter().enumerate() {
        exact_rank[*idx] = rank;
    }
    for (rank, (idx, _)) in approx.iter().enumerate() {
        approx_rank[*idx] = rank;
    }
    let mean_rank_drift = (0..db_len)
        .map(|idx| exact_rank[idx].abs_diff(approx_rank[idx]) as f32)
        .sum::<f32>()
        / db_len as f32;

    let mut absolute_errors = Vec::with_capacity(db_len);
    for (idx, exact_score) in &exact {
        let approx_score = approx
            .iter()
            .find(|(candidate_idx, _)| candidate_idx == idx)
            .map(|(_, score)| *score)
            .unwrap_or(f32::NAN);
        absolute_errors.push((exact_score - approx_score).abs());
    }
    absolute_errors.sort_by(f32::total_cmp);
    let mean_absolute_score_error =
        absolute_errors.iter().sum::<f32>() / absolute_errors.len() as f32;
    let p95_absolute_score_error = absolute_errors
        [((absolute_errors.len() as f32 * 0.95).floor() as usize).min(absolute_errors.len() - 1)];
    let storage_bytes_per_vector = artifacts
        .iter()
        .map(|artifact| artifact.encoded.len())
        .sum::<usize>() as f32
        / artifacts.len() as f32;

    println!(
        "turbo_quant_drift recall@{k}={recall_at_k:.3} mean_rank_drift={mean_rank_drift:.3} mean_abs_score_error={mean_absolute_score_error:.3} p95_abs_score_error={p95_absolute_score_error:.3} storage_bytes_per_vector={storage_bytes_per_vector:.1}"
    );

    assert!(recall_at_k.is_finite());
    assert!(mean_rank_drift.is_finite());
    assert!(mean_absolute_score_error.is_finite());
    assert!(p95_absolute_score_error.is_finite());
    assert!(storage_bytes_per_vector < (dim * std::mem::size_of::<f32>()) as f32 * 8.0);
    Ok(())
}
