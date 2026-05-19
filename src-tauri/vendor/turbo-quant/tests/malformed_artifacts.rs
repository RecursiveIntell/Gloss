use turbo_quant::{bitpack, PolarCode, QjlSketch, TurboCode, TurboQuantizer};

fn valid_code() -> (TurboQuantizer, TurboCode) {
    let q = TurboQuantizer::new(8, 8, 8, 42).unwrap();
    let vector = (0..8).map(|i| i as f32 * 0.125 + 0.1).collect::<Vec<_>>();
    let code = q.encode(&vector).unwrap();
    (q, code)
}

#[test]
fn negative_polar_radius_rejected() {
    let (q, mut code) = valid_code();
    code.polar_code.radii[0] = -1.0;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn nan_polar_radius_rejected() {
    let (q, mut code) = valid_code();
    code.polar_code.radii[0] = f32::NAN;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn infinite_polar_radius_rejected() {
    let (q, mut code) = valid_code();
    code.polar_code.radii[0] = f32::INFINITY;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn nonzero_polar_angle_padding_rejected() {
    let (q, mut code) = valid_code();
    let last = code.polar_code.packed_angle_indices.len() - 1;
    code.polar_code.packed_angle_indices[last] |= 0b1111_0000;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn qjl_padding_bits_rejected() {
    let q = TurboQuantizer::new(8, 8, 9, 42).unwrap();
    let mut code = q
        .encode(&(0..8).map(|i| i as f32 * 0.125 + 0.1).collect::<Vec<_>>())
        .unwrap();
    let last = code.residual_sketch.as_ref().unwrap().packed_signs.len() - 1;
    code.residual_sketch.as_mut().unwrap().packed_signs[last] |= 0b1111_1110;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn qjl_packed_length_rejected() {
    let (q, mut code) = valid_code();
    code.residual_sketch.as_mut().unwrap().packed_signs.clear();
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn qjl_nan_norm_rejected() {
    let (q, mut code) = valid_code();
    code.residual_sketch.as_mut().unwrap().norm = f32::NAN;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn qjl_negative_norm_rejected() {
    let (q, mut code) = valid_code();
    code.residual_sketch.as_mut().unwrap().norm = -0.1;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn query_nan_rejected() {
    let (q, code) = valid_code();
    let mut query = vec![0.1; 8];
    query[0] = f32::NAN;
    assert!(q.inner_product_estimate(&code, &query).is_err());
}

#[test]
fn query_infinity_rejected() {
    let (q, code) = valid_code();
    let mut query = vec![0.1; 8];
    query[0] = f32::INFINITY;
    assert!(q.inner_product_estimate(&code, &query).is_err());
}

#[test]
fn mismatched_polar_residual_dimensions_rejected() {
    let (q, mut code) = valid_code();
    code.residual_sketch.as_mut().unwrap().dim = 16;
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}

#[test]
fn direct_malformed_shapes_rejected() {
    let q = TurboQuantizer::new(8, 8, 8, 42).unwrap();
    let code = TurboCode {
        polar_code: PolarCode {
            dim: 8,
            bits: 7,
            radii: vec![1.0; 4],
            packed_angle_indices: bitpack::pack_indices(&[0; 4], 7).unwrap(),
        },
        residual_sketch: Some(QjlSketch {
            dim: 8,
            projections: 8,
            packed_signs: Vec::new(),
            norm: 1.0,
        }),
    };
    assert!(q.inner_product_estimate(&code, &[0.1; 8]).is_err());
}
