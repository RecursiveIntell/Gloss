use boundary_compiler::{BoundaryProfile, JcsError, ResourceCeilings};

fn profile_with(ceilings: ResourceCeilings) -> BoundaryProfile {
    BoundaryProfile::new(ceilings)
}

#[test]
fn every_profile_budget_mutation_changes_admission() {
    let cases = [
        (
            profile_with(ResourceCeilings {
                max_input_bytes: 6,
                ..ResourceCeilings::default()
            }),
            r#"{"a":1}"#,
            "input_bytes",
        ),
        (
            profile_with(ResourceCeilings {
                max_nodes: 2,
                ..ResourceCeilings::default()
            }),
            r#"[1,2]"#,
            "nodes",
        ),
        (
            profile_with(ResourceCeilings {
                max_depth: 1,
                ..ResourceCeilings::default()
            }),
            r#"[[1]]"#,
            "depth",
        ),
        (
            profile_with(ResourceCeilings {
                max_object_keys: 1,
                ..ResourceCeilings::default()
            }),
            r#"{"a":1,"b":2}"#,
            "object_keys",
        ),
        (
            profile_with(ResourceCeilings {
                max_string_bytes: 3,
                ..ResourceCeilings::default()
            }),
            r#""four""#,
            "string_bytes",
        ),
        (
            profile_with(ResourceCeilings {
                max_array_len: 1,
                ..ResourceCeilings::default()
            }),
            r#"[1,2]"#,
            "array_len",
        ),
    ];

    for (profile, input, expected_resource) in cases {
        let error = profile.parse(input).unwrap_err();
        assert!(matches!(
            error,
            JcsError::ResourceCeilingExceeded { ref resource, .. }
                if resource == expected_resource
        ));
    }
}

#[test]
fn successful_admission_receipt_lists_every_enforced_rule() {
    let admission = BoundaryProfile::rfc8785()
        .parse(r#"{"a":["ok",1]}"#)
        .unwrap();
    let rules: Vec<_> = admission
        .receipt
        .enforced_rules
        .iter()
        .map(|rule| rule.rule.as_str())
        .collect();
    assert_eq!(
        rules,
        [
            "input_bytes",
            "nodes",
            "depth",
            "object_keys",
            "string_bytes",
            "array_len",
            "duplicate_keys",
            "canonicalization",
        ]
    );
    assert_eq!(admission.receipt.canonicalization_profile, "rfc8785");
}
