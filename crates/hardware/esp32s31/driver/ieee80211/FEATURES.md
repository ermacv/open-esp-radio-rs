# ESP32-S31 Wi-Fi capability entry point

The canonical source inventory for the ESP32-S31 Wi-Fi driver now lives in
[the Wi-Fi/PHY catalog](../../../../../qualification/catalog/esp32s31/wifi-phy.toml).
It records source status, composition level, exact scope and limitations, and
package and document links independently of qualification readiness.

Generate the complete static inventory without vendor evidence or HIL runs:

```console
cargo qualification catalog render --catalog qualification/catalog/esp32s31/wifi-phy.toml --out target/qualification/catalog/wifi-phy-static
```

Generate the resolved STA readiness view, using the sole qualification
evaluator and its configured evidence inputs:

```console
cargo qualification catalog render --manifest qualification/targets/esp32s31/wifi-sta.toml --out target/qualification/catalog/wifi-sta
```

The ignored output separates `domain-inventory.md`,
`capability-catalog.md`, `program-inventory.md`, and
`migration-map.md`. The catalog is the tracked source; generated Markdown is
never a readiness authority.

Architecture navigation: [whole-radio map](../FEATURES.md),
[STA composition](../../../../roles/esp32s31/ieee80211/sta/README.md), [AP engine](../../../../roles/esp32s31/ieee80211/ap/src/engine.rs),
[MAC TX](mac/src/tx.rs), [MAC RX](mac/src/rx.rs), and
[shared PHY](../../phy/FEATURES.md).

## Qualification scope

The [STA program](../../../../../qualification/targets/esp32s31/wifi-sta.toml)
resolves its direct catalog roots and dependencies to exactly the same 14
required capability IDs. AP, STA+AP,
monitor, ESP-NOW, unsupported protocol surfaces, and other source-facet entries
remain visible in the full domain inventory without receiving readiness
booleans.

## Interfaces and operating modes

See catalog section `wifi-interfaces-and-operating-modes`.

## Legacy and HT MAC behavior

See catalog section `wifi-legacy-and-ht-mac-behavior`.

## Security

See catalog section `wifi-security`.

## TSF, beacon monitoring, and power saving

See catalog section `wifi-tsf-beacon-monitoring-and-power-saving`.

## 802.11ax / HE

See catalog section `wifi-802-11ax-he`.

## Frequency, TX power, antenna, FTM, and ESP-NOW

See catalog section `wifi-frequency-tx-power-antenna-ftm-and-esp-now`.

## Physical publication preconditions

The exact fail-closed publication boundary and its owner links are preserved in
catalog section `wifi-physical-publication-preconditions`.
