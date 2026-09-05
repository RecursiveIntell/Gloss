use quant_codec_core::*;

#[test]
fn digest_is_stable_for_canonical_bytes() {
    let a = CodecProfileDigest::from_parts(&[b"codec", b"v1", b"profile"]);
    let b = CodecProfileDigest::from_parts(&[b"codec", b"v1", b"profile"]);
    let c = CodecProfileDigest::from_parts(&[b"codecv1", b"profile"]);

    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.to_string().len(), 64);
}

struct MockProfile;

impl CodecProfile for MockProfile {
    fn codec_id(&self) -> CodecId {
        CodecId::new("mock").unwrap()
    }

    fn codec_version(&self) -> &str {
        "0"
    }

    fn profile_digest(&self) -> CodecProfileDigest {
        CodecProfileDigest::from_canonical_bytes(b"mock")
    }

    fn fixed_rate_bits(&self) -> Option<u16> {
        Some(8)
    }

    fn block_dim(&self) -> Option<u16> {
        Some(64)
    }

    fn is_lossy(&self) -> bool {
        true
    }
}

#[test]
fn trait_mock_compiles() {
    let profile = MockProfile;
    assert_eq!(profile.codec_id().as_str(), "mock");
    assert!(profile.is_lossy());
}
