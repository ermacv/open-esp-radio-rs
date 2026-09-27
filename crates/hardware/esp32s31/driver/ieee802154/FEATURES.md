# ESP32-S31 IEEE 802.15.4 source capabilities

This page is the stable entry point for the ESP32-S31 IEEE 802.15.4 radio and
MAC source inventory. The canonical declarations, exact limitations, source
contracts, publication boundaries and ownership references live in the
[IEEE 802.15.4 catalog](../../../../../qualification/catalog/esp32s31/ieee802154.toml).
They do not claim a complete IEEE 802.15.4-2015 stack, calibrated RF operation
or on-air qualification.

Render the static inventory from the repository root:

```console
cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/ieee802154.toml \
  --out target/qualification/catalog/ieee802154-static
```

The static command does not load vendor evidence or HIL runs. Use manifest
render or the evaluator separately when a readiness view from the current
evidence context is required. The existing six-capability program remains the
sole readiness authority for this radio/MAC scope.

See the [whole-radio entry page](../FEATURES.md) for shared PHY ownership,
power lifetime and coexistence limits.

## Qualification scope mapping

Canonical section: `ieee802154-qualification-scope-mapping`.

## Capability sources and publication scope

Canonical section: `ieee802154-capability-sources-and-publication-scope`.
Portable `RadioCapabilities` is a vocabulary; the ESP32-S31 radio role
advertises the subset the engine implements as source coverage. Vendor API
declarations are inventory references rather than open-driver evidence.

## PHY and RF

Canonical section: `ieee802154-phy-and-rf`.

## CCA and channel access

Canonical section: `ieee802154-cca-and-channel-access`.

## RX/TX dataplane and acknowledgments

Canonical section: `ieee802154-rx-tx-dataplane-and-acknowledgments`.
The MAC engine is ported from the public ESP-IDF driver; the host stand
compares its software sequence with the compiled vendor driver.

## Filtering, addressing and MAC automation

Canonical section: `ieee802154-filtering-addressing-and-mac-automation`.

## Timing

Canonical section: `ieee802154-timing`.

## Security, power and coexistence

Canonical section: `ieee802154-security-power-and-coexistence`.

## Product stacks

Canonical section: `ieee802154-product-stacks`. Thread, Zigbee and Matter are
HOST-ONLY scopes and are not capabilities admitted by the radio/MAC gate.

## Ownership and readiness

Canonical section: `ieee802154-ownership-and-readiness`. The HAL, PAC, MAC
engine and runtime do not by themselves compose the missing RF-ready public
service.
