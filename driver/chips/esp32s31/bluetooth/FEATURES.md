# ESP32-S31 Bluetooth LE source capabilities

This matrix describes implemented code and its composition limits. It does
not claim RF delivery, interoperable connectivity or Bluetooth qualification.
The [qualification specification](../../../../qualification/targets/esp32s31/bluetooth-le.toml)
owns readiness requirements and hardware evidence. A successful HCI command,
scheduler `RUN`, descriptor or isolated state machine does not establish an
end-to-end controller capability.

## Implementation and composition

| Layer or path | Implemented source contract | Unsupported or unqualified boundary |
| --- | --- | --- |
| PAC/HAL | Typed clock/reset, controller time, IRQ, scheduler and memory-list access; publication barriers | Physical validation, complete long-running timer/PHY maintenance and powered teardown are not established by these accessors |
| Controller SRAM | DTM, advertising, scanning and peripheral graphs; private codecs; CPU/hardware ownership and bounded RX extraction | No complete connected TX queue, acknowledgment/retry owner or verified packet-engine contract |
| DTM | RX/TX preparation, recurring events, completion/recycle, Test End and Reset with Embassy composition | These source paths do not establish RF/HIL qualification or complete long-running maintenance |
| Advertising/scanning | Portable policy, typed HCI commands and chip event paths | Source composition does not establish on-air delivery |
| Connectable advertising | Configuration, generation/event identity, first event, no-connection recurrence, ordered stop and CONNECT_IND handoff | Scheduler `RUN` does not establish successful advertisement or connection exchange |
| Peripheral connection | Causal first window, first `RUN`, lower completion/RX/recycle and recurrence preparation/publication | The active actor retains the first running owner but does not drive its radio completion/recurrence; missed-event recovery and reliability are incomplete |
| Portable connection LL | CONNECT_IND validation, channel selection, event progression and anchor bookkeeping | No complete SN/NESN, retransmission, duplicate suppression, establishment/supervision timeout or LLCP owner |
| HCI | Standard packet types, bounded in-process transport, bootstrap, DTM, advertising and scanning | Connection events, handles, ACL routing and credits are not connected; non-command input is quarantined |
| Trouble Host | Host bootstrap test through `bt-hci` | No production Trouble runner, connected ACL or GATT interoperability composition |
| Shutdown | Typed rollback before publication and selected HCI Reset paths | Complete powered quiescence, PHY release and cold reconstruction are not implemented |

The controller uses `bt-hci` 0.10.1. `trouble-host` 0.8.0 is a development
dependency of the portable HCI crate, not the product GATT server. BR/EDR,
privacy, encryption, extended advertising, other PHYs, ISO and sleep are outside
this implementation scope. The platform adapters do not permit safe concurrent
Wi-Fi/Bluetooth singleton ownership. HCI capability advertisement must remain
limited to the composed implementation.

## Ownership

| Owner | Authority |
| --- | --- |
| PAC | MMIO representation and restricted access |
| HAL | Semantic accessors, register transactions and hardware ownership proofs |
| [Memory crate](memory/) | Controller-SRAM layouts, private codecs and DMA-visible storage |
| Portable LE LL | Protocol policy and advertising generation/event identity |
| Chip roles | Scheduler timing, admission, publication and role-specific RX/recycle |
| Embassy runtime | Waits, command/response fairness and durable task state |
| Integration | Static resources, platform claims and interrupt routes |

`single_item_completion` is the shared lower completion engine for advertising,
scanning and peripheral roles. `controller_start/timed_preparation` owns shared
time requests, rechecks, rollback and orphan draining. Sharing those mechanisms
does not compose a missing caller or transfer protocol policy between roles.

## Peripheral timing limits

The [active controller branch](../../../runtime/embassy/esp32s31/bluetooth/src/controller/dispatch.rs)
services a pending HCI response and returns `PeripheralConnectionActive`.
Repeated observations of that boundary do not represent successive radio
events.

Lower recurrence requires a caller-owned local clock accuracy bound. Its
software window widening does not establish arbitrary missed-event anchor
recovery or accumulated timing uncertainty. The
[memory codec](memory/src/peripheral_connection_memory/codec.rs) stores
link-state event span and scheduler captured anchor in distinct objects;
equal offsets within those objects do not make them the same field. A raw
captured anchor is not a normalized packet-start timestamp. Hardware timing
qualification must cover that interpretation independently of the ownership
and layout types.
