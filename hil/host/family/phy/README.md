# oer-hil-family-phy

The `[phy]` HIL scenario family. Its one workload, `vendor-calibration`,
captures the vendor-versus-production PHY calibration cross-check on the
board under test. The family names no chip: it records the chip-neutral
captures of [`oer-phy-calibration-capture`](../../../phy-calibration-capture)
and reaches what a chip compares, and which register images both sides
read, through its comparison port (`comparison::Comparison`). A
chip-specific composition implements the port and registers
`oer_hil_family_phy::family::<C>()`: for the ESP32-S31,
[`oer-hil-family-phy-esp32s31`](../phy-esp32s31/README.md) with
[`oer-esp32s31-phy-vendor-calibration`](../../../../crates/hardware/esp32s31/calibration/README.md),
which the runner's family registry lists.

```toml
[phy]
kind = "vendor-calibration"
lifecycle = "cold"            # restart, ieee802154, ieee802154-restart
boots = 10
# image = "correctness"       # the lifecycle point's own by default
# vendor_project = "calibration"
# vendor_windows = [{ address = 0x2010fc00, words = 2 }]
# vendor_transmit = true
# vendor_only = true
```

Each round flashes the vendor firmware (built by the image pipeline's
pinned ESP-IDF build, `oer_image::esp_idf::idf`, inside the run) and then the
scenario's own image back, through the run's
`oer_hil_workload::context::BoardImages`: the flash operation under the run's
lock of the board, journaled. After a scenario that wrote another image, the
runner flashes the next scenario's image again. Each boot is a typed
observation of the repetition and the comparison its result; a verdict other
than MATCH fails it. The vendor console protocols (the calibration firmware
and the IEEE 802.15.4 reference firmware) are read here; the comparison is a
call through the port.

```console
cargo test -p oer-hil-family-phy
cargo hil plan phy-vendor-calibration
```
