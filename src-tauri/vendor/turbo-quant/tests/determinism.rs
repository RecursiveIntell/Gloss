use turbo_quant::{PolarQuantizer, QjlQuantizer, TurboQuantizer};

fn make_vector(dim: usize, seed: u64) -> Vec<f32> {
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;
    use rand_distr::{Distribution, StandardNormal};
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    (0..dim).map(|_| StandardNormal.sample(&mut rng)).collect()
}

/// Same seed → same result, always.
#[test]
fn polar_encoding_is_reproducible_across_instances() {
    let x = make_vector(16, 1);
    let q1 = PolarQuantizer::new(16, 8, 42).unwrap();
    let q2 = PolarQuantizer::new(16, 8, 42).unwrap();
    let c1 = q1.encode(&x).unwrap();
    let c2 = q2.encode(&x).unwrap();
    assert_eq!(c1.packed_angle_indices, c2.packed_angle_indices);
}

#[test]
fn qjl_sketch_is_reproducible_across_instances() {
    let x = make_vector(16, 2);
    let q1 = QjlQuantizer::new(16, 32, 7).unwrap();
    let q2 = QjlQuantizer::new(16, 32, 7).unwrap();
    let s1 = q1.sketch(&x).unwrap();
    let s2 = q2.sketch(&x).unwrap();
    assert_eq!(s1.packed_signs, s2.packed_signs);
}

#[test]
fn turbo_encoding_is_reproducible_across_instances() {
    let x = make_vector(16, 3);
    let q1 = TurboQuantizer::new(16, 8, 16, 99).unwrap();
    let q2 = TurboQuantizer::new(16, 8, 16, 99).unwrap();
    let c1 = q1.encode(&x).unwrap();
    let c2 = q2.encode(&x).unwrap();
    assert_eq!(
        c1.polar_code.packed_angle_indices,
        c2.polar_code.packed_angle_indices
    );
    assert_eq!(
        c1.residual_sketch.as_ref().unwrap().packed_signs,
        c2.residual_sketch.as_ref().unwrap().packed_signs
    );
}

/// Different seeds → different codes.
#[test]
fn different_seeds_produce_different_codes() {
    let x = make_vector(16, 1);
    let q1 = PolarQuantizer::new(16, 8, 0).unwrap();
    let q2 = PolarQuantizer::new(16, 8, 1).unwrap();
    let c1 = q1.encode(&x).unwrap();
    let c2 = q2.encode(&x).unwrap();
    // Almost certainly different (astronomically unlikely to collide).
    assert_ne!(c1.packed_angle_indices, c2.packed_angle_indices);
}

/// Order of vectors encoded doesn't affect individual codes.
#[test]
fn encoding_order_independence() {
    let q = TurboQuantizer::new(16, 8, 16, 42).unwrap();
    let a = make_vector(16, 10);
    let b = make_vector(16, 20);
    let c = make_vector(16, 30);

    let code_a_first = q.encode(&a).unwrap();
    let _code_b = q.encode(&b).unwrap();
    let _code_c = q.encode(&c).unwrap();
    let code_a_again = q.encode(&a).unwrap();

    assert_eq!(
        code_a_first.polar_code.packed_angle_indices,
        code_a_again.polar_code.packed_angle_indices
    );
}

/// Inner product ordering is preserved for same-quantizer comparisons.
#[test]
fn inner_product_rank_ordering_preserved_at_8bits() {
    let dim = 32;
    let q = TurboQuantizer::new(dim, 8, 32, 0).unwrap();
    let query = make_vector(dim, 999);

    // Build 10 database vectors with known exact inner products.
    let mut db: Vec<(Vec<f32>, f32)> = (0..10u64)
        .map(|i| {
            let v = make_vector(dim, i * 100);
            let ip: f32 = v.iter().zip(query.iter()).map(|(a, b)| a * b).sum();
            (v, ip)
        })
        .collect();

    // Sort by exact IP descending.
    db.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Encode all.
    let codes: Vec<_> = db.iter().map(|(v, _)| q.encode(v).unwrap()).collect();

    // Sort by estimated IP.
    let mut estimated: Vec<(usize, f32)> = codes
        .iter()
        .enumerate()
        .map(|(i, code)| (i, q.inner_product_estimate(code, &query).unwrap()))
        .collect();
    estimated.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    // Top-3 by estimate should overlap substantially with top-3 exact.
    let top3_exact: std::collections::BTreeSet<usize> = (0..3).collect();
    let top3_estimated: std::collections::BTreeSet<usize> =
        estimated.iter().take(3).map(|(i, _)| *i).collect();

    let overlap = top3_exact.intersection(&top3_estimated).count();
    assert!(
        overlap >= 2,
        "top-3 recall too low: {overlap}/3, estimated={estimated:?}"
    );
}
