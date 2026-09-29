# ESP32-C5 vendor firmware

ESP-IDF applications that run vendor code on the stand's ESP32-C5, so
hardware cross-checks observe the device under test from a vendor
implementation on the air. `cargo xtask vendor-firmware --chip esp32c5
[PROJECT]` builds them against the pinned ESP-IDF and the pinned vendor
archives of [`artifacts.toml`](../artifacts.toml); outputs stay in
`target/vendor-firmware/esp32c5/<project>/`. A project with a `firmware.toml`
is also an entry of the stand's firmware catalog, so `cargo hil firmware flash
<image> --board esp32c5` builds, flashes and journals it.

| Project | Behavior |
| --- | --- |
| `ble-scan-request-counter` | Catalog image `ble-scan-request-counter`. The pinned BLE Controller over VHCI answers `ADV <ms>` (at most 600000 ms) by advertising a scannable legacy set (`ADV_SCAN_IND`, 30 ms, local name `OERSR` in the scan response) through the extended advertising commands with scan-request notification, then prints `@SCANREQ total=<n>` and one `@SCANNER <address type> <address> <count>` line per scanner. `SYNC` resets the Controller and prints `@READY` and the public address as `@ADDR`. An active scanner's `SCAN_REQ` transmissions are therefore counted on the air independently of its own receiver. |
