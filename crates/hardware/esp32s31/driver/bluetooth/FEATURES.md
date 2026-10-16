# ESP32-S31 Bluetooth source capabilities

The canonical ESP32-S31 Bluetooth source inventory is
[`qualification/catalog/esp32s31/bluetooth.toml`](../../../../../qualification/catalog/esp32s31/bluetooth.toml).
It contains the complete LE and Classic source rows, Host-only scopes,
external source references, ownership contract and peripheral timing limits. Source status does not imply RF delivery,
interoperability or qualification readiness.

Render from the repository root. The catalog imports its shared-PHY and
coexistence source facts:

```console
cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/bluetooth.toml \
  --out target/qualification/catalog/bluetooth-static
```

The generated `domain-inventory.md` and `capability-catalog.md` are ignored
views. No Bluetooth qualification program exists since #770 (tag
`legacy-radio-2026-10-16`). Classic BR/EDR remains source inventory.

See the [whole-radio capability map](../FEATURES.md) for shared ownership and
the [PHY consumer boundary](../../phy/FEATURES.md#protocol-consumer-composition)
for cross-protocol composition limits.

## Product catalog

The [product catalog](../../../../../qualification/catalog/esp32s31/bluetooth-products.toml)
keeps the narrower peripheral/ACL criteria; no program selects them and no
production Controller composition exists since #770.

## Legacy advertising and scanning

The canonical sections `bluetooth-legacy-advertising-scanning` and
`bluetooth-le-roles-connections-reliability` record the advertising, scanning
and peripheral connection roles and their current limits. The Controller core
and the S31 radio role that ran them were removed with the Bluetooth LE
orchestration (#770, tag `legacy-radio-2026-10-16`).

## Capability advertisement

The canonical `bluetooth-capability-advertisement` section records the exact
optional LE feature bits returned by the portable bootstrap and the rule that
each bit requires a complete production Controller operation.

## Ownership

The canonical `bluetooth-ownership` section records the PAC, HAL, Controller
memory, engine and portable HCI and Link Layer boundaries.

## Peripheral timing limits

The canonical `bluetooth-peripheral-timing-limits` section records the clock
accuracy and captured-anchor requirements that a peripheral composition must
satisfy.
