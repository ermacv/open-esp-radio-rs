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
The complete LE program above retains its wider requirements. Current source
subsets and existing plaintext ACL workloads do not qualify either product.

## Qualification scope mapping

The canonical catalog section `bluetooth-qualification-scope-mapping` retains
every LE mapping and the explicit projection of the shared
`bluetooth-initial-phy-handoff` fact. Broader Controller scopes remain
independent of that implemented lower handoff.

## Legacy advertising and scanning

The canonical sections `bluetooth-legacy-advertising-scanning` and
`bluetooth-le-roles-connections-reliability` retain the single-channel
connectable boundary, passive-scanning limits and their distinction from
extended or connected PHY support.

## Capability advertisement

The canonical `bluetooth-capability-advertisement` section records the exact
optional LE feature bits exposed by production. Catalog migration does not
change the bitmap or infer support from DTM-only PHY paths.

## Ownership

The canonical `bluetooth-ownership` section records the PAC, HAL, Controller
memory, portable Link Layer, chip-role, Embassy runtime and integration
boundaries. Shared scheduling machinery does not compose a missing role or
transfer protocol policy between roles.

## Peripheral timing limits

The canonical `bluetooth-peripheral-timing-limits` section retains completion,
backpressure, cancellation, credit, handle-generation, supervision and clock
accuracy requirements. A compiled lifecycle remains distinct from successful
over-air behavior and current hardware evidence.
