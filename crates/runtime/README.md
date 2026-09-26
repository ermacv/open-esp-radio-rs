# Radio execution

`esp32s31/{ieee80211,ieee802154,bluetooth}` (`oer-esp32s31-ieee80211-runtime`,
`oer-esp32s31-ieee802154-runtime`, `oer-esp32s31-bluetooth-runtime`) contains
concrete radio execution as executor-independent `async` code; `ieee80211` (`oer-ieee80211-runtime`) holds the
chip-independent Wi-Fi execution primitives they share, and `bluetooth`
(`oer-bluetooth-runtime`) the service loop that joins the in-process HCI
transport, the portable LE Controller core and any radio port. Directory boundaries describe the execution
responsibility of each package.

## Execution and time contract

A runtime exposes futures and never spawns, names or requires an executor; any
executor that polls them is valid, and the architecture check rejects executor
dependencies below adapters and compositions. The portable primitives are
`embassy-sync` (bounded mailboxes and signals over a caller-chosen raw mutex)
and `embassy-futures` (select/join). Time is the `embassy-time` interface:
`Instant` and `Timer` read and wait on one global monotonic timebase supplied
through `embassy-time-driver`. The final image links exactly one driver — the
ESP32-S31 [platform timer queue](../adapters/embassy/esp32s31/executor/) on the
chip, the `std` driver in host tests. A runtime never installs a driver and does
not assume which executor wakes its timers.

| Module | Responsibility |
| --- | --- |
| `ieee80211/src/{monitor,connected_tasks,station_network,stack_boundary}` | Portable capture/injection handoffs, task shutdown, association-scoped network ownership and explicit polling boundary |
| `esp32s31/ieee80211/src/roles/` | Role execution and retained TX/RX owners |
| `esp32s31/ieee80211/src/roles/access_point/network_tx.rs` | One AP TX owner, publication and cancellation |
| `esp32s31/ieee80211/src/roles/access_point/network_tx/{queue,power_save,aggregate,completion}.rs` | Lease queues, TIM/DTIM release, standby aggregation and completion on that same owner |
| `esp32s31/ieee80211/src/roles/station/maintenance/` | Connected-station PHY maintenance request protocol, automatic tracking control, pause timeline and terminal-failure classification; the composition owns the static request owner, the physical round trip and system reset |
| `esp32s31/ieee80211/src/roles/esp_now/mailbox/` | Bounded ESP-NOW application RX/TX mailboxes shared by the connected station and the standalone role |
| `esp32s31/ieee80211/src/datapath/` | Packet handoff and async composition around chip transactions; depends on no role module, and `datapath/network` owns the STA/AP network interface identities |
| `esp32s31/ieee802154/src/` | The IEEE 802.15.4 radio role and its MAC owners under one blocking mutex, portable command admission, the interrupt entry and a bounded queue of owned portable events; overflow reports the loss |
| `bluetooth/src/` | The Controller service loop: publishes queued packets, takes one command when the core is ready and Host ACL data while the connection has room for it, submits radio requests one at a time, feeds outcomes back and retries refused requests after a short delay; `NoRadio` serves hosts without a radio |
| `esp32s31/bluetooth/src/` | The Bluetooth LE radio role and its hardware under one async lock: receive-chain publication at install, request admission against a fresh controller-time sample, the scheduler driver that reports finished events and carries list transactions through their hardware waits, stop and resume around shared-PHY maintenance, a bounded queue of owned outcomes whose overflow reports the loss, the source-127 modem-timer task driver and the radio port of the Controller service loop |
| `esp32s31/ieee80211/src/datapath/owned.rs` | The only `owned-network` code: owned-adapter RX/link bindings, single and dual owned networks and the pinned-SRAM `DatapathTxConsumer` |
| `esp32s31/ieee80211/src/diagnostics/` | Optional execution observation |
| `esp32s31/phy/` | The one `embassy-time` implementation of the PHY delay, tracking clock and tracking timer used by every radio composition |

Hardware transactions and finite chip state remain below these packages. A
runtime retains their affine owners across borrowed waits, returns the same
resources on rejection and preserves terminal owners when quiescence is not
proven. A composed owner alone does not establish hardware qualification.

The PHY time leaves are adapters inside the execution packages. The Wi-Fi
binding supplies a direct `embassy-time` delay; the checked `EmbassyPhyTime`
binding also validates the timebase and rejects deadline overflow. The two
bindings have distinct time contracts. The platform
executor/time ABI remains in [adapters](../adapters/embassy/README.md).

The [integration layer](../composition/esp32s31/embassy/) chooses memory budgets,
claims static resources and owns protocol lifecycle composition within the
[documented capability boundaries](../hardware/esp32s31/driver/FEATURES.md).
Wi-Fi's supervisor owns shared physical resources and transitions between roles.
No product composition depends on a second runtime owner hidden in a task or
network handle.

Portable protocol policy cannot depend on this execution domain. The
architecture audit follows transitive normal/build dependencies to enforce
that boundary; the generic radio service and its Embassy adapter cannot depend
on these concrete ESP32-S31 runtimes.

## Telemetry features

Core0 profiling (`tx-phase-telemetry`, `core0-rx-coarse-telemetry`,
`task-poll-telemetry`) stays out of production code without changing it. The
profiling modules under `esp32s31/ieee80211/src/diagnostics/` and
`datapath/tx_performance.rs` are always compiled: without their feature, a
sample reads no CSR and is zero-sized, recorders return immediately, and
owner fields that exist only for telemetry use zero-sized holders
(`Core0PreparedTxMark`, `Core0Tally`, `Core0PathCount`). `diagnostics::profile`
selects the RX runner and DMA profiles. Each task-poll module has a zero-sized
stand-in under `diagnostics/disabled/` that exposes only the API the datapath
calls. Telemetry whose arguments need computation is guarded by a constant
such as `TX_PHASE_TELEMETRY` rather than `cfg`.

A call-site `cfg` remains where removing it would change the production
machine code: samples that cross an `.await`, `&mut` profile parameters and
the hottest scheduler and RX dispatch loops. Diagnostic behavior switches,
such as the recycled-RX probe delay, and the `diagnostics` observation API
stay feature-gated by design. A change to this code keeps the release images'
`.text`, `.hot.text` and `.isr.text` byte-identical when built from the same
checkout path.

