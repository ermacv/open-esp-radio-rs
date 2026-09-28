fn main() {
    oer_esp32s31_platform_layout::build::configure_runtime(
        "oer-esp32s31-example-thread",
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../platform/esp32s31/linker"),
    );
}
