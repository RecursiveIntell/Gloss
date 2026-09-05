#[path = "../../src-tauri/build_support/runtime_identity.rs"]
mod runtime_identity;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let identity = runtime_identity::semantic_memory_identity(
        &root.join("src-tauri/Cargo.toml"),
        &root.join("Cargo.lock"),
    );
    let root_lock: toml::Value = std::fs::read_to_string(root.join("Cargo.lock"))
        .unwrap()
        .parse()
        .unwrap();
    let gate_lock: toml::Value = std::fs::read_to_string("Cargo.lock")
        .unwrap()
        .parse()
        .unwrap();
    let root_packages = root_lock["package"].as_array().unwrap();
    for package in gate_lock["package"].as_array().unwrap() {
        if package.get("source").is_none() {
            continue;
        }
        assert!(
            root_packages
                .iter()
                .any(|current| current.get("name") == package.get("name")
                    && current.get("version") == package.get("version")
                    && current.get("source") == package.get("source")
                    && current.get("checksum") == package.get("checksum")),
            "runtime gate lock drift: {} {}",
            package["name"],
            package["version"]
        );
    }
    println!("cargo:rustc-env=GLOSS_SEMANTIC_MEMORY_IDENTITY={identity}");
    println!("cargo:rerun-if-changed=Cargo.lock");
    println!("cargo:rerun-if-changed=../../Cargo.lock");
    println!("cargo:rerun-if-changed=../../src-tauri/Cargo.toml");
    println!("cargo:rerun-if-changed=../../src-tauri/build_support/runtime_identity.rs");
}
