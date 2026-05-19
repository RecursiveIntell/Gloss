use turbo_quant::{QjlQuantizer, TurboCodeWireV1, TurboQuantizer};

#[test]
fn polar_angles_are_bitpacked_in_encoded_size() {
    let q = turbo_quant::PolarQuantizer::new(16, 3, 42).unwrap();
    let code = q.encode(&[0.25; 16]).unwrap();
    assert_eq!(code.packed_angle_indices.len(), (8usize * 3).div_ceil(8));
    assert_eq!(code.encoded_bytes(), 8 * 4 + 3);
}

#[test]
fn qjl_signs_are_bitpacked_in_encoded_size() {
    let q = QjlQuantizer::new(16, 17, 42).unwrap();
    let sketch = q.sketch(&[0.25; 16]).unwrap();
    assert_eq!(sketch.packed_signs.len(), 17usize.div_ceil(8));
    assert_eq!(sketch.encoded_bytes(), 17usize.div_ceil(8) + 4);
}

#[test]
fn turbo_payload_size_uses_packed_payload_not_vec_widths() {
    let q = TurboQuantizer::new(16, 4, 17, 42).unwrap();
    let code = q.encode(&[0.25; 16]).unwrap();
    let expected = (8 * 4) + (8usize * 3).div_ceil(8) + 17usize.div_ceil(8) + 4;
    assert_eq!(code.encoded_bytes(), expected);
    assert!(TurboCodeWireV1::encode(&code, &q).unwrap().len() > code.encoded_bytes());
}
