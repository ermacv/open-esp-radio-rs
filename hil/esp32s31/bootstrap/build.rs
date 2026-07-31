use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let linker_dir = manifest_dir
        .parent()
        .expect("bootstrap must live below the ESP32-S31 HIL workspace")
        .join("linker");

    for file in [
        "../linker/rom/esp32s31-eco0.x",
        "../linker/bootstrap/link.x",
        "../linker/bootstrap/memory.x",
        "../linker/bootstrap/sections.x",
        "../linker/bootstrap/flash-sections.x",
        "../linker/bootstrap/psram-sections.x",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-env-changed=PSRAM_RUNTIME_BIN");
    assert!(
        env::var_os("PSRAM_RUNTIME_BIN").is_some(),
        "PSRAM_RUNTIME_BIN must name the packed stage-two runtime"
    );

    println!("cargo:rustc-link-search={}", linker_dir.display());
    for argument in ["-Trom/esp32s31-eco0.x", "-Tbootstrap/link.x", "--nmagic"] {
        println!("cargo:rustc-link-arg-bin=open-esp-radio-hil-esp32s31-bootstrap={argument}");
    }
}
