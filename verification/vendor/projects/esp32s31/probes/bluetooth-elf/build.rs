fn main() {
    // Cargo can reuse this compiled build script after the package is moved.
    // Resolve the current invocation's manifest directory, not its build path.
    let directory = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("Cargo supplies the package manifest directory");
    let script = std::path::PathBuf::from(directory).join("link.x");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg=-T{}", script.display());
}
