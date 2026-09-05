fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let enabled = |name| std::env::var_os(name).is_some();
    let data = enabled("CARGO_FEATURE_PROFILE_PSRAM_DATA");
    let code = enabled("CARGO_FEATURE_CODE_PSRAM");
    assert!(
        data ^ enabled("CARGO_FEATURE_PROFILE_SRAM_DATA"),
        "select one data profile"
    );
    assert!(
        code ^ enabled("CARGO_FEATURE_CODE_FLASH"),
        "select one code profile"
    );
    oer_firmware::linker::configure_runtime(
        "open-esp-radio-hil-esp32s31-runtime",
        &manifest.join("../../../../platform/esp32s31/linker"),
        data,
        code,
        enabled("CARGO_FEATURE_PSRAM_TASK_STACK"),
    );
}
