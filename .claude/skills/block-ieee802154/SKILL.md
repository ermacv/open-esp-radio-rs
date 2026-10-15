---
name: block-ieee802154
description: Use when a task changes or investigates the IEEE 802.15.4 block of this repository — the portable 802.15.4 MAC and radio contracts, the IEEE 802.15.4 port (owner shape), the Espressif MAC engine ported from ESP-IDF, the radio role and runtime, the ESP32-S31 composition and interrupt route, the OpenThread adapter or Thread example, the 802.15.4 vendor host stand, or 802.15.4 and PHY-calibration HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# IEEE 802.15.4 block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[ownership and refusals](../../../docs/architecture.md#ownership-and-refusals),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## Packages (path → role)

- Protocol, portable: `crates/protocols/ieee802154` (`oer-ieee802154`: MAC frames, CSMA, CSL, enhanced ACK, security, time sync, the radio state machine and the port). No portable service package.
- Family (Espressif, S31 and C5): engine `crates/hardware/espressif/ieee802154/engine` (`oer-espressif-ieee802154-engine`, the MAC driver ported from public ESP-IDF, `Ieee802154LowLevel`, host register model under feature `model`); shared MAC values `crates/hardware/ieee802154/pac`; trace points `crates/hardware/ieee802154/trace`; role `crates/roles/espressif/ieee802154/radio`; runtime `crates/runtime/espressif/ieee802154` (`Ieee802154Runtime`).
- ESP32-S31: PAC, HAL and PHY modules inside the chip crates (`hardware/esp32s31/{pac,hal}/src/ieee802154*`, `phy/src/ieee802154_client.rs`; `Ieee802154MacOwners` implements the engine's low level); composition `crates/composition/esp32s31/embassy/ieee802154` (`oer-esp32s31-ieee802154-system`, `Ieee802154System`); esp-hal interrupt route and time `crates/adapters/esp-hal/esp32s31/ieee802154`. `crates/hardware/esp32s31/driver/ieee802154/` holds only the capability page.
- OpenThread: `crates/adapters/openthread/ieee802154` (`OpenThreadRadio` over any port); example `examples/esp32s31/thread`.
- Facade: `crates/oer` features `ieee802154`, `esp32s31-ieee802154`, `embassy-ieee802154`, `openthread`. HIL: `hil/scenarios/ieee802154/`, workloads in `hil/host/family/ieee802154`, ESP32-C5 peers in `hil/peers/esp32c5-ieee802154` and `hil/peers/esp32c5-openthread`, agent `hil/targets/esp32s31/agent/src/product_hil/ieee802154/`.

## Port

The port is the owner shape of `crates/protocols/ieee802154/src/owner.rs` over the lifecycle package `oer-radio-port` (`crates/radio/port`). Optional operations are `Ieee802154Capabilities` behind a capabilities marker (`Ieee802154Caps`); every value a call takes is validated by type. The backend is `Ieee802154RuntimeBackend` (`crates/runtime/espressif/ieee802154/src/backend.rs`), which `Ieee802154Runtime::install` returns: the composition (`start`) packages it and its independent runtime driver in `Installed` after binding the MAC route, the application runs it under `run_port`, and the port's end masks the route (`detach`) and returns the engine and MAC owners to `Ieee802154System::stop`. The engine stays in the runtime's blocking mutex shared with the MAC interrupt, an interim the owner allowed until #668. The role advertises `IEEE802154_RADIO_CAPABILITIES`.
  - interface `I` is a type parameter of its slots, its configured token and its client (owner decision on #603);
  - the one operation permit sits in the backend's held part and comes back before its terminal is handed out; a client takes it with its interface's seat (`Seat<'id, Operation>`, one per interface client from `Permits<Operation, 1, S>`, passed to `WorkPermits::new`), so the line has a place for every interface and serves them in arrival order (#581);
  - receive and sleep are per-interface wishes; the radio receives their union on one shared channel;
  - a radio fault poisons the port.

## Shared contracts (change only with their owners)

`crates/radio/port` (`ClockInfo`, `EventsLost`, lifecycle, failure classes), `crates/time` (`Ieee802154Instant` = `RadioInstant<Ieee802154Radio>`, `Clock`, `Timer`), `crates/radio/coex` and `crates/hardware/espressif/coex`, the shared radio system `crates/runtime/esp32s31/radio` (`RadioGuard::join_ieee802154`) and `crates/composition/esp32s31/embassy/radio`, the PHY concurrent domain, the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`Ieee802154RadioPartition`), PAC and `registers/`.

## Traps

- `Ieee802154RuntimeCaps::Interfaces` is a `PoolSet` layout, composed from `InterfacePool<Caps, I, P>`. The runtime combines it with the operation permit layout and consumes `Mint::into_pools(self)` once. A composition that wraps several interfaces expands one tuple through its consumed `PoolBuilder`; it cannot mint each interface separately.

- The radio clock is the runtime's monotonic clock (`ClockInfo::MONOTONIC_MICROS`), unlike Wi-Fi's affine MAC time; the engine stamps each frame at its SFD with the platform microsecond clock. The esp-hal adapter's `now_micros` needs `esp_hal::init` first.
- The engine takes every deadline decision inside its ISR (`Ieee802154Engine::isr`: ACK and pending bit, enhanced ACK, ACK watchdog, next operation), as the vendor does; deferring them to a task cannot meet the deadlines ([driver port](../../../docs/vendor/esp32s31/ieee802154-driver-port.md)).
- CSMA-CA backoffs and retry delays are software, run by the port's joined driver (`PortBackend::drive`, which `run_port` joins with the body); the events consumer only takes events. Over OpenThread, `SubMac` runs CSMA-CA and retries itself.
- One event consumer. A full queue (16 events) drops the newest received frame and reports `EventsLost` once in its place; one slot stays free for the next operation's terminal. A radio fault poisons the port (owner decision on #603): the consumer hands out the earlier events and the failed operation's terminal, then `Poisoned`.
- The owner shape has no quiesce: the owner's `Disabled`/`Enabled` state replaces the old lifecycle refusals.
- Configuration (addresses, interface enable, the pending table, promiscuous, auto-ACK, coordinator) goes through the role's `Ieee802154Radio::configure`, which applies it in any state as ESP-IDF's `esp_ieee802154_set_*` do; the portable state machine refuses `Configure` while disabled or busy, and a backend that routes the owner shape's setters through `submit` poisons the port on the disabled setup or during an operation. A backend's refusal is a fault (`StateLost`), so a backend path must never meet a refusal the types already excluded.
- The enabled period reserves the fault's queue slot at `enable`, and `disable` gives it back; the disable drains the terminals it ended without re-applying the interfaces' resting wish to the disabled radio.
- The runtime is a process singleton: modem source 132 has one handler and the MAC one set of owners, in one blocking mutex with the engine. Route order: activate the HAL interrupt owner, install the runtime, then `bind`; teardown reverses it. The handler lives in SRAM.
- Interrupt model ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals), #643): the MAC interrupt's context owns the engine, because the ACK, enhanced-ACK, pending-bit and TX SFD frame-patch decisions must land inside the 192 µs turnaround and cannot move to a task. Tasks will post commands through a lock-free queue and a FROM_CPU doorbell at the MAC's level and core (#668, after #603); the shared blocking mutex above is what #668 removes, so never add a new task-side path that takes it. A doorbell raise reaches its handler in ~2 µs, ≤ 4.74 µs observed under 60 Mbit/s Wi-Fi RX (HIL `diagnostic-interrupt-latency`).
- The MAC DMA reaches internal SRAM only (`IEEE802154_DMA_WINDOW`): a frame in PSRAM is a DMA error and never reaches the air. `start` fails closed with `Ieee802154StartError::BuffersNotDmaVisible`. A frame received into the stub buffer is dropped.
- `EVENT_STATUS` is write-one-to-clear, sampled into an affine token no caller can forge or replay; a stale image fails closed before `ED_START`. A `STOP` write is not proof of DMA quiescence ([dataplane](../../../docs/vendor/esp32s31/ieee802154-dataplane.md), [control](../../../docs/vendor/esp32s31/ieee802154-control.md)).
- Platform clock references go back to ESP-HAL only through their owner's explicit release under a lease: common power (`exit_common_power`: analog-I2C, 160 MHz, MPLL), the modem clock planner (`disable_modem_clocks`: 160 MHz, analog-I2C, coexistence) and the Bluetooth low-power clock (`deselect_bluetooth_low_power_clock`: low-power timer). A guard otherwise ends with `abandon` (kept on, fail-stop); dropping it keeps the clock on but is a defect a debug build asserts. The diagnostic probes keep their shared radio in a static for the boot: with `CLK_COEX_EN` closed under the MAC, every ED is rejected as `EdCoexistenceReject`.
- A failure before shared hardware work rolls back to the parked partition; a started PHY, clock or power transaction that fails stays fail-stop until reset.
- PHY: tracking runs with the MAC receiving, as ESP-IDF's `phy_common.c` does; the runtime never pauses for it, and an operation that loses RF ends with its ordinary terminal (`TxStatus::CoexistenceRejected` for a denied transmission; a denied reception is a silent miss). RF stays open for the client's lifetime; there is no modem retention or light sleep with the pinned libphy.
- Scheduled TX uses TIMER0 and the modem ETM, scheduled RX TIMER1; ETM channels zero and one belong to IEEE 802.15.4 (BLE routes channel two).
- Coexistence: `start` reads the arbiter's table once; a changed table reaches the MAC only through `update_coexistence`. `stop` refuses while Wi-Fi coexistence takes part. External coexistence does nothing on the pinned libcoexist. `recent_rssi` reads the shared BTBB byte of the last reception, whichever protocol received it.
- Security: a retransmission keeps its frame counter and key index; only a CSL receiver gets a new counter per retry. OpenThread reads the capabilities once when its instance is built.
- The 802.15.4 HIL images are built on the Wi-Fi runtime: a Wi-Fi source change stales their evidence.
- No readiness claim beyond the catalog: Thread, Zigbee and Matter are host-only, and CSMA/CA and CSL are outside the qualified capabilities.

## Checks

- `cargo test -p <package>` (the engine with `--features model`), then `cargo xtask check changed`; the vendor host stand is its own workspace: `cargo test --manifest-path verification/esp32s31/host/ieee802154/Cargo.toml`.
- `cargo hil images check --class diagnostic-ieee802154-radio --type-check` for chip-side changes (also `diagnostic-ieee802154-radio-trace`, `diagnostic-ieee802154-thread`).
- HIL smoke: `cargo hil run ieee802154-air-check ieee802154-peer-exchange ieee802154-thread-exchange` (`hil-run` skill); PHY: `phy-vendor-calibration-ieee802154`.
- Vendor comparison: the host stand's `compare` and its `ieee802154-host` shard (`vendor-evidence` skill).

Skills: `protocol-change`, `driver-or-hardware-change`, `hil-run`, `vendor-evidence`, `qualification-entry` (catalog `qualification/catalog/esp32s31/ieee802154.toml`, program `qualification/targets/esp32s31/ieee802154.toml`), `push-and-ci`.

Package detail: the READMEs of the [composition](../../../crates/composition/esp32s31/embassy/ieee802154/README.md), [engine](../../../crates/hardware/espressif/ieee802154/engine/README.md), [OpenThread adapter](../../../crates/adapters/openthread/ieee802154/README.md), [Thread example](../../../examples/esp32s31/thread/README.md), [vendor host stand](../../../verification/esp32s31/host/ieee802154/README.md) and the [capability page](../../../crates/hardware/esp32s31/driver/ieee802154/FEATURES.md); the protocol, role, runtime and adapter document themselves in their crate-root rustdoc. Vendor behavior: `docs/vendor/esp32s31/ieee802154-*.md`.

The shared lifecycle borrows `B` in its owner and consumes an independent,
unbranded driver once. The operation permit and every branded in-flight
owner live in `Held<'id>`. Phase parts are disjoint borrows of `owner.phase()`;
none is moved out of its loan. A dropped operation permit returns itself to
its source pool; submission transfers the actual token, with no empty guard.

`Ieee802154PortEvent` contains only owned RX frames and operation terminals.
Each terminal carries its interface, recovered from the matching affine
`Completed` proof; a foreign request ID retains the in-flight permit and
poisons the port. Lifecycle and faults never masquerade as port events.

The runtime's emergency path closes one atomic admission/count word before
masking the already-bound route. The adapter retains a static reference to that
route: emergency masking never takes the mutex that contains `radio.isr`.
ISR entry and task hardware operations retain a counted entry through their
accesses; late entries reject before locking. A finite CPU join always ends in
`Unconfirmed`, because CPU exclusion alone cannot prove MAC/DMA completion.
Install requires static runtime and engine buffers. The route claim and all
backing remain quarantined until chip reset; ordinary uninstall keeps its
existing hardware sequence. Never turn a successful CPU join into permission
to return or reuse the engine's DMA backing.
