# ESP32-S31 IEEE 802.15.4 source capabilities

This page is the stable entry point for the ESP32-S31 IEEE 802.15.4 radio and
MAC source inventory. It does not claim a complete IEEE 802.15.4-2015 stack or
calibrated RF operation.

See the [whole-radio entry page](../FEATURES.md) for shared PHY ownership,
power lifetime and coexistence limits.

## Capability sources and publication scope

Portable `RadioCapabilities` is a vocabulary; the engine's `RADIO_CAPABILITIES`
declares what the engine itself implements as source coverage. It omits
`CSMA_CA` and `TRANSMIT_RETRIES`: the engine transmits each frame once, and
the backoffs and retries the removed ESP32-S31 radio role ran (tag
`legacy-radio-2026-10-16`) belong above the port (#563 decision 8). Vendor API
declarations are inventory references rather than open-driver evidence.

## RX/TX dataplane and acknowledgments

The MAC engine is ported from the public ESP-IDF driver and is chip-neutral
([`oer-espressif-ieee802154-engine`](../../../espressif/ieee802154/engine/src/lib.rs)); the
ESP32-S31 HAL implements its low-level interface. The host stand compares
its software sequence with the compiled vendor driver.

## Product stacks

Thread, Zigbee and Matter are host-only scopes.

## Ownership

The HAL, PAC and MAC engine are not a service: the former role, runtime and
composition that joined them to the shared radio were deleted (#770, tag
`legacy-radio-2026-10-16`); the engine records their
[bring-up order](../../../espressif/ieee802154/engine/README.md#bring-up-and-teardown-order).
