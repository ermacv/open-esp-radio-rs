//! Provenance belongs to the linked host; publication never hashes today's source as its identity.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, env, fs, path::Path};
fn visit(root: &Path, path: &Path, inputs: &mut BTreeMap<String, String>) {
    println!("cargo:rerun-if-changed={}", path.display());
    if path.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name();
            if matches!(
                name.to_str(),
                Some("tests" | "tests.rs" | "test_support.rs")
            ) {
                continue;
            }
            if entry.path().is_dir()
                || entry.path().extension().is_some_and(|e| e == "rs")
                || name == "Cargo.toml"
            {
                visit(root, &entry.path(), inputs);
            }
        }
    } else {
        inputs.insert(
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
        );
    }
}
fn main() {
    let manifest = std::path::PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../../../../..").canonicalize().unwrap();
    let path = manifest.join("../model-inputs.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let registry: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let mut inputs = BTreeMap::new();
    for mechanism in registry["mechanisms"].as_object().unwrap().values() {
        for path in mechanism["implementation"].as_array().unwrap() {
            visit(&root, &root.join(path.as_str().unwrap()), &mut inputs);
        }
    }
    fs::write(
        std::path::PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("model-inputs.json"),
        serde_json::to_vec(&inputs).unwrap(),
    )
    .unwrap();
}
