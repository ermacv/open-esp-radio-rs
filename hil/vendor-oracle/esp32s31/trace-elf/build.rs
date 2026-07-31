use std::{env, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let repository = manifest
        .ancestors()
        .nth(4)
        .expect("trace ELF must live below hil/vendor-oracle/esp32s31");
    let script = manifest.join("link.x");
    let rom = repository.join("hil/esp32s31/linker/rom/esp32s31-eco0.x");
    let archive = repository.join("_oracles/libphy.a");

    for input in [&script, &rom, &archive] {
        println!("cargo:rerun-if-changed={}", input.display());
    }
    for argument in [
        format!("-T{}", rom.display()),
        format!("-T{}", script.display()),
        "--no-gc-sections".to_owned(),
        "--emit-relocs".to_owned(),
        "--whole-archive".to_owned(),
        archive.display().to_string(),
        "--no-whole-archive".to_owned(),
        "--unresolved-symbols=ignore-all".to_owned(),
    ] {
        println!("cargo:rustc-link-arg={argument}");
    }
}
