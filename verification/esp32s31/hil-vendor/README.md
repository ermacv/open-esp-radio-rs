# ESP32-S31 vendor firmware

ESP-IDF applications that run vendor code on the HIL board, so hardware
cross-checks compare vendor and production behavior on the same device.
`cargo xtask vendor-firmware esp32s31 [PROJECT]` builds them against the
pinned ESP-IDF and the pinned vendor archives of
[`artifacts.toml`](../artifacts.toml); outputs and `build.json` stay in
`target/vendor-firmware/esp32s31/<project>/`. Each application uses the HIL
partition table, so the HIL runner flashes it into `ota_0` like a production
image.

| Project | Behavior |
| --- | --- |
| `calibration` | Enables the PHY once with full calibration and no stored calibration data, then prints each reported vendor object as `oer-vendor-calibration <object> <hex bytes>` and `oer-vendor-calibration-end` on the USB-Serial/JTAG console. The reported objects and their sizes come from the pinned archives that define them (`main/CMakeLists.txt`); today the object is `phy_param` of `libphy.a`. |
