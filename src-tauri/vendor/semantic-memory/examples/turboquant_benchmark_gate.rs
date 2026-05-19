#[cfg(not(feature = "turbo-quant-codec"))]
fn main() {
    eprintln!("turboquant_benchmark_gate requires --features turbo-quant-codec");
    std::process::exit(2);
}

#[cfg(feature = "turbo-quant-codec")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use semantic_memory::{TurboQuantCodec, VectorCodec};
    use serde_json::json;
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::Instant;

    const DIM: usize = 384;
    const CORPUS_SIZE: usize = 1_000;
    const QUERIES: usize = 50;
    const K: usize = 10;
    const CANDIDATE_MULTIPLIER: usize = 20;
    const BITS: u8 = 8;
    const PROJECTIONS: usize = 96;
    const SEED: u64 = 0x5EED_0031;

    let output_path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                "docs/audits/p32-research-max-retrieval-runtime-20260513/turboquant-smoke-benchmark-summary.json",
            )
        });

    let codec = TurboQuantCodec::new(DIM, BITS, PROJECTIONS, SEED)?;
    let profile_digest = codec.profile().digest();
    let corpus = (0..CORPUS_SIZE)
        .map(|index| deterministic_vector(index as u64, DIM))
        .collect::<Vec<_>>();
    let artifacts = corpus
        .iter()
        .map(|vector| codec.encode(vector))
        .collect::<Result<Vec<_>, _>>()?;

    let encoded_bytes_total = artifacts
        .iter()
        .map(|artifact| artifact.encoded.len() as f64)
        .sum::<f64>();
    let encoded_bytes_per_vector = encoded_bytes_total / CORPUS_SIZE as f64;
    let raw_bytes_per_vector = (DIM * std::mem::size_of::<f32>()) as f64;
    let candidate_count = (K * CANDIDATE_MULTIPLIER).min(CORPUS_SIZE);

    let mut recall_sum = 0.0;
    let mut ndcg_sum = 0.0;
    let mut rank_drift_sum = 0.0;
    let mut rank_drift_count = 0usize;
    let mut abs_score_errors = Vec::new();
    let mut candidate_scoring_ms = Vec::new();
    let mut exact_rerank_ms = Vec::new();

    for query_index in 0..QUERIES {
        let anchor = (query_index * 17) % CORPUS_SIZE;
        let query = deterministic_query(&corpus[anchor], query_index as u64);

        let exact_scores = corpus
            .iter()
            .enumerate()
            .map(|(index, vector)| (index, dot(&query, vector) as f64))
            .collect::<Vec<_>>();
        let exact_top = top_n(exact_scores.clone(), K);
        let exact_rank = ranks(&top_n(exact_scores.clone(), CORPUS_SIZE));

        let prepared = codec.prepare_query(&query)?;
        let candidate_started = Instant::now();
        let mut approx_scores = artifacts
            .iter()
            .enumerate()
            .map(|(index, artifact)| {
                score_artifact(&codec, artifact, &prepared).map(|score| (index, score))
            })
            .collect::<Result<Vec<_>, _>>()?;
        candidate_scoring_ms.push(candidate_started.elapsed().as_secs_f64() * 1_000.0);

        for (index, approx_score) in &approx_scores {
            let exact_score = exact_scores[*index].1;
            abs_score_errors.push((exact_score - *approx_score).abs());
        }

        approx_scores.sort_by(|left, right| right.1.total_cmp(&left.1));
        approx_scores.truncate(candidate_count);

        let rerank_started = Instant::now();
        let reranked = approx_scores
            .into_iter()
            .map(|(index, _)| (index, exact_scores[index].1))
            .collect::<Vec<_>>();
        let reranked_top = top_n(reranked, K);
        exact_rerank_ms.push(rerank_started.elapsed().as_secs_f64() * 1_000.0);

        let exact_ids = exact_top
            .iter()
            .map(|(index, _)| *index)
            .collect::<HashSet<_>>();
        let matched = reranked_top
            .iter()
            .filter(|(index, _)| exact_ids.contains(index))
            .count();
        recall_sum += matched as f64 / K as f64;
        ndcg_sum += ndcg_at_k(&reranked_top, &exact_top, K);

        for (position, (index, _)) in reranked_top.iter().enumerate() {
            if let Some(exact_position) = exact_rank.get(index) {
                rank_drift_sum += position.abs_diff(*exact_position) as f64;
                rank_drift_count += 1;
            }
        }
    }

    let recall_at_10 = recall_sum / QUERIES as f64;
    let ndcg_at_10 = ndcg_sum / QUERIES as f64;
    let mean_rank_drift = if rank_drift_count == 0 {
        0.0
    } else {
        rank_drift_sum / rank_drift_count as f64
    };
    let mean_abs_score_error = mean(&abs_score_errors);
    let p95_abs_score_error = percentile(abs_score_errors, 0.95);
    let candidate_scoring_p50_ms = percentile(candidate_scoring_ms.clone(), 0.50);
    let candidate_scoring_p95_ms = percentile(candidate_scoring_ms, 0.95);
    let exact_rerank_p95_ms = percentile(exact_rerank_ms, 0.95);
    let classification = if recall_at_10 >= 0.99
        && ndcg_at_10 >= 0.99
        && encoded_bytes_per_vector < raw_bytes_per_vector
    {
        "green"
    } else if recall_at_10 >= 0.95 && encoded_bytes_per_vector < raw_bytes_per_vector {
        "amber"
    } else {
        "red"
    };

    let report = json!({
        "schema_version": "retrieval_benchmark_summary_v1",
        "run_id": "P32_RESEARCH_MAX_RETRIEVAL_RUNTIME",
        "gate_class": "smoke",
        "mode": "turbo_quant_candidate_then_exact_f32",
        "filtered": false,
        "profile_digest": profile_digest,
        "dim": DIM,
        "corpus_size": CORPUS_SIZE,
        "query_count": QUERIES,
        "k": K,
        "candidate_multiplier": CANDIDATE_MULTIPLIER,
        "recall_at_10": recall_at_10,
        "ndcg_at_10": ndcg_at_10,
        "mean_rank_drift": mean_rank_drift,
        "mean_abs_score_error": mean_abs_score_error,
        "p95_abs_score_error": p95_abs_score_error,
        "encoded_bytes_per_vector": encoded_bytes_per_vector,
        "raw_bytes_per_vector": raw_bytes_per_vector,
        "candidate_scoring_p50_ms": candidate_scoring_p50_ms,
        "candidate_scoring_p95_ms": candidate_scoring_p95_ms,
        "p95_candidate_latency_ms": candidate_scoring_p95_ms,
        "exact_rerank_count": candidate_count,
        "raw_rows_loaded_per_query_p95": candidate_count,
        "fallback_rate": 0.0,
        "exact_rerank_p95_ms": exact_rerank_p95_ms,
        "classification": classification
    });

    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, serde_json::to_string_pretty(&report)? + "\n")?;
    println!("{}", output_path.display());
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
fn deterministic_vector(seed: u64, dim: usize) -> Vec<f32> {
    let mut vector = (0..dim)
        .map(|index| {
            let x = (seed as f32 + 1.0) * 0.013 + (index as f32 + 3.0) * 0.031;
            (x.sin() * 0.7) + ((x * 1.7).cos() * 0.3)
        })
        .collect::<Vec<_>>();
    normalize(&mut vector);
    vector
}

