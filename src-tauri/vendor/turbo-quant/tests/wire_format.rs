use turbo_quant::{TurboCodeWireV1, TurboQuantizer, TURBO_CODE_WIRE_MAGIC};

const RESERVED_OFFSET: usize = 9;
const RESERVED2_OFFSET: usize = 15;
const SEED_OFFSET: usize = 22;
const PAYLOAD_LEN_OFFSET: usize = 38;
const PAYLOAD_START_OFFSET: usize = 46;

fn fixture() -> (TurboQuantizer, Vec<f32>) {
    let q = TurboQuantizer::new(384, 8, 96, 7).unwrap();
    let vector = (0..384)
        .map(|i| ((i as f32 * 0.017).sin() * 0.5) + 0.01)
        .collect();
    (q, vector)
}

fn encoded_fixture() -> (TurboQuantizer, Vec<u8>) {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    (q, encoded)
}

#[test]
fn roundtrip_preserves_approximate_score() {
    let (q, vector) = fixture();
    let query = vector.iter().map(|v| v * 0.9 + 0.01).collect::<Vec<_>>();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    let decoded = TurboCodeWireV1::decode(&encoded, &q).unwrap();
    let before = q.inner_product_estimate(&code, &query).unwrap();
    let after = q.inner_product_estimate(&decoded, &query).unwrap();
    assert!((before - after).abs() < 1e-5);
}

#[test]
fn deterministic_encoding() {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    assert_eq!(
        TurboCodeWireV1::encode(&code, &q).unwrap(),
        TurboCodeWireV1::encode(&code, &q).unwrap()
    );
}

#[test]
fn wrong_magic_rejected() {
    let (q, mut encoded) = encoded_fixture();
    encoded[..4].copy_from_slice(b"BAD!");
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn wrong_version_rejected() {
    let (q, mut encoded) = encoded_fixture();
    encoded[4..6].copy_from_slice(&2u16.to_le_bytes());
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn trailing_byte_rejected() {
    let (q, mut encoded) = encoded_fixture();
    encoded.push(0);
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn dimension_mismatch_rejected() {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    let other = TurboQuantizer::new(192, 8, 96, 7).unwrap();
    assert!(TurboCodeWireV1::decode(&encoded, &other).is_err());
}

#[test]
fn bit_width_mismatch_rejected() {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    let other = TurboQuantizer::new(384, 7, 96, 7).unwrap();
    assert!(TurboCodeWireV1::decode(&encoded, &other).is_err());
}

#[test]
fn projection_mismatch_rejected() {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    let other = TurboQuantizer::new(384, 8, 128, 7).unwrap();
    assert!(TurboCodeWireV1::decode(&encoded, &other).is_err());
}

#[test]
fn seed_mismatch_rejected() {
    let (q, mut encoded) = encoded_fixture();
    encoded[SEED_OFFSET..SEED_OFFSET + 8].copy_from_slice(&8u64.to_le_bytes());
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn reserved_bytes_rejected() {
    let (q, mut encoded) = encoded_fixture();
    encoded[RESERVED_OFFSET] = 1;
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());

    let (q, mut encoded) = encoded_fixture();
    encoded[RESERVED2_OFFSET] = 1;
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn payload_length_too_small_or_large_rejected() {
    let (q, mut encoded) = encoded_fixture();
    let payload_len = u64::from_le_bytes(
        encoded[PAYLOAD_LEN_OFFSET..PAYLOAD_LEN_OFFSET + 8]
            .try_into()
            .unwrap(),
    );
    encoded[PAYLOAD_LEN_OFFSET..PAYLOAD_LEN_OFFSET + 8]
        .copy_from_slice(&(payload_len - 1).to_le_bytes());
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());

    let (q, mut encoded) = encoded_fixture();
    encoded[PAYLOAD_LEN_OFFSET..PAYLOAD_LEN_OFFSET + 8]
        .copy_from_slice(&(payload_len + 1).to_le_bytes());
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn qjl_padding_bits_rejected() {
    let q = TurboQuantizer::new(384, 8, 97, 7).unwrap();
    let vector = (0..384)
        .map(|i| ((i as f32 * 0.019).cos() * 0.4) + 0.02)
        .collect::<Vec<_>>();
    let code = q.encode(&vector).unwrap();
    let mut encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    let polar_block_count = q.dim() / 2;
    let angle_bytes = (polar_block_count * (q.bits() as usize - 1)).div_ceil(8);
    let sign_offset = PAYLOAD_START_OFFSET + polar_block_count * 4 + angle_bytes;
    let final_sign_byte = sign_offset + q.projections().div_ceil(8) - 1;
    encoded[final_sign_byte] |= 0b1111_1110;
    assert!(TurboCodeWireV1::decode(&encoded, &q).is_err());
}

#[test]
fn encoded_bytes_less_than_raw_f32() {
    let (q, vector) = fixture();
    let code = q.encode(&vector).unwrap();
    let encoded = TurboCodeWireV1::encode(&code, &q).unwrap();
    assert_eq!(&encoded[..4], TURBO_CODE_WIRE_MAGIC);
    assert!(encoded.len() < vector.len() * std::mem::size_of::<f32>());
}
