# HIL protocol

Typed messages shared by the host runner and target firmware. Every message
is its own type in one module, identified on the wire by a key: a hash of the
message's path and of its postcard schema. [`messages.lock`](messages.lock)
names the framing version, each module's dependencies (`[modules]`) and every
message's path, key and kind (`[messages]`); its test keeps it equal to the
source. Image capabilities, current-state admission and
product qualification are separate contracts.

- [Wire contract](wire.md): framing, version checks, discovery and boot identity.
- [Wi-Fi](wifi.md): role ownership, traffic sessions and retained results.
- [Bluetooth](bluetooth.md): GATT and secure GATT lifecycle, Direct Test Mode and raw HCI.
- [PHY](phy.md): fault injection, placement and delivery continuity.
- [Platform and memory diagnostics](diagnostics.md): watchdog, event trace, program-counter profile and stack/copy measurements.

Source types define the accepted wire layout; these references explain its
meaning and limits. Host scenario execution belongs to the
[runner](../host/README.md); firmware setup belongs to the
[ESP32-S31 target](../targets/esp32s31/README.md).

## Modules and messages

A module is a path prefix and a directory: `base` (hello, capabilities, boot
status, post-mortem, link health, acceptance and refusal), `system`, `wifi`,
`network`, `bluetooth`, `ieee802154`, `phy` and `telemetry`. A module declares
its messages with `messages!`:

- an endpoint is a request the device serves and names its one response;
- a topic is a device message: a response or an unsolicited event;
- a property is a marker an image advertises in its capabilities and never
  sends.

A response may follow other messages about the same request: a replayed
session publishes its evidence before `network::Finished`, and an accepted
Wi-Fi operation publishes its completion later, with the request's identifier.
A refusal is `base::Rejected` with its reason.

The crate root is the framework: `key.rs` (message identity and
`messages!`), `framing.rs`, `io.rs` and `envelope.rs`. It names no module.
Everything else lives in a module's directory, `src/<module>/`: its
messages in `mod.rs` and every payload type they carry beside them, reached
as `oer_hil_protocol::<module>::Type`. A module may use another module's
payloads through `crate::<module>`; the lock's `[modules]` section lists each
module with the modules it uses, the registry test derives that list from the
sources and rejects a cycle. A runner that speaks a module needs the
framework, `base`, that module and what it uses.

A message's meaning is its path and its type. A change of meaning without a
change of type takes a new path; units live in the types. A change of a
message's schema changes its key, and the lock records the change for review.

## Radio families

The crate compiles its radio families behind features of the observer
registry's family names: `wifi` (the `wifi` and `network` modules' messages,
traffic sessions, roles, monitor and scan, their evidence, UDP probes and
stream patterns), `bluetooth` (Direct Test Mode, HCI, GATT and secure GATT),
`ieee802154` (probes, air check, peer sessions and Thread) and `system` (the
memory copy benchmark). The shared core is always compiled: framing, the
envelope, keys and capabilities, the `base` module, boot and post-mortem
evidence, the event trace, the program-counter profile, PHY diagnostics and
startup artifacts. An image enables only the families it serves, so its
build, and the evidence bound to its sources, reads only their files; a
family that is off has no messages in the build. The host decodes every
message: the `registry` feature enables every family and lists each
message's path, key, kind, schema and readable decoder.
