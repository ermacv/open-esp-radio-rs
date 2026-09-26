# ESP32-S31 Bluetooth source capabilities

The canonical ESP32-S31 Bluetooth source inventory is
[`qualification/catalog/esp32s31/bluetooth.toml`](../../../../../qualification/catalog/esp32s31/bluetooth.toml).
It contains the complete LE and Classic source rows, Host-only scopes,
qualification mappings, external source references, ownership contract and
peripheral timing limits. Source status does not imply RF delivery,
interoperability or qualification readiness.

Render from the repository root. The catalog imports its shared-PHY and
coexistence source facts:

```console
cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/bluetooth.toml \
  --out target/qualification/catalog/bluetooth-static
```

The generated `domain-inventory.md`, `capability-catalog.md` and
`migration-map.md` are ignored views. The
[Bluetooth LE qualification program](../../../../../qualification/targets/esp32s31/bluetooth-le.toml)
selects the same declarations through the sole qualification evaluator. Classic
BR/EDR remains source inventory outside that LE gate.

See the [whole-radio capability map](../FEATURES.md) for shared ownership and
the [PHY consumer boundary](../../phy/FEATURES.md#protocol-consumer-composition)
for cross-protocol composition limits.

## Product programs

[Peripheral/ACL](../../../../../qualification/targets/esp32s31/bluetooth-peripheral-acl.toml)
and [secure peripheral GATT](../../../../../qualification/targets/esp32s31/bluetooth-secure-gatt.toml)
select the narrower product criteria in the
[product catalog](../../../../../qualification/catalog/esp32s31/bluetooth-products.toml).
The secure program includes the peripheral lifecycle through dependencies.
The complete LE program above retains its wider requirements. No LE Controller
composition currently exists, so neither product has a source implementation.

## Qualification scope mapping

The canonical catalog section `bluetooth-qualification-scope-mapping` retains
every LE mapping and the explicit projection of the shared
`bluetooth-initial-phy-handoff` fact. Broader Controller scopes remain
independent of that implemented lower handoff.

## Legacy advertising and scanning

The canonical sections `bluetooth-legacy-advertising-scanning` and
`bluetooth-le-roles-connections-reliability` record the portable advertising,
scanning and connection owners and the absence of a Controller composition.

## Capability advertisement

The canonical `bluetooth-capability-advertisement` section records the exact
optional LE feature bits returned by the portable bootstrap and the rule that
each bit requires a complete production Controller operation.

## Ownership

The canonical `bluetooth-ownership` section records the PAC, HAL, Controller
memory, engine and portable HCI/Link Layer boundaries. Shared scheduling
machinery does not compose a role.

## Peripheral timing limits

The canonical `bluetooth-peripheral-timing-limits` section records the clock
accuracy and captured-anchor requirements that a peripheral composition must
satisfy.
