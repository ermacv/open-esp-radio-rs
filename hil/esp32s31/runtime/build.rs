use std::{env, path::PathBuf};

const BIN: &str = "open-esp-radio-hil-esp32s31-runtime";

fn main() {
    // `radio_hil` consumes these values through `option_env!`. Explicitly
    // tracking them prevents Cargo from reusing an image configured for a
    // previous AP, PHY vector or traffic mode.
    for variable in [
        "OPEN_RADIO_AMPDU_COALESCE_US",
        "OPEN_RADIO_AMPDU_LIMIT",
        "OPEN_RADIO_AMSDU_BENCH",
        "OPEN_RADIO_FORCE_HT20",
        "OPEN_RADIO_FORCE_HE20",
        "OPEN_RADIO_FORCE_LEGACY_TX",
        "OPEN_RADIO_HT_MCS",
        "OPEN_RADIO_HT_SGI",
        "OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM",
        "OPEN_RADIO_NETWORK_AMSDU_BENCH",
        "OPEN_RADIO_TX_BENCH_RATE_KBPS",
        "OPEN_RADIO_HE_GI_LTF",
        "OPEN_RADIO_HE_DCM_HIL",
        "OPEN_RADIO_HE_DCM_LDPC",
        "OPEN_RADIO_HE_DCM_MCS",
        "OPEN_RADIO_HE_DCM_DATA_POWER_CODE",
        "OPEN_RADIO_HE_DELIMITER_HIL",
        "OPEN_RADIO_HE_LDPC_HIL",
        "OPEN_RADIO_HE_MATRIX_HIL",
        "OPEN_RADIO_HE_MCS",
        "OPEN_RADIO_HE_TB_HIL",
        "OPEN_RADIO_LEGACY_RATE_MBIT",
        "OPEN_RADIO_LAN_PROBE_IPV4",
        "OPEN_RADIO_PERF_AP",
        "OPEN_RADIO_RAW_MAC_BENCH",
        "OPEN_RADIO_STA_CHANNEL",
        "OPEN_RADIO_STA_GATEWAY_IPV4",
        "OPEN_RADIO_STA_IPV4",
        "OPEN_RADIO_STA_PASSWORD",
        "OPEN_RADIO_STA_SSID",
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
