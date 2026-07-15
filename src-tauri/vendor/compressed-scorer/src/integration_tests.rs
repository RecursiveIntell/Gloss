//! Integration tests with real fib-quant and turbo-quant codecs

#[cfg(all(feature = "no_std", any(feature = "fib", feature = "turbo")))]
use alloc::{vec, vec::Vec};

#[cfg(any(feature = "fib", feature = "turbo"))]
use crate::candidate::search_topk;
#[cfg(any(feature = "fib", feature = "turbo"))]
use crate::trait_def::CompressedScorer;

#[cfg(feature = "fib")]
#[test]
fn test_fib_quant_end_to_end() {
    use crate::fib_impl::FibScorerAdapter;

    // Create a scorer — ambient_dim is determined by block_dim in paper_default
    let scorer = match FibScorerAdapter::from_params(4, 4, 32, 7, 42) {
        Ok(s) => s,
        Err(_) => return,
    };

    let dim = scorer.dim();

    // Create some test vectors
    let v1: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.1).collect();
    let v2: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.2).collect();
    let v3: Vec<f32> = (0..dim).map(|i| (i as f32) * -0.1).collect();

    // Encode them
    let c1 = scorer.encode(&v1).expect("encode v1");
    let c2 = scorer.encode(&v2).expect("encode v2");
    let c3 = scorer.encode(&v3).expect("encode v3");

    let compressed = vec![c1, c2, c3];

    // Search for v1 (should match c1 best — approximate)
    let results = search_topk(&scorer, &v1, &compressed, 3).expect("search");

    assert!(!results.is_empty());
    // v3 (opposite direction) should have the lowest score
    assert_eq!(results.last().unwrap().idx, 2);
}

#[cfg(feature = "turbo")]
#[test]
fn test_turbo_quant_end_to_end() {
    use crate::turbo_impl::TurboScorerAdapter;

    let dim = 64usize;
    let scorer = TurboScorerAdapter::new(dim, 8, 16, 42).expect("create scorer");

    // Create test vectors
    let v1: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.01).collect();
    let v2: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.02).collect();
    let v3: Vec<f32> = (0..dim).map(|i| -(i as f32) * 0.01).collect();

    // Encode
    let c1 = scorer.encode(&v1).expect("encode v1");
    let c2 = scorer.encode(&v2).expect("encode v2");
    let c3 = scorer.encode(&v3).expect("encode v3");

    let compressed = vec![c1, c2, c3];

    // Search for v1
    let results = search_topk(&scorer, &v1, &compressed, 3).expect("search");

    assert!(!results.is_empty());
    // v3 (opposite direction) should have the lowest score
    assert_eq!(results.last().unwrap().idx, 2);
}

#[cfg(feature = "turbo")]
#[test]
fn test_turbo_quant_decode_approximate() {
    use crate::trait_def::CompressedScorer;
    use crate::turbo_impl::TurboScorerAdapter;

    let dim = 32usize;
    let scorer = TurboScorerAdapter::new(dim, 8, 8, 42).expect("create scorer");

    let v: Vec<f32> = (0..dim).map(|i| (i as f32) * 0.05).collect();
    let code = scorer.encode(&v).expect("encode");
    let decoded = scorer.decode(&code).expect("decode");

    assert_eq!(decoded.len(), dim);
    // Approximate decode — check it's in the right ballpark
    for (i, (&orig, &dec)) in v.iter().zip(decoded.iter()).enumerate() {
        // Allow up to 50% relative error per element for approximate decode
        let tolerance = (orig.abs() * 0.5).max(0.1);
        assert!(
            (orig - dec).abs() < tolerance,
            "element {}: orig={}, decoded={}, diff={}",
            i,
            orig,
            dec,
            (orig - dec).abs()
        );
    }
}

#[cfg(feature = "fib")]
#[test]
fn test_fib_quant_codec_name() {
    use crate::fib_impl::FibScorerAdapter;

    if let Ok(scorer) = FibScorerAdapter::from_params(4, 4, 32, 7, 42) {
        assert_eq!(scorer.codec_name(), "fib_quant");
        assert!(scorer.dim() > 0);
        assert!(scorer.internal_bytes() > 0);
    }
}

#[cfg(feature = "turbo")]
#[test]
fn test_turbo_quant_codec_name() {
    use crate::turbo_impl::TurboScorerAdapter;

    let scorer = TurboScorerAdapter::new(64, 8, 16, 42).expect("create scorer");
    assert_eq!(scorer.codec_name(), "turbo_quant");
    assert_eq!(scorer.dim(), 64);
}
