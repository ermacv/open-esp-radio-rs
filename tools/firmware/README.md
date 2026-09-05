# Staged firmware tooling

`oer-firmware` is the host implementation of the ESP32-S31 image contract.
It configures linker arguments for application build scripts, packs runtime
checksums, validates ELF placement and interrupt entry instructions, checks
compiler stack metadata, validates the ROM image checksum/digest, prepares the
OTA selector, and configures bootstrap/image/flash commands.

It does not execute scenarios, classify HIL images or manage lab devices.
`cargo xtask build firmware` owns application build/flash orchestration. The
HIL runner uses the same mechanisms and adds observer checks, source snapshots,
replay and evidence. The [platform](../../platform/esp32s31/README.md) owns
embedded startup and the board profile.

Run host regressions with `cargo test -p oer-firmware`. Generated firmware and
reports remain in the invoking application's or HIL runner's ignored outputs.
