fn main() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("link.x");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg=-T{}", script.display());
}
