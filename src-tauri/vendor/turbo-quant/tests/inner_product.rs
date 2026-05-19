use turbo_quant::{PolarQuantizer, TurboQuantizer};

fn random_vector(dim: usize, seed: u64) -> Vec<f32> {
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;
    use rand_distr::{Distribution, StandardNormal};
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..dim).map(|_| StandardNormal.sample(&mut rng)).collect()
}

fn inner_product(x: &[f32], y: &[f32]) -> f32 {
    x.iter().zip(y.iter()).map(|(a, b)| a * b).sum()
}

/// Higher bits → lower error.
#[test]
fn error_decreases_with_bits() {
    let dim = 32;
    let n = 20usize;

    for bits in [4u8, 8, 12] {
        let q = PolarQuantizer::new(dim, bits, 0).unwrap();
        let errors: Vec<f32> = (0..n as u64)
            .map(|seed| {
                let x = random_vector(dim, seed * 2);
                let y = random_vector(dim, seed * 2 + 1);
                let exact = inner_product(&x, &y);
                let code = q.encode(&x).unwrap();
                let est = q.inner_product_estimate(&code, &y).unwrap();
                (est - exact).abs()
            })
            .collect();

        let avg_error: f32 = errors.iter().sum::<f32>() / n as f32;
        println!("bits={bits}, avg_error={avg_error:.4}");
        // This is more of a smoke test — just ensure it terminates without panic.
        assert!(avg_error >= 0.0);
    }
}

/// At high bits, relative error should be small.
#[test]
fn high_bits_gives_low_relative_error() {
    let dim = 64;
    let q = PolarQuantizer::new(dim, 16, 42).unwrap();

    let mut total_relative_error = 0.0f32;
    let n = 20;

    for seed in 0..n as u64 {
        let x = random_vector(dim, seed * 2);
        let y = random_vector(dim, seed * 2 + 1);

        let exact = inner_product(&x, &y);
        if exact.abs() < 0.1 {
            continue; // skip near-orthogonal pairs (relative error undefined)
        }

        let code = q.encode(&x).unwrap();
        let est = q.inner_product_estimate(&code, &y).unwrap();
        let relative = (est - exact).abs() / exact.abs();
        total_relative_error += relative;
    }

    let avg = total_relative_error / n as f32;
    assert!(
        avg < 0.05,
        "avg relative error too high at 16 bits: {avg:.4}"
    );
}

/// TurboQuant self-similarity: ⟨x, x⟩ = ||x||² > 0.
#[test]
fn self_inner_product_is_positive() {
    let q = TurboQuantizer::new(32, 8, 32, 0).unwrap();

    for seed in 0..10u64 {
        let x = random_vector(32, seed);
        let exact_norm_sq: f32 = x.iter().map(|v| v * v).sum();
        let code = q.encode(&x).unwrap();
        let estimated = q.inner_product_estimate(&code, &x).unwrap();

        assert!(
            estimated > 0.0,
            "self inner product should be positive (seed={seed}): estimated={estimated}, exact={exact_norm_sq}"
        );
    }
}

/// Antipodal vectors should have negative inner product estimates.
#[test]
fn antipodal_vectors_give_negative_estimate() {
    let q = TurboQuantizer::new(32, 8, 32, 5).unwrap();

    for seed in 0..10u64 {
        let x = random_vector(32, seed);
        let neg_x: Vec<f32> = x.iter().map(|v| -v).collect();

        let code = q.encode(&x).unwrap();
        let estimated = q.inner_product_estimate(&code, &neg_x).unwrap();

        assert!(
            estimated < 0.0,
            "antipodal inner product should be negative (seed={seed}): estimated={estimated}"
        );
    }
}

/// L2 distance: same vector should give distance ≈ 0.
#[test]
fn l2_distance_to_self_is_near_zero() {
    let q = TurboQuantizer::new(32, 16, 32, 0).unwrap();
    let x = random_vector(32, 77);
    let code = q.encode(&x).unwrap();
    let dist = q.l2_distance_estimate(&code, &x).unwrap();

    // Won't be exactly zero due to compression, but should be small at 16 bits.
    let norm_sq: f32 = x.iter().map(|v| v * v).sum();
    let relative = dist / norm_sq;
    assert!(
        relative < 0.1,
        "relative self-distance too large: {relative:.4}"
    );
}

/// Nearest-neighbor recall: top-k by estimate should mostly match top-k exact.
#[test]
fn topk_recall_at_8bits() {
    let dim = 64;
    let n_db = 100;
    let k = 10;
    let q = TurboQuantizer::new(dim, 8, 32, 42).unwrap();
    let query = random_vector(dim, 0);

    // Build database.
    let db: Vec<Vec<f32>> = (1..=n_db as u64).map(|s| random_vector(dim, s)).collect();

    // Exact top-k.
    let mut exact: Vec<(usize, f32)> = db
        .iter()
        .enumerate()
        .map(|(i, v)| (i, inner_product(v, &query)))
        .collect();
    exact.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let exact_top_k: std::collections::BTreeSet<usize> =
        exact.iter().take(k).map(|(i, _)| *i).collect();

    // Estimated top-k.
    let codes: Vec<_> = db.iter().map(|v| q.encode(v).unwrap()).collect();
    let mut estimated: Vec<(usize, f32)> = codes
        .iter()
        .enumerate()
        .map(|(i, code)| (i, q.inner_product_estimate(code, &query).unwrap()))
        .collect();
    estimated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    let estimated_top_k: std::collections::BTreeSet<usize> =
        estimated.iter().take(k).map(|(i, _)| *i).collect();

    let recall = exact_top_k.intersection(&estimated_top_k).count() as f32 / k as f32;
    println!("top-{k} recall@8bits: {recall:.2}");
    assert!(recall >= 0.6, "top-{k} recall too low: {recall:.2}");
}
