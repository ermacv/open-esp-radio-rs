# esp32s31 coupling and the esp-hal/esp-pacs forks: ESP32-C5 impact

## esp-hal fork (ermacv/esp-hal a228e858)
- upstream esp-rs/esp-hal 0eb3e53b (2026-09-04) + 5 S31 commits, 26 files, +1226/-107.
- Every change is cfg(esp32s31) except the opt-in `__ethernet` feature (no C5 effect).
  For C5 the fork == upstream 0eb3e53b.
- S31-only fork APIs our code uses: esp_hal::flash (4), psram::prepare_code (1),
  interrupt::reinitialize_vectoring_after_handoff (1), rng::Trng/TrngSource (6,
  S31 LP TRNG path). All in S31 platform/adapter code; C5 does not need them
  with the standard esp-hal boot.
- upstream is 61 commits ahead; C5-relevant: radio clock refcount/cleanup
  (5af7bdf6d, 05690aecc: esp-radio clocks_ll/esp32c5.rs), PAC update to released
  versions (ed83ac5c5), "Enable WiFi 6 for ESP32-C5" (7a60b5889), BLE modem sleep.
  We don't use esp-radio, but a later fork rebase will move the esp32c5 PAC pin.
- esp_hal::init on C5 already sets up the modem clock domain (esp-radio's C5
  init_clocks is a no-op); IEEE 802.15.4 needs only MODEM_SYSCON clk_zb_apb/zbmac.

## esp-pacs fork (ermacv/esp-pacs 507d14db)
- root [patch] replaces only the `esp32s31` crate (radio interrupt sources).
- `esp32c5` comes from esp-rs/esp-pacs 5cd68d2 through esp-hal. A C5 radio
  interrupt source publication, if needed, needs the same kind of fork commit.

## Repository coupling (code lines, comments excluded)
- tools/xtask (188): per-chip `match` in vendor_fetch/evidence/vendor_scenario/
  vendor_provenance; 8 CLI defaults `esp32s31`; firmware.rs builds hard-coded S31
  images; architecture/unsafe/facade/network/phy checks list S31 package names.
- hil/host/runner-core (117) + runner (17) + arbiter tests (22): paths
  hil/targets/esp32s31, target/hil/esp32s31, platform/esp32s31, target "esp32s31",
  uses oer_esp32s31_firmware (image packing, bootstrap, device lease).
- tools/firmware (package oer-esp32s31-firmware, 13): S31 staged boot image
  packing, TARGET riscv32imafc, espflash --chip esp32s31, S31 layout crate.
- qualification/evaluator (77): target == "esp32s31" checks, catalog/target paths.
- root Cargo.toml (41): members list and the esp32s31 esp-pacs [patch].
- crates/oer facade (18): chip-named features (esp32s31, esp32s31-wifi, ...).
- CI (21): S31-only jobs; no C5 build, register check or target.
- rust-toolchain: riscv32imac added (done).
- portable protocols (4): Bluetooth tests use a `S31` ConnectionAllowances fixture.
- chip-named trees (expected): crates/*/esp32s31, registers/esp32s31,
  verification/esp32s31, platform/esp32s31, hil/targets/esp32s31, examples/esp32s31.
