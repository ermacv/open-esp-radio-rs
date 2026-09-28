# HIL protocol

Typed commands and evidence shared by the host runner and target firmware.
The wire version is defined only by [`PROTOCOL_VERSION`](src/message.rs).
Version compatibility, image capabilities, current-state admission and product
qualification are separate contracts.

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.
- [Wi-Fi](wifi.md): role ownership, traffic sessions and retained results.
- [Bluetooth](bluetooth.md): GATT and secure GATT lifecycle, Direct Test Mode and raw HCI.
- [PHY](phy.md): fault injection, placement and delivery continuity.
- [Platform and memory diagnostics](diagnostics.md): watchdog, event trace, program-counter profile and stack/copy measurements.

Source types define the accepted wire layout; these references explain its
meaning and limits. Host scenario execution belongs to the
[runner](../host/README.md); firmware setup belongs to the
[ESP32-S31 target](../targets/esp32s31/README.md).

## Radio families

The crate compiles its radio families behind features of the observer
registry's family names: `wifi` (traffic sessions, roles, monitor and scan,
their evidence, UDP probes and stream patterns), `bluetooth` (Direct Test
Mode, HCI, GATT and secure GATT), `ieee802154` (probes, air check, peer
sessions and Thread) and `system` (the memory copy benchmark). The shared
core is always compiled: framing, the envelope and its `Command`, `Event`
and `EvidenceRecord` sets, capabilities, boot and post-mortem evidence, the
event trace, the program-counter profile, PHY diagnostics and startup artifacts. The host enables every
family, the default; an image enables only the families it serves, so its
build, and the evidence bound to its sources, reads only their files.

A family that is off keeps its variants in the three enums, in the same
order, because the wire encodes a variant by its position: their payload
types become the uninhabited `Absent`, so such a variant can be neither built
nor decoded and the encoding of every other variant is unchanged. An intact
frame of this protocol version whose body the build cannot decode is a
command of such a family: the decoder reports `UndecodableBody` with the
request's identity, and the image answers `Rejected(Unsupported)` at once.

A family's payload changes stay in its own modules. A new variant still
edits `message.rs`, which every image compiles, so batch new variants rather
than adding them one change at a time.

