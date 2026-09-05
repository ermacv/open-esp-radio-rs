# ESP32-S31 Bluetooth LE implementation frontier

This is the production-code frontier, not an RF or Bluetooth qualification
claim. Qualification requirements remain in
[bluetooth-le.toml](../../../../qualification/targets/esp32s31/bluetooth-le.toml).
A compiling descriptor or isolated state machine does not establish a live
controller capability.

## Supported direction

The first end-to-end target is one always-awake LE 1M peripheral connection,
legacy advertising, bounded ACL buffers and a Trouble GATT server through
the standard `bt-hci` interface. The current dependency pair is `bt-hci`
0.10.1 and `trouble-host` 0.8.0. Trouble is currently a development dependency
used to test bootstrap, not a production GATT composition.

BR/EDR, privacy, encryption, extended advertising, other PHYs, ISO, sleep and
concurrent Wi-Fi/Bluetooth are separate milestones. Advertised HCI capabilities
must remain limited to implemented behavior.

## Current implementation

| Layer or path | Implemented | Remaining boundary |
| --- | --- | --- |
| PAC/HAL | Typed clock/reset, controller time, IRQ, scheduler and memory-list access; publication barriers | Physical validation, long-running timer/PHY maintenance and powered teardown |
| Controller SRAM | DTM, advertising, scanning and peripheral graphs; private codecs; CPU/hardware ownership and bounded RX extraction | Connected TX queue, acknowledgment/retry ownership and verified packet-engine semantics |
| DTM | RX/TX preparation, recurring events, completion/recycle, Test End and Reset with Embassy composition | Recorded RF/HIL evidence and complete long-running maintenance |
| Advertising/scanning | Portable policy, typed HCI commands and chip event paths | Target actor regression coverage and on-air validation |
| Connectable advertising | Configuration, generation/event identity, first event, no-connection recurrence, ordered stop and CONNECT_IND handoff | Hardware evidence for recurrence/stop; do not infer RF delivery from scheduler RUN |
| Peripheral connection | Causal first window, first RUN, common completion engine, RX/recycle and lower recurrence preparation/publication | The active actor does not yet drive connection completion/recurrence; missed-event recovery and reliability remain incomplete |
| Portable connection LL | CONNECT_IND validation, channel selection, event progression and anchor bookkeeping | SN/NESN responsibility, retransmission, duplicate suppression, establishment/supervision timeout and LLCP |
| HCI | Standard packet types, bounded in-process transport, bootstrap, DTM, advertising and scanning | Connection events, handles, ACL routing and credits; non-command input is currently quarantined |
| Trouble | Real Host runner bootstrap test | Production runner, connected ACL and GATT interoperability |
| Shutdown | Typed pre-publication rollback and selected HCI Reset paths | Complete powered quiescence, PHY release and cold reconstruction |

The peripheral active actor currently retains the first running owner and
services a pending HCI response, but does not call its radio-completion methods.
Its repeated `PeripheralConnectionActive` observation is not evidence of
successive connection events.

Peripheral recurrence additionally requires a caller-owned local clock
accuracy bound. Its current software-widening profile does not solve arbitrary
missed-event anchor recovery or accumulated timing uncertainty.

## Ownership and reuse

- PAC owns MMIO encoding. HAL publishes semantic accessors and ownership proofs.
- The memory crate owns SRAM layouts, private codecs and DMA-visible storage.
  Chip, LL and executor code must not duplicate their bit operations.
- Portable LL owns protocol policy and advertising generation/event identity.
- The chip implements scheduler timing, admission, publication and role recycle.
- Embassy owns waits, fairness and durable task storage; integration owns board
  resources and interrupt routes.
- `single_item_completion` is shared by advertising, scanning and peripheral
  roles. Role-specific RX/recycle policy remains separate.
- `controller_start/timed_preparation` shares time-request, recheck, rollback and
  orphan-drain behavior. New roles should use these engines where semantics match.

The actor and HCI wrappers still expose too many internal preparation phases.
Reduce this surface around one session interface per role. Measure final actor,
future and stack memory before replacing ordinary outcomes with callback
frameworks solely to satisfy size lints.

## Ordered closure plan

1. Restore target compilation and behavioral tests after the interrupted
   refactors; record a reproducible commit before adding another feature.
2. Verify advertising RUN, completion, recurrence, Disable/Reset and re-enable
   through the production actor, including HCI response backpressure.
3. Drive peripheral RUN through completion/recycle into the next event.
   Validate explicit clock accuracy, anchor acquisition and missed-event behavior.
4. Resolve packet-engine ACK/retry semantics and implement bounded connected
   TX/RX ownership, duplicate handling, supervision and disconnect cleanup.
5. Implement the LL control procedures required by the advertised feature set.
6. Connect HCI events, connection handles and ACL credit accounting to the real
   dataplane; add a production Trouble peripheral/GATT example.
7. Validate repeated connections, faults, periodic PHY/timer work and shutdown.

Each milestone requires a concrete observable result, focused production-path
tests and a commit. A single board can verify scheduler progression and
quiescent resource recovery. On-air advertising and connection/data exchange
require a BLE observer or peer. Neither a successful HCI command nor first RUN
alone proves RF delivery.

## Evidence corrections still required

The peripheral research note historically treated link-state event-span storage
and scheduler captured-anchor storage as one location. The current memory codec
uses distinct objects. The disagreement must be resolved against source evidence
before qualifying anchor normalization; see the explicit review note in
[bluetooth-peripheral-connection.md](../../../../verification/vendor/projects/esp32s31/analysis/bluetooth-peripheral-connection.md).
