# ESP32-S31 radio subsystem source capabilities

This page is the stable entry point for shared radio ownership, cold power and
clock setup, protocol handoff, powered-idle transitions, shutdown and
cross-protocol arbitration. The canonical source declarations and their exact
limitations live in the
[whole-radio catalog](../../../../qualification/catalog/esp32s31/whole-radio.toml).
The protocol and PHY inventories remain independently owned and are linked
below rather than duplicated here.

Render the static whole-radio view from the repository root with its two source
fact owners explicitly loaded:

```console
cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/whole-radio.toml \
  --out target/qualification/catalog/whole-radio-static
```

The static command does not evaluate readiness or load vendor evidence and HIL
runs. There is no standalone whole-radio qualification target. Exact bounded
facts may be projected into this view, but they do not promote broader
multi-client ownership, protocol lifetime or concurrent arbitration claims.

## Coverage index

Canonical section: `whole-radio-coverage-index`.

- [Shared PHY](../phy/FEATURES.md)
- [Coexistence](coex/FEATURES.md)
- [Wi-Fi](ieee80211/FEATURES.md)
- [Bluetooth](bluetooth/FEATURES.md)
- [IEEE 802.15.4](ieee802154/FEATURES.md)
- [Qualification programs](../../../../qualification/README.md)

## Exclusive ownership and client handoff

Canonical section: `whole-radio-exclusive-ownership-and-client-handoff`.
Exclusive root ownership and inactive route reselection do not establish safe
concurrent sharing or an active protocol switch.

## Cold power and clocks

Canonical section: `whole-radio-cold-power-and-clocks`.

## Active operation, power saving and shutdown

Canonical section: `whole-radio-active-operation-power-saving-and-shutdown`.
Protocol power-save policy, PHY maintenance admission and physical RF/clock
shutdown remain distinct boundaries.

## Concurrent ownership and arbitration

Canonical section: `whole-radio-concurrent-ownership-and-arbitration`.
Standalone protocol paths do not combine into a qualified joint-radio runtime.

## Evidence and implementation ownership

Canonical section: `whole-radio-evidence-and-implementation-ownership`.
The existing Wi-Fi STA, Bluetooth LE and IEEE 802.15.4 programs remain the sole
readiness authorities for their exact scopes.
