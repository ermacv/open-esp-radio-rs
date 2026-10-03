# ESP32-S31 boot-state probe

A standalone image that reads, on the board, boot state the `ermacv/esp-hal`
fork relies on but neither ESP-IDF nor the PAC settles:

- **TRNG.** Its LP clock, configuration and interrupt words after
  `esp_hal::init`, and four output words.
- **PMA.** Every `pmacfg`/`pmaaddr` pair as the ESP-IDF bootloader left it,
  then after `esp_hal::init`, which programs PMA entry 7 for the 64 MiB
  external-memory aperture. Each line decodes the NAPOT region.
- **Flash MMU page number.** An unused, invalid MMU entry is written with
  10-, 11- and 15-bit page numbers and read back, then restored. The PAC
  publishes a 10-bit `PADDR`; ESP-IDF's `ext_mem_defs.h` names an 11-bit value
  mask and 32768 pages.
- **`PMU.IMM_HP_CK_POWER_1`.** Whether the CPLL tie-high bits read back set
  after a write that keeps the running CPLL powered, or are pulses.
- **Watchdog disable.** TIMG0's watchdog is armed with a 1 s system-reset
  stage, disabled through esp-hal and left for 3 s. A second `PROBE-BOOT`
  before `PROBE-DONE` means the disable did not apply.

The image boots through the ESP-IDF bootloader that `espflash` bundles, the
one production images use. From this directory:

```console
cargo build --release
cargo hil flash --board esp32s31 --air none --image esp32s31-boot-state-probe \
  --monitor 30s --until PROBE-DONE \
  target/riscv32imafc-unknown-none-elf/release/oer-esp32s31-boot-state-probe
```
