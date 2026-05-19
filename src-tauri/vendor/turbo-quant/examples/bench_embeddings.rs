use std::{env, fs, path::PathBuf};

use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, StandardNormal};
use turbo_quant::{
    eval::recall_at_k, BenchmarkComparisonV1, BenchmarkCorpus, BenchmarkReceiptV1,
    CompressionEvalV1, RotationKind, TurboMode, TurboQuantizer,
};

#[derive(Debug)]
struct Args {
    dim: usize,
    db_size: usize,
    queries: usize,
    bits: u8,
    projections: usize,
    seed: u64,
    top_k: usize,
    out: PathBuf,
    rotation: RotationKind,
    compare_stored: bool,
}

fn main() -> turbo_quant::Result<()> {
    let args = parse_args();
    let quantizer = TurboQuantizer::new_with_mode_and_rotation(
        args.dim,
        args.bits,
        args.projections,
        args.seed,
        TurboMode::PolarWithQjl,
        args.rotation,
    )?;
    let mut rng = ChaCha8Rng::seed_from_u64(args.seed);
    let db = random_matrix(args.db_size, args.dim, &mut rng);
    let queries = random_matrix(args.queries, args.dim, &mut rng);
    let metrics = evaluate(&quantizer, &db, &queries, args.top_k)?;
    let mut comparisons = Vec::new();
    if args.compare_stored && quantizer.rotation_kind() != RotationKind::StoredQr {
        let stored = TurboQuantizer::new_with_stored_rotation(
            args.dim,
            args.bits,
            args.projections,
            args.seed,
        )?;
        comparisons.push(BenchmarkComparisonV1 {
            name: "stored_qr_reference".into(),
            profile: stored.profile(),
            metrics: evaluate(&stored, &db, &queries, args.top_k)?,
        });
    }
    let receipt = BenchmarkReceiptV1 {
        schema: "BenchmarkReceiptV1".into(),
        profile: quantizer.profile(),
        corpus: BenchmarkCorpus {
            dim: args.dim,
            db_size: args.db_size,
            queries: args.queries,
            seed: args.seed,
            generator: "standard_normal_chacha8".into(),
        },
        metrics,
        comparisons,
        commands: vec![env::args().collect::<Vec<_>>().join(" ")],
        warnings: vec![
            "synthetic benchmark; do not use as deployment-quality evidence".into(),
            "compressed scores are sidecar estimates and require exact fallback gates".into(),
        ],
    };

    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent).map_err(|err| turbo_quant::TurboQuantError::MalformedCode {
            reason: format!("failed to create benchmark output directory: {err}"),
        })?;
    }
    fs::write(&args.out, serde_json::to_vec_pretty(&receipt).unwrap()).map_err(|err| {
        turbo_quant::TurboQuantError::MalformedCode {
            reason: format!(
                "failed to write benchmark receipt {}: {err}",
                args.out.display()
            ),
        }
    })?;
    println!("{}", args.out.display());
    Ok(())
}

fn parse_args() -> Args {
    let mut args = Args {
        dim: 128,
        db_size: 512,
        queries: 16,
        bits: 4,
        projections: 64,
        seed: 42,
        top_k: 10,
        out: PathBuf::from("target/turbo-quant/p24-bench.json"),
        rotation: RotationKind::Auto,
        compare_stored: true,
    };
    let mut iter = env::args().skip(1);
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .unwrap_or_else(|| panic!("missing value for {flag}"));
        match flag.as_str() {
            "--dim" => args.dim = value.parse().unwrap(),
            "--db-size" => args.db_size = value.parse().unwrap(),
            "--queries" => args.queries = value.parse().unwrap(),
            "--bits" => args.bits = value.parse().unwrap(),
            "--projections" => args.projections = value.parse().unwrap(),
            "--seed" => args.seed = value.parse().unwrap(),
            "--top-k" => args.top_k = value.parse().unwrap(),
            "--out" => args.out = PathBuf::from(value),
            "--rotation" => args.rotation = parse_rotation(&value),
            "--compare-stored" => args.compare_stored = value.parse().unwrap(),
            other => panic!("unknown argument {other}"),
        }
    }
    args
}

fn parse_rotation(value: &str) -> RotationKind {
    match value {
        "auto" => RotationKind::Auto,
        "fast" | "fast_hadamard" => RotationKind::FastHadamard,
        "stored" | "stored_qr" => RotationKind::StoredQr,
        other => panic!("unknown rotation {other}"),
    }
}

fn random_matrix(rows: usize, dim: usize, rng: &mut ChaCha8Rng) -> Vec<Vec<f32>> {
    (0..rows)
        .map(|_| (0..dim).map(|_| StandardNormal.sample(rng)).collect())
        .collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(left, right)| left * right)
        .sum()
}

fn top_indices(scores: &[f32], k: usize) -> Vec<usize> {
    let mut indexed = scores.iter().copied().enumerate().collect::<Vec<_>>();
    indexed.sort_by(|left, right| right.1.partial_cmp(&left.1).unwrap());
    indexed
        .into_iter()
        .take(k)
        .map(|(index, _)| index)
        .collect()
}

fn evaluate(
    quantizer: &TurboQuantizer,
    db: &[Vec<f32>],
    queries: &[Vec<f32>],
    top_k: usize,
) -> turbo_quant::Result<CompressionEvalV1> {
    let codes = db
        .iter()
        .map(|vector| quantizer.encode(vector))
        .collect::<turbo_quant::Result<Vec<_>>>()?;
    let mut exact_rankings = Vec::with_capacity(queries.len());
    let mut estimated_rankings = Vec::with_capacity(queries.len());
    let mut abs_error_sum = 0.0f32;
    let mut score_count = 0usize;

    for query in queries {
        let exact_scores = db
            .iter()
            .map(|vector| dot(vector, query))
            .collect::<Vec<_>>();
        let estimated_scores = codes
            .iter()
            .map(|code| quantizer.inner_product_estimate(code, query))
            .collect::<turbo_quant::Result<Vec<_>>>()?;
        abs_error_sum += exact_scores
            .iter()
            .zip(estimated_scores.iter())
            .map(|(exact, estimated)| (exact - estimated).abs())
            .sum::<f32>();
        score_count += exact_scores.len();
        exact_rankings.push(top_indices(&exact_scores, top_k));
        estimated_rankings.push(top_indices(&estimated_scores, top_k));
    }

    Ok(CompressionEvalV1 {
        schema: "CompressionEvalV1".into(),
        recall_at_k: recall_at_k(&exact_rankings, &estimated_rankings, top_k),
        mean_absolute_error: abs_error_sum / score_count as f32,
        queries: queries.len(),
        db_size: db.len(),
        top_k,
    })
}
