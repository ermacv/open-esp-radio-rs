fn main() {
    oer_firmware::linker::configure_runtime(
        "oer-example-esp32s31-monitor",
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../platform/esp32s31/linker"),
        true,
        true,
        true,
    );
}
