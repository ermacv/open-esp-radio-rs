use oer_esp32s31_platform_layout::{
    build,
    memory::{CodePlacement, DataPlacement, RuntimeProfile},
};

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let enabled = |name| std::env::var_os(name).is_some();
    let psram_data = enabled("CARGO_FEATURE_PROFILE_PSRAM_DATA");
    let psram_code = enabled("CARGO_FEATURE_CODE_PSRAM");
    assert!(
        psram_data ^ enabled("CARGO_FEATURE_PROFILE_SRAM_DATA"),
        "select one data profile"
    );
    assert!(
        psram_code ^ enabled("CARGO_FEATURE_CODE_FLASH"),
        "select one code profile"
    );
    let profile = RuntimeProfile::new(
        if psram_code {
            CodePlacement::Psram
        } else {
            CodePlacement::Flash
        },
        if psram_data {
            DataPlacement::Psram
        } else {
            DataPlacement::Sram
        },
        enabled("CARGO_FEATURE_PSRAM_TASK_STACK"),
    )
    .unwrap_or_else(|error| panic!("unsupported runtime placement profile: {error:?}"));
    build::configure_runtime(
        "oer-hil-esp32s31-runtime",
        &manifest.join("../../../../platform/esp32s31/linker"),
        profile,
    );
}
