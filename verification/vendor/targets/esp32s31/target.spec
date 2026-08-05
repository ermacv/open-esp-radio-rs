# Public target selection for the current ESP32-S31 validator harness.
schema 1
target esp32s31-rev0
harness esp32s31-radio-v1
architecture riscv32
calling-convention riscv-ilp32
endianness little
pointer-width 32
rust-target riscv32imafc-unknown-none-elf
svd ../../../../svd/esp32s31-radio.svd
svd ../../../../svd/esp32s31-platform-radio-deps.svd
pac-bindings ../../../../svd/esp32s31-radio.bindings
profiles profiles/compiled-equivalence.profile
dispositions dispositions/phy.disposition
evidence-baseline baselines/phy.evidence
