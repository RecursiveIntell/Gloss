//! Integration tests for boundary-compiler RFC 8785 JCS canonicalization.

use boundary_compiler::{parse_with_dup_check, Canonicalizer, ContentDigest};
use serde_json::json;

#[test]
fn decoded_duplicate_keys_are_object_scoped() {
    assert!(parse_with_dup_check(r#"{"a":1,"\u0061":2}"#).is_err());
    assert!(parse_with_dup_check(r#"{"left":{"x":1},"right":{"x":2}}"#).is_ok());
}

#[test]
fn duplicate_error_reports_the_decoded_property_name() {
    let error = parse_with_dup_check(r#"{"na\u006de":1,"name":2}"#).unwrap_err();
    assert!(matches!(
        error,
        boundary_compiler::JcsError::DuplicateKey { ref key } if key == "name"
    ));
}

#[test]
fn jcs_utf16_and_ecmascript_number_vectors() {
    let value = parse_with_dup_check(r#"{"\uE000":1,"\uD800\uDC00":2}"#).unwrap();
    assert_eq!(
        Canonicalizer::new().canonicalize(&value).unwrap(),
        "{\"𐀀\":2,\"\":1}"
    );
    assert_eq!(
        Canonicalizer::new()
            .canonicalize(&parse_with_dup_check("-0").unwrap())
            .unwrap(),
        "0"
    );
    assert_eq!(
        Canonicalizer::new()
            .canonicalize(&parse_with_dup_check("9007199254740993").unwrap())
            .unwrap(),
        "9007199254740992"
    );
}

#[test]
fn rfc8785_preserves_non_ascii_control_code_points() {
    let value = serde_json::Value::String("\u{0085}".to_owned());
    assert_eq!(
        Canonicalizer::new().canonicalize(&value).unwrap(),
        "\"\u{0085}\""
    );
}

#[test]
fn rfc8785_appendix_b_number_vectors() {
    // RFC 8785 Appendix B, excluding the non-finite values forbidden by JSON.
    let cases = [
        (0x0000_0000_0000_0000, "0"),
        (0x8000_0000_0000_0000, "0"),
        (0x0000_0000_0000_0001, "5e-324"),
        (0x8000_0000_0000_0001, "-5e-324"),
        (0x7fef_ffff_ffff_ffff, "1.7976931348623157e+308"),
        (0xffef_ffff_ffff_ffff, "-1.7976931348623157e+308"),
        (0x4340_0000_0000_0000, "9007199254740992"),
        (0xc340_0000_0000_0000, "-9007199254740992"),
        (0x4430_0000_0000_0000, "295147905179352830000"),
        (0x44b5_2d02_c7e1_4af5, "9.999999999999997e+22"),
        (0x44b5_2d02_c7e1_4af6, "1e+23"),
        (0x44b5_2d02_c7e1_4af7, "1.0000000000000001e+23"),
        (0x444b_1ae4_d6e2_ef4e, "999999999999999700000"),
        (0x444b_1ae4_d6e2_ef4f, "999999999999999900000"),
        (0x444b_1ae4_d6e2_ef50, "1e+21"),
        (0x3eb0_c6f7_a0b5_ed8c, "9.999999999999997e-7"),
        (0x3eb0_c6f7_a0b5_ed8d, "0.000001"),
        (0x41b3_de43_5555_5553, "333333333.3333332"),
        (0x41b3_de43_5555_5554, "333333333.33333325"),
        (0x41b3_de43_5555_5555, "333333333.3333333"),
        (0x41b3_de43_5555_5556, "333333333.3333334"),
        (0x41b3_de43_5555_5557, "333333333.33333343"),
        (0xbecb_f647_612f_3696, "-0.0000033333333333333333"),
        (0x4314_3ff3_c1cb_0959, "1424953923781206.2"),
    ];

    let canonicalizer = Canonicalizer::new();
    for (bits, expected) in cases {
        let number = serde_json::Number::from_f64(f64::from_bits(bits)).unwrap();
        assert_eq!(
            canonicalizer
                .canonicalize(&serde_json::Value::Number(number))
                .unwrap(),
            expected,
            "IEEE-754 bits {bits:016x}"
        );
    }
}

#[test]
fn rfc8785_property_order_vector() {
    let value = parse_with_dup_check(
        r#"{"\u20ac":"Euro Sign","\r":"Carriage Return","\ufb33":"Hebrew Letter Dalet With Dagesh","1":"One","\ud83d\ude00":"Emoji: Grinning Face","\u0080":"Control","\u00f6":"Latin Small Letter O With Diaeresis"}"#,
    )
    .unwrap();
    assert_eq!(
        Canonicalizer::new().canonicalize(&value).unwrap(),
        "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{0080}\":\"Control\",\"ö\":\"Latin Small Letter O With Diaeresis\",\"€\":\"Euro Sign\",\"😀\":\"Emoji: Grinning Face\",\"דּ\":\"Hebrew Letter Dalet With Dagesh\"}"
    );
}

#[test]
fn schema_validator_fails_closed_without_schema() {
    assert!(boundary_compiler::SchemaValidator::new()
        .validate(&json!({}))
        .is_err());
}

/// Test 1: rfc8785_basic_object — {"b":1,"a":2} → keys sorted to {"a":2,"b":1}.
#[test]
fn rfc8785_basic_object() {
    let c = Canonicalizer::new();
    let val = json!({"b": 1, "a": 2});
    let canonical = c.canonicalize(&val).unwrap();
    assert_eq!(canonical, r#"{"a":2,"b":1}"#);
}

/// Test 2: rfc8785_nested — nested objects sort recursively.
#[test]
fn rfc8785_nested() {
    let c = Canonicalizer::new();
    let val = json!({
        "z": {"b": 1, "a": 2},
        "m": [3, 2, 1],
        "a": "hello"
    });
    let canonical = c.canonicalize(&val).unwrap();
    // Keys at top level: a, m, z (alphabetical)
    // "z" nested has keys: a, b (alphabetical)
    assert_eq!(canonical, r#"{"a":"hello","m":[3,2,1],"z":{"a":2,"b":1}}"#);
}

/// Test 3: rfc8785_arrays_preserve_order — [3,1,2] stays [3,1,2].
#[test]
fn rfc8785_arrays_preserve_order() {
    let c = Canonicalizer::new();
    let val = json!([3, 1, 2]);
    let canonical = c.canonicalize(&val).unwrap();
    // Arrays are NOT sorted — order is preserved per RFC 8785
    assert_eq!(canonical, r#"[3,1,2]"#);
}

/// Test 4: rfc8785_number_formatting — standard number serialization.
#[test]
fn rfc8785_number_formatting() {
    let c = Canonicalizer::new();
    // Integer
    assert_eq!(c.canonicalize(&json!(42)).unwrap(), "42");
    // Negative
    assert_eq!(c.canonicalize(&json!(-1)).unwrap(), "-1");
    // Float
    assert_eq!(c.canonicalize(&json!(0.5)).unwrap(), "0.5");
    // Scientific notation if serde_json produces it
    let scientific = json!(1e10);
    let result = c.canonicalize(&scientific).unwrap();
    assert!(result.contains("1e") || result.contains("10"));
}

/// Test 5: duplicate_key_rejection — {"a":1,"a":2} → Err.
#[test]
fn duplicate_key_rejection() {
    // JSON with duplicate keys at same nesting level
    let result = parse_with_dup_check(r#"{"a": 1, "a": 2}"#);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.to_string().contains("duplicate") || err.to_string().contains("DuplicateKey"));
}

/// Test: nested duplicate keys are also rejected.
#[test]
fn duplicate_key_nested_rejection() {
    let result = parse_with_dup_check(r#"{"outer": {"x": 1, "x": 2}}"#);
    assert!(result.is_err());
}

/// Test: ContentDigest computes for canonical JSON.
#[test]
fn content_digest_computation() {
    let val = json!({"b": 2, "a": 1});
    let digest = ContentDigest::compute(&val).unwrap();
    // Digest hex should be non-empty
    let hex = digest.hex();
    assert!(!hex.is_empty());
    // Should be deterministic — same input always gives same digest
    let digest2 = ContentDigest::compute(&val).unwrap();
    assert_eq!(digest.hex(), digest2.hex());
}

/// Test: canonicalization with parse_and_validate accepts valid JSON.
#[test]
fn parse_and_validate_accepts_valid() {
    let input = r#"{"z": 1, "a": 2, "m": 3}"#;
    let val = boundary_compiler::parse_and_validate(input).unwrap();
    let c = Canonicalizer::new();
    let canonical = c.canonicalize(&val).unwrap();
    assert_eq!(canonical, r#"{"a":2,"m":3,"z":1}"#);
}
