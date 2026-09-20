# HIL protocol

Typed commands and evidence shared by the host runner and target firmware.
The wire version is defined only by [`PROTOCOL_VERSION`](src/message.rs).
Version compatibility, image capabilities, current-state admission and product
qualification are separate contracts.

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.
- [Wi-Fi](wifi.md): role ownership, traffic sessions and retained results.
- [Bluetooth](bluetooth.md): peripheral, encrypted ACL and secure GATT lifecycle.
- [PHY](phy.md): maintenance, fault injection and nested timing evidence.
- [Platform and memory diagnostics](diagnostics.md): watchdog and stack/copy measurements.

Source types define the accepted wire layout; these references explain its
meaning and limits. Host scenario execution belongs to the
[runner](../host/README.md); firmware setup belongs to the
[ESP32-S31 target](../targets/esp32s31/README.md).
