use std::path::Path;

pub fn semantic_memory_identity(manifest_path: &Path, lock_path: &Path) -> String {
    let manifest: toml::Value = std::fs::read_to_string(manifest_path)
        .expect("read Gloss manifest")
        .parse()
        .expect("parse Gloss manifest");
    let pin = manifest["dependencies"]["semantic-memory"]["version"]
        .as_str()
        .expect("semantic-memory must have an exact version pin");
    let version = pin
        .strip_prefix('=')
        .expect("semantic-memory runtime identity requires an exact version pin");
    let lock: toml::Value = std::fs::read_to_string(lock_path)
        .expect("read canonical workspace lock")
        .parse()
        .expect("parse canonical workspace lock");
    let packages = lock["package"].as_array().expect("lock packages");
    let matches: Vec<_> = packages
        .iter()
        .filter(|package| package["name"].as_str() == Some("semantic-memory"))
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "ambiguous semantic-memory runtime identity"
    );
    let package = matches[0];
    assert_eq!(
        package["version"].as_str(),
        Some(version),
        "manifest/lock runtime identity mismatch"
    );
    assert_eq!(
        package["source"].as_str(),
        Some("registry+https://github.com/rust-lang/crates.io-index")
    );
    let checksum = package["checksum"].as_str().expect("registry checksum");
    format!("semantic-memory {version} registry-sha256:{checksum}")
}