#[cfg(feature = "turbo-quant-codec")]
fn deterministic_query(anchor: &[f32], seed: u64) -> Vec<f32> {
    let mut query = anchor
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let noise = ((seed as f32 + index as f32 * 0.019).sin()) * 0.015;
            value + noise
        })
        .collect::<Vec<_>>();
    normalize(&mut query);
    query
}

#[cfg(feature = "turbo-quant-codec")]
fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

#[cfg(feature = "turbo-quant-codec")]
fn dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum()
}

#[cfg(feature = "turbo-quant-codec")]
fn score_artifact(
    codec: &semantic_memory::TurboQuantCodec,
    artifact: &semantic_memory::VectorArtifactV1,
    prepared: &turbo_quant::TurboProjectedQuery,
) -> Result<f64, semantic_memory::MemoryError> {
    codec
        .score_inner_product_prepared(artifact, prepared)
        .map(f64::from)
}

#[cfg(feature = "turbo-quant-codec")]
fn top_n(mut scores: Vec<(usize, f64)>, n: usize) -> Vec<(usize, f64)> {
    scores.sort_by(|left, right| right.1.total_cmp(&left.1));
    scores.truncate(n);
    scores
}

#[cfg(feature = "turbo-quant-codec")]
fn ranks(scores: &[(usize, f64)]) -> std::collections::HashMap<usize, usize> {
    scores
        .iter()
        .enumerate()
        .map(|(rank, (index, _))| (*index, rank))
        .collect()
}

#[cfg(feature = "turbo-quant-codec")]
fn ndcg_at_k(ranking: &[(usize, f64)], ideal: &[(usize, f64)], k: usize) -> f64 {
    let gains = ideal
        .iter()
        .enumerate()
        .map(|(rank, (index, _))| (*index, 1.0 / (rank as f64 + 1.0)))
        .collect::<std::collections::HashMap<_, _>>();
    let dcg = ranking
        .iter()
        .take(k)
        .enumerate()
        .map(|(rank, (index, _))| {
            let gain = gains.get(index).copied().unwrap_or(0.0);
            gain / ((rank + 2) as f64).log2()
        })
        .sum::<f64>();
    let ideal_dcg = ideal
        .iter()
        .take(k)
        .enumerate()
        .map(|(rank, (index, _))| {
            let gain = gains.get(index).copied().unwrap_or(0.0);
            gain / ((rank + 2) as f64).log2()
        })
        .sum::<f64>();
    if ideal_dcg > 0.0 {
        dcg / ideal_dcg
    } else {
        0.0
    }
}

#[cfg(feature = "turbo-quant-codec")]
fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

#[cfg(feature = "turbo-quant-codec")]
fn percentile(mut values: Vec<f64>, percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile).round() as usize;
    values[index]
}
