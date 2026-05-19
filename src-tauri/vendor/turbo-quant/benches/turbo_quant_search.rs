use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use std::time::{Duration, Instant};
use turbo_quant::TurboQuantizer;

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
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

fn corpus(dim: usize, len: usize) -> Vec<Vec<f32>> {
    (0..len)
        .map(|index| deterministic_vector(dim, index as u64 + 1))
        .collect()
}

fn projection_matrix(dim: usize) -> [usize; 2] {
    [(dim / 8).max(1), (dim / 4).max(1)]
}

fn raw_dot(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

fn top_k_scores(scores: &mut Vec<(usize, f32)>, k: usize) {
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores.truncate(k);
}

fn bench_encode_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("turbo_quant_encode_throughput");
    group.sampling_mode(SamplingMode::Flat);
    group.measurement_time(Duration::from_secs(1));
    for dim in [32usize, 384, 768, 1536] {
        for bits in [4u8, 8] {
            for projections in projection_matrix(dim) {
                let quantizer = TurboQuantizer::new(dim, bits, projections, 42).unwrap();
                let vector = deterministic_vector(dim, 7);
                group.throughput(Throughput::Elements(1));
                group.bench_with_input(
                    BenchmarkId::from_parameter(format!(
                        "dim={dim}/bits={bits}/proj={projections}"
                    )),
                    &(dim, bits, projections),
                    |b, _| {
                        b.iter(|| quantizer.encode_to_bytes(black_box(&vector)).unwrap());
                    },
                );
            }
        }
    }
    group.finish();
}

fn bench_score_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("turbo_quant_score_throughput");
    group.sampling_mode(SamplingMode::Flat);
    group.measurement_time(Duration::from_secs(1));
    for dim in [32usize, 384, 768, 1536] {
        let projections = (dim / 8).max(1);
        let quantizer = TurboQuantizer::new(dim, 8, projections, 42).unwrap();
        let vector = deterministic_vector(dim, 11);
        let query = deterministic_vector(dim, 12);
        let code = quantizer.encode(&vector).unwrap();
        let prepared = quantizer.prepare_query(&query).unwrap();

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("raw_dot", dim), &dim, |b, _| {
            b.iter(|| raw_dot(black_box(&vector), black_box(&query)));
        });
        group.bench_with_input(BenchmarkId::new("turbo_unprepared", dim), &dim, |b, _| {
            b.iter(|| {
                quantizer
                    .inner_product_estimate(black_box(&code), black_box(&query))
                    .unwrap()
            });
        });
        group.bench_with_input(BenchmarkId::new("turbo_prepared", dim), &dim, |b, _| {
            b.iter(|| {
                quantizer
                    .inner_product_estimate_prepared(black_box(&code), black_box(&prepared))
                    .unwrap()
            });
        });
    }
    group.finish();
}

fn bench_candidate_generation(c: &mut Criterion) {
    let mut group = c.benchmark_group("turbo_quant_candidate_generation");
    group.sampling_mode(SamplingMode::Flat);
    group.measurement_time(Duration::from_secs(2));

    for corpus_size in [1_000usize, 10_000] {
        for dim in [32usize, 384, 768, 1536] {
            let bits = 8;
            let projections = (dim / 8).max(1);
            let quantizer = TurboQuantizer::new(dim, bits, projections, 42).unwrap();
            let corpus = corpus(dim, corpus_size);
            let encoded: Vec<Vec<u8>> = corpus
                .iter()
                .map(|vector| quantizer.encode_to_bytes(vector).unwrap())
                .collect();
            let query = deterministic_vector(dim, 99);
            let prepared = quantizer.prepare_query(&query).unwrap();

            group.throughput(Throughput::Elements(corpus_size as u64));
            group.bench_with_input(
                BenchmarkId::from_parameter(format!("n={corpus_size}/dim={dim}")),
                &(corpus_size, dim),
                |b, _| {
                    b.iter_custom(|iters| {
                        let start = Instant::now();
                        for _ in 0..iters {
                            let mut scores = Vec::with_capacity(encoded.len());
                            for (index, bytes) in encoded.iter().enumerate() {
                                let code = quantizer.decode_code_from_bytes(bytes).unwrap();
                                let score = quantizer
                                    .inner_product_estimate_prepared(&code, &prepared)
                                    .unwrap();
                                scores.push((index, score));
                            }
                            top_k_scores(&mut scores, 10);
                            black_box(scores);
                        }
                        start.elapsed()
                    });
                },
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_encode_throughput,
    bench_score_throughput,
    bench_candidate_generation
);
criterion_main!(benches);
