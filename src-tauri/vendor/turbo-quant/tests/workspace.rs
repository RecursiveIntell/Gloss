use turbo_quant::TurboQuantizer;

#[test]
fn crate_builds_as_its_own_workspace_root() {
    let profile = TurboQuantizer::new(16, 8, 8, 42).unwrap().profile();
    assert_eq!(profile.crate_name, "turbo-quant");
    assert_eq!(profile.crate_version, env!("CARGO_PKG_VERSION"));
}
