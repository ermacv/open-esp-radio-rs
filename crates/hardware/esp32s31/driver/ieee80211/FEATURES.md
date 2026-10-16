# ESP32-S31 Wi-Fi capability entry point

The ESP32-S31 Wi-Fi driver has no catalog inventory and no qualification
program. The old S31 Wi-Fi stack (station and access-point roles, runtime and
composition), its Wi-Fi entries in
[the Wi-Fi/PHY catalog](../../../../../qualification/catalog/esp32s31/wifi-phy.toml)
and its verification were deleted (#762); the tag `legacy-wifi-2026-10-16`
keeps them as source material. That catalog still holds the shared PHY entries.

Architecture navigation: [whole-radio map](../FEATURES.md),
[MAC TX](mac/src/tx.rs), [MAC RX](mac/src/rx.rs), and
[shared PHY](../../phy/FEATURES.md).

## Lower-MAC core

The driver core `LowerMacCore` lives in [`src/lower_mac.rs`](src/lower_mac.rs).
No ESP32-S31 backend implements the portable lower-MAC port yet; the port's
operation mapping and limits are in
[the port's README](../../../../protocols/ieee80211/lower-mac/README.md#implementers).
