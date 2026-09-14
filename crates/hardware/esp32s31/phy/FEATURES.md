# ESP32-S31 shared PHY capability entry point

The canonical source inventory for RF, analog, calibration, tracking, cache,
power, lifecycle, and protocol composition lives in
[the Wi-Fi/PHY catalog](../../../../qualification/catalog/esp32s31/wifi-phy.toml).
Source-facet status is distinct from composition level and from qualification
readiness.

Generate the complete static inventory without vendor evidence or HIL runs:

```console
cargo qualification catalog render --catalog qualification/catalog/esp32s31/wifi-phy.toml --out target/qualification/catalog/wifi-phy-static
```

The ignored `domain-inventory.md` contains the detailed PHY sections and all
protocol-consumer matrix cells. `migration-map.md` maps each former table row
or matrix cell to one canonical ID.

Architecture navigation: [whole-radio map](../driver/FEATURES.md),
[Wi-Fi](../driver/ieee80211/FEATURES.md),
[Bluetooth](../driver/bluetooth/FEATURES.md),
[IEEE 802.15.4](../driver/ieee802154/FEATURES.md), and
[coexistence](../driver/coex/FEATURES.md).

## RF and analog primitives

See catalog section `phy-rf-and-analog-primitives`.

## RX and TX calibration primitives

See catalog section `phy-rx-and-tx-calibration-primitives`.

## Calibration state and tracking

See catalog section `phy-calibration-state-and-tracking`.

## Protocol consumer composition

See catalog section `phy-protocol-consumer-composition`. This compatibility
heading preserves existing inbound links; the generated view contains every
Wi-Fi, Bluetooth, and IEEE 802.15.4 matrix cell separately.

## Lifecycle boundaries

See catalog section `phy-lifecycle-boundaries`. This compatibility heading
preserves existing inbound links.

## Qualification boundary

There is no standalone PHY qualification target. Protocol programs select their
own requirements; the static PHY inventory does not promote any proof state.
