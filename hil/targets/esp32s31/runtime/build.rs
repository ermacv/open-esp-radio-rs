fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    oer_esp32s31_platform_layout::build::configure_runtime(
        "oer-hil-esp32s31-runtime",
        &manifest.join("../../../../platform/esp32s31/linker"),
    );
}
