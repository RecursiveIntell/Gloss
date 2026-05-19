use turbo_quant::{PolarQuantizer, TurboQuantizer};

#[test]
fn polar_projected_query_matches_unprepared_scoring() {
    let q = PolarQuantizer::new(32, 8, 42).unwrap();
    let vector = vec![0.25; 32];
    let query = vec![0.125; 32];
    let code = q.encode(&vector).unwrap();
    let prepared = q.project_query(&query).unwrap();
    let direct = q.inner_product_estimate(&code, &query).unwrap();
    let prepared_score = q
        .inner_product_estimate_with_projected_query(&code, &prepared)
        .unwrap();
    assert!((direct - prepared_score).abs() <= 1e-6);
}

#[test]
fn turbo_projected_query_matches_unprepared_scoring() {
    let q = TurboQuantizer::new(32, 8, 8, 42).unwrap();
    let vector = vec![0.25; 32];
    let query = vec![0.125; 32];
    let code = q.encode(&vector).unwrap();
    let prepared = q.prepare_query(&query).unwrap();
    let direct = q.inner_product_estimate(&code, &query).unwrap();
    let prepared_score = q.inner_product_estimate_prepared(&code, &prepared).unwrap();
    assert!((direct - prepared_score).abs() <= 1e-6);
}
