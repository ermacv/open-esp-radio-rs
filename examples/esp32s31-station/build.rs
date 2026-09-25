fn main() {
    oer_esp32s31_platform_layout::build::configure_runtime(
        "oer-example-esp32s31-station",
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/esp32s31/linker"),
        oer_esp32s31_platform_layout::memory::RuntimeProfile::STANDALONE,
    );
}
