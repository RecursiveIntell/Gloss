#[path = "build_support/runtime_identity.rs"]
mod runtime_identity;

fn main() {
    let identity = runtime_identity::semantic_memory_identity(
        std::path::Path::new("Cargo.toml"),
        std::path::Path::new("../Cargo.lock"),
    );
    println!("cargo:rustc-env=GLOSS_SEMANTIC_MEMORY_IDENTITY={identity}");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=../Cargo.lock");
    tauri_build::build()
}
