# Public generic target selection for ESP32-S31 binary analysis.
schema 1
target esp32s31-rev0
architecture riscv32
calling-convention riscv-ilp32
endianness little
pointer-width 32
rust-target riscv32imafc-unknown-none-elf
memory-map memory.toml
svd ../../../../svd/esp32s31-radio.svd
svd ../../../../svd/esp32s31-platform-radio-deps.svd
pac-bindings ../../../../svd/esp32s31-radio.bindings
profiles profiles/compiled-equivalence.profile
dispositions dispositions/phy.disposition
evidence-baseline baselines/phy.evidence
