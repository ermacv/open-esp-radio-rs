use std::{env, path::PathBuf};

const BIN: &str = "open-esp-radio-hil-esp32s31-runtime";

fn main() {
    // The product HIL uses compile-time switches only where a workload changes
    // static socket-buffer geometry. Credentials and network policy are sent
    // at runtime and must not affect image identity.
    for variable in [
        "OPEN_RADIO_BIDIRECTIONAL_BENCH",
        "OPEN_RADIO_TCP_BENCH",
        "OPEN_RADIO_TX_BENCH",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let linker_dir = manifest_dir
        .parent()
        .expect("runtime must live below the ESP32-S31 HIL workspace")
        .join("linker");

    for file in [
        "../linker/rom/esp32s31-eco0.x",
        "../linker/runtime/link.x",
        "../linker/runtime/memory.x",
        "../linker/runtime/sections.x",
    ] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rustc-link-search={}", linker_dir.display());
    for argument in ["-Trom/esp32s31-eco0.x", "-Truntime/link.x", "--nmagic"] {
        println!("cargo:rustc-link-arg-bin={BIN}={argument}");
    }

    let psram_data = env::var_os("CARGO_FEATURE_PROFILE_PSRAM_DATA").is_some();
    let sram_data = env::var_os("CARGO_FEATURE_PROFILE_SRAM_DATA").is_some();
    let flash_code = env::var_os("CARGO_FEATURE_CODE_FLASH").is_some();
    let psram_code = env::var_os("CARGO_FEATURE_CODE_PSRAM").is_some();
    assert!(
        psram_data ^ sram_data,
        "select exactly one data profile: profile-psram-data or profile-sram-data"
    );
    assert!(
        flash_code ^ psram_code,
        "select exactly one code profile: code-flash or code-psram"
    );
    assert!(
        !flash_code || psram_data,
        "the Flash-code profile requires profile-psram-data"
    );

    let (code_origin, code_length): (u32, u32) = if psram_code {
        (0x5001_0000, 0x00ff_0000)
    } else {
        (0x4000_0140, 0x03ff_fec0)
    };
    let (data_origin, data_length): (u32, u32) = if psram_data {
        (0x5001_0000, 0x00ff_0000)
    } else {
        (0x2f00_0000, 0x0006_afc0)
    };
    for (symbol, value) in [
        ("RUNTIME_CODE_IN_PSRAM", u32::from(psram_code)),
        ("RUNTIME_CODE_ORIGIN", code_origin),
        ("RUNTIME_CODE_LENGTH", code_length),
        ("RUNTIME_DATA_IN_PSRAM", u32::from(psram_data)),
        ("RUNTIME_DATA_ORIGIN", data_origin),
        ("RUNTIME_DATA_LENGTH", data_length),
    ] {
        println!("cargo:rustc-link-arg-bin={BIN}=--defsym={symbol}={value}");
    }
}
