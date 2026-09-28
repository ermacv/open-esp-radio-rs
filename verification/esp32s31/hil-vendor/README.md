# ESP32-S31 vendor firmware

ESP-IDF applications that run vendor code on the HIL board, so hardware
cross-checks compare vendor and production behavior on the same device.
`cargo xtask vendor-firmware --chip esp32s31 [PROJECT]` builds them against the
pinned ESP-IDF and the pinned vendor archives of
[`artifacts.toml`](../artifacts.toml); outputs and `build.json` stay in
`target/vendor-firmware/esp32s31/<project>/`. A project with a `firmware.toml`
is also an entry of the stand's firmware catalog, so `cargo hil firmware flash
<image> --board <board>` builds, flashes and journals it. Each application uses the HIL
partition table, so the HIL runner flashes it into `ota_0` like a production
image.

| Project | Behavior |
| --- | --- |
| `calibration` | Catalog image `vendor-calibration`. Enables the PHY once with full calibration and no stored calibration data, then prints each reported vendor object as `oer-vendor-calibration <object> <hex bytes>` and `oer-vendor-calibration-end` on the USB-Serial/JTAG console. The reported objects and their sizes come from the pinned archives that define them (`main/CMakeLists.txt`); the object is `phy_param` of `libphy.a`. It then answers `r <hex address>` requests with `oer-vendor-calibration-register <address> <value>`; the host chooses the registers. The [calibration cross-check](../hardware/calibration/README.md) consumes both. |
| `ieee802154-reference` | The [ESP32-C5 reference peer](../../../hil/peers/esp32c5-ieee802154/README.md) application built for the ESP32-S31: ESP-IDF's IEEE 802.15.4 driver behind the peer's line protocol, for on-air comparison with the open driver and for the IEEE 802.15.4 points of the calibration cross-check. Its catalog image is `ieee802154-vendor-reference`. |
