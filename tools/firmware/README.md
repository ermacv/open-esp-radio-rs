# Staged firmware tooling

`oer-firmware` is the host implementation of the ESP32-S31 image contract.
It configures linker arguments for application build scripts, packs runtime
checksums, validates ELF placement and interrupt entry instructions, checks
compiler stack metadata, validates the ROM image checksum/digest, prepares the
OTA selector, and configures bootstrap/image/flash commands.

The optional `device` feature provides serial-device selection and a lease shared
by xtask and HIL. The lease spans all writes and optional monitoring, uses USB
identity or the canonical serial path, and lives in the user's host cache so
separate checkouts cannot independently claim the same device. Automatic
selection requires exactly one USB serial device; otherwise supply `--port`.
Embedded build-script consumers disable default features.

It does not execute scenarios, classify HIL images or configure network fixtures.
`cargo xtask build firmware` owns application build/flash orchestration. The
HIL runner uses the same mechanisms and adds observer checks, source snapshots,
replay and evidence. The [platform](../../platform/esp32s31/README.md) owns
embedded startup and the board profile.

Run host regressions with `cargo test -p oer-firmware`. Generated firmware and
reports remain in the invoking application's or HIL runner's ignored outputs.
