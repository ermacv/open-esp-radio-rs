# ESP32-S31 compiled verification probes

This isolated workspace builds retained Rust entry points for instruction- and
effect-level comparison with caller-owned vendor artifacts. The probes depend
on production driver crates, but no production crate or HIL firmware depends
on the probes.

Build the comparison ELF from the repository root:

```console
CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-register-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-register-probes-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-bluetooth-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-bluetooth-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

`library/` owns stable retained wrappers and explicit C-layout projections.
`elf/` owns only the comparison image entry point and linker layout. Neither is
a board test, runtime adapter or public driver API. Bluetooth production probes
live in the separate `bluetooth-library/` and `bluetooth-elf/` pair so BLE
verification never depends on Wi-Fi/PHY probe wrappers.
