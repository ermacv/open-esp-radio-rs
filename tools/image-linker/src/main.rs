//! The image linker: rustc runs it in place of `rust-lld` for every
//! ESP32-S31 image (`-C linker`, set by
//! `oer_esp32s31_firmware::compiler::configure_image_compiler`).

fn main() {
    std::process::exit(oer_image_linker::run(std::env::args_os().skip(1).collect()));
}
