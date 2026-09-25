use std::{env, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let linker_dir = manifest_dir
        .parent()
        .expect("bootstrap must live below the ESP32-S31 platform workspace")
        .join("linker");

    println!("cargo:rerun-if-env-changed=PSRAM_RUNTIME_BIN");
    assert!(
        env::var_os("PSRAM_RUNTIME_BIN").is_some(),
        "PSRAM_RUNTIME_BIN must name the packed stage-two runtime"
    );
    oer_esp32s31_platform_layout::build::configure_bootstrap(
        "oer-esp32s31-platform-bootstrap",
        &linker_dir,
    );
}
