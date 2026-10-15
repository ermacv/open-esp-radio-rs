---
name: block-bluetooth
description: Use when a task changes or investigates the Bluetooth LE block of this repository — HCI, the Link Layer, the LE Controller core, the LeRadioBackend contract, the in-process HCI transport and service loop, the ESP32-S31 Bluetooth driver, controller memory, radio role, runtime or composition, the Trouble GATT adapter, Bluetooth vendor comparison, or Bluetooth and Wi-Fi/BLE coexistence HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# Bluetooth LE block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[ownership and refusals](../../../docs/architecture.md#ownership-and-refusals),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## Packages (path → role)

- Protocols, portable sans-IO: `crates/protocols/bluetooth/` — `hci` (`oer-bluetooth-hci`, packets and codecs), `le/ll` (`oer-bluetooth-ll`, Link Layer PDUs, connection state, control procedures, encryption, DTM planner), `le/controller` (`oer-bluetooth-controller`, `LeController`: HCI command service, Link Layer roles, the event arbiter), `le/radio` (`oer-bluetooth-radio`, the port backend).
- Services and portable runtime: `crates/services/bluetooth/hci/transport` (`oer-bluetooth-hci-transport`, bounded in-process HCI between Host and Controller); `crates/runtime/bluetooth` (`oer-bluetooth-runtime`, `serve`: transport, Controller core and any `LeRadioBackend`, as the body of its port).
- ESP32-S31: driver `crates/hardware/esp32s31/driver/bluetooth` (`oer-esp32s31-bluetooth`: controller HAL and time, BLE PHY, interrupts, scheduler) and its `memory` (`oer-esp32s31-bluetooth-memory`); HAL partition `crates/hardware/esp32s31/hal/src/bluetooth.rs` (`ColdOwner` → `PoweredOwner` → `ClockedOwner` → `TaskOwner`); PHY client `crates/hardware/esp32s31/phy/src/bluetooth_client.rs`; role `crates/roles/esp32s31/bluetooth/radio` (`BluetoothRadio`); runtime `crates/runtime/esp32s31/bluetooth` (`BluetoothRuntime`); composition `crates/composition/esp32s31/embassy/bluetooth` (`oer-esp32s31-bluetooth-system`: `BluetoothParked`, `start_bluetooth_hci`); esp-hal interrupt routes and handlers in `crates/adapters/esp-hal/esp32s31/radio` (`bluetooth_interrupt.rs`, `bluetooth_handlers.rs`).
- Trace: `crates/protocols/bluetooth/trace` (`oer-bluetooth-trace`, events of `Domain::Bluetooth`, channel table in its README). The Controller, the S31 role and runtime record behind their own `trace` feature (the composition's `trace`, the agent's `bluetooth-trace`); host tests read the thread's records with `crate::trace::take`. Image `diagnostic-bluetooth-gatt-trace`, scenario `bluetooth-trouble-gatt-trace`.
- Host binding: `crates/adapters/trouble/bluetooth/gatt` (`oer-bluetooth-gatt-trouble`, GATT over the Trouble Host, composed by the HIL firmware).
- Facade: `crates/oer` features `bluetooth`, `esp32s31-bluetooth`. No example application. HIL: `hil/scenarios/bluetooth/`, workloads and the Linux fixture in `hil/host/family/bluetooth`, Wi-Fi/BLE load in `hil/host/family/coexistence`; agent features `bluetooth-*` and `wifi-ble-coex` in `hil/targets/esp32s31/agent`.

## Port

`LeRadioBackend` (`crates/protocols/bluetooth/le/radio/src/port.rs`) is a `PortBackend` of the shared port lifecycle (`oer-radio-port`, `crates/radio/port/src/lifecycle.rs`; #709) and has no extension traits: optional roles, PHYs, link acknowledgement, receive-wait limits, the horizon and the timing are `LeRadioCapabilities` behind the port's capabilities marker `type Caps: LeCapabilities` (`const CAPS`, known when the image is built; the S31 marker is `BluetoothCapabilities<C, pools…>`, built from the radio config `C` with the radio's own `capabilities`/`radio_timing`). Every request carries `Supported<K, _>` values: the core builds them through `K` (timed ones `Supported::placed` at its `now`, with the one reservation function `RadioTiming::reservation` the backend also uses), so a backend refuses nothing they exclude and overlapping work remains governed by the validated reservation; a Host request `K` excludes is an HCI status when the command is taken, and `serves_the_core` (LE 1M at 0 dBm, the DLE maximum with its MIC) is a `const` check where the core is created. Refusals left are the world's (`Late`, `ClockUnavailable`, `EpochExhausted`) and those of the role, event and accept-list state (`Unknown`, `AlreadyConfigured`, `NoInstance`, `Busy`, `ListFull`, `NotListed`); an S31 structure that disagrees is a `RadioFault`, not a refusal. The S31 long recurring receive wait counts two-microsecond units, so the radio rounds an odd wait up (`recurring_wait`). The phase's client `submit`, `cancel` and `client_now`, the backend's `now`, `enable`, `disable` and the events part's `next_event` are async (a fresh controller-time latch per request; the trait's documentation), `view` and `activity` synchronous, and `CLOCK` a constant; `Enable` is never refused (`Refusal = Infallible`). The S31 backend is `Installed<BluetoothBackend>` with a separate `BluetoothDriver` (`crates/runtime/esp32s31/bluetooth/src/port.rs`, `runtime.rs`; the joined driver, owner decisions on #581: comments 6120815544 and 6121016144). `install` returns it; `run_port` consumes it and lends its retained driver to static `drive` and joins it with the body for the port's whole life: the driver owns the radio role and hardware by value, drives the scheduler, samples the controller clock every 60 s enabled or disabled, and serves the port's requests as messages that own their PDUs (`OwnedRadioRequest`, `Supported::map_storage`). No shared lock spans an await (the driver alone lends its private cell), and nothing beside the port reaches the radio: the interrupt entry signals a wake, `BluetoothSystem::run` keeps only the modem timer task, interrupt faults and coexistence publication, and the coexistence profile is an atomic. The uninstall retires the driver, which returns the radio and hardware (`into_parts`), or keeps them with why (`BluetoothRetained`) when the owner was poisoned while enabled; `BluetoothSystem::stop` takes that result. Shared-PHY maintenance never pauses the port (it runs under the arbiter lease with the vendor grant-protect brackets, and an event that loses RF ends as an ordinary outcome). `NoRadio` is the host stub.

## Shared contracts (change only with their owners)

`crates/radio/port` (`ClockInfo`, `ClockRelation`, `EventsLost`, failure classes), `crates/time` (`LeInstant` = `RadioInstant<LeRadio>`, `Timer`), `crates/radio/coex` (`CoexPriority`), the shared radio system `crates/runtime/esp32s31/radio` and `crates/composition/esp32s31/embassy/radio`, the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`BluetoothPartition`) and `shared_radio.rs`, PAC and `registers/` (`model/peripherals/bluetooth-*`, `ble-*`).

## Traps

- `PortBackend::Pools` declares the installation's shared token layout. The current BLE backends use `()` and consume `Mint::into_pools(self)` once; controller-internal pools remain in the controller. No per-kind Mint constructors exist.

- The LE radio clock is the controller clock extended from its 32-bit latch, with no relation to the monotonic clock (`BluetoothBackend::CLOCK` publishes `monotonic: None`): never convert an `LeInstant` through monotonic time. The runtime samples it at least every `TIME_REFRESH` (60 s) to keep the extension; the modem LP timer is an alarm of its own domain, armed through a port operation, never an `oer_time::Timer`.
- Controller time has two raw ticks per microsecond; fractional ticks round down, and scheduler positions wrap only inside the driver (`driver/bluetooth/README.md`, "Controller time"). A raw captured anchor is not a packet-start timestamp (catalog section `bluetooth-peripheral-timing-limits`). The S31 radio epoch starts 2^32 µs above zero and ends 2^32 µs below `u64::MAX` (`radio.rs`, `EPOCH_MARGIN`), so every capture and its packet-start correction are representable by construction; a sample past the end reports the epoch exhausted (`ClockError::EpochExhausted`) instead of advancing.
- Events never overlap: the Controller core resolves role conflicts through one arbiter and places every window with the reservation function the backend shares (`RadioTiming::reservation`), so the S31 radio meets no overlapping window (a `debug_assert`, not a refusal) and never moves one. The radio role holds no register; the caller serializes every entry (`BluetoothRadio`).
- `serve` goes on after `EventsLost` (only advertising and scan reports can be lost; ends and connection data hold reserved slots). Radio epoch exhaustion has one exit, `ServeExit::EpochExhausted`, from the clock (`ClockError`/`RequestError::EpochExhausted`) or from planning (`oer_bluetooth_controller::EpochExhausted`, the planner's only error); every other planning failure is unrepresentable by type.
- `serve` is the port's body: it enables the port, serves one phase, then disables and uninstalls it; a poisoned port drains preceding outcomes and uninstalls with its retained hardware owners (`Served::Poisoned` or `Served::DisableFailed`). The body returns `Served` as `PortBody::Output`; `PortEnd` carries it with returned owners or a fail-stop outcome, while an abandoned owner has no body result. Ordinary service shutdown awaits it. Cancellation of the whole port poisons the runtime permanently and reports `Unconfirmed`: the current backend cannot prove synchronous hardware stop. The chip must be reset before reuse; cancellation is not a restart path. Poll `BluetoothSystem::run` beside it; an interrupt or modem-timer fault poisons the port first (`BluetoothRuntime::platform_fault`), so keep polling the body to its end. The S31 `disable` runs within `TRANSITION_BOUND` (10 ms, owner decision on #581 from a 121 µs measured stop; `enable` shares it) and poisons past it: it stops the scheduler, so a listed event ends with its recorded completion, `NotExecuted` when its start lay ahead of the stop, or `EventResult::Aborted` when it started without a status, and hands every outcome the phase left to the sink; an admitted event still owing its end afterwards is `BluetoothRuntimeFault::Unsettled`. The trace's `Lifecycle` records time each transition.
- Controller memory is initialized internal SRAM (`.dma.data.open_radio_bluetooth_*`, the `controller_memory!` sections), not a zeroed `.bss` region, and is claimed once per boot (`BluetoothMemoryError::AlreadyClaimed`): host bonds and connections do not survive a restart (source fact `bluetooth-same-storage-powered-restart`).
- Lifecycle: power and clock failures roll back to the parked client; any failure after the first Controller write keeps its owners fail-stop until reset. RF closes only when the last PHY client leaves the shared radio. Initialization and scheduler `RUN` are not RF evidence.
- PHY order: `esp_phy_enable(PHY_MODEM_BT)` and BTBB after the controller HAL init; unlike IEEE 802.15.4, Bluetooth keeps the transmit-on delay the first BTBB initialization writes (`bluetooth_client.rs`). The vendor's modem ETM channel zero belongs to IEEE 802.15.4 here, so production routes channel two (`verification/esp32s31/README.md`, BLE PHY register initialization).
- Three interrupt sources (124, 127, 133) go to one dispatcher; both HAL owners are installed before any CPU route is enabled (`bluetooth_interrupt.rs`).
- Interrupt model ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals), #643): the hardware scheduler keeps the air timing, so the primary interrupt only services the Controller's status through its interrupt owner and signals the driver (`on_scheduler_wake`); every decision is the driver's, which runs inside the port (#581). Never move scheduling logic into the interrupt.
- Narrow on purpose: LE 1M only for connections, no peripheral latency, one peripheral connection in the product (the Controller core holds `CONNECTIONS` of them, set by the composition; #376 raises it), 0 dBm for every role, LE 2M and Coded only in DTM, always awake on the main-XTAL profile (no modem or LP sleep). Feature bits stay clear until a production path is complete; a recovered register, SRAM layout, parser or DTM-only PHY never grants advertisement (catalog section `bluetooth-capability-advertisement`).
- Coexistence: each epoch enables Bluetooth coexistence at start and disables it at stop after withdrawing every status bit (composition `coex.rs`). Alone on the antenna every event requests it at the highest priority (role `coexistence.rs`); PHY tracking belongs to `RadioSystem::run_tracking` and does not wait for a gap between Bluetooth events.
- Secure GATT accepts only LE Secure Connections Numeric Comparison; Just Works, passkey, OOB and legacy pairing are not fallbacks (`adapters/trouble/bluetooth/gatt/README.md`).
- The controller archives' symbols are obfuscated and change between releases: a vendor leaf names the symbol of the pinned release; source names come from `verification/esp32s31/facts/names/` (grep only).

## Checks

- `cargo test -p <package>` (the Trouble adapter with `--all-features`), then `cargo xtask check changed`.
- `cargo hil images check --class bluetooth-gatt --type-check` for chip-side changes (also `bluetooth-dtm`, `bluetooth-secure-gatt`, `wifi-ble-coex`).
- HIL smoke: `cargo hil run bluetooth-dtm-bidirectional bluetooth-trouble-gatt bluetooth-trouble-secure-gatt` (`hil-run` skill); coexistence `wifi-udp-rx-ble-echo`.
- Vendor comparison: the `bluetooth` scenario over the probes in `verification/esp32s31/probes/bluetooth` (`vendor-evidence` skill).

Skills: `protocol-change`, `driver-or-hardware-change`, `hil-run`, `vendor-evidence`, `qualification-entry` (catalogs `qualification/catalog/esp32s31/bluetooth.toml` and `bluetooth-products.toml`; programs `bluetooth-le`, `bluetooth-peripheral-acl`, `bluetooth-secure-gatt`), `push-and-ci`.

Package detail: [HCI transport](../../../crates/services/bluetooth/hci/transport/README.md), [S31 driver](../../../crates/hardware/esp32s31/driver/bluetooth/README.md) and its [capability page](../../../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md), [Trouble GATT](../../../crates/adapters/trouble/bluetooth/gatt/README.md); the other packages document themselves in their crate-root rustdoc. Vendor behavior: `docs/vendor/esp32s31/bluetooth-*.md`.

The lifecycle driver is unbranded. Installation-branded queues and in-flight
owners belong in `Held<'id>`, never in `Driver`; see `oer-radio-port`'s
`PortBackend::drive`. A runtime fault is published into `PoisonState` before
the driver consumes the publication and its loan in `DriverLoan::fail_stop`;
its affine `PoisonProof` then accompanies `DriverExit::Faulted`. The body drains
earlier outcomes before poisoned uninstall returns the retained owners.

The core owns the driver throughout `run_port`. `drive` consumes a closed
`DriverLoan` with shared access; mutable BLE progress is in the driver's
private cell, unreachable by ISR/task entry. Cancellation releases that
borrow, while `Quarantined` retains driver/backing owners without Drop.
Ordinary uninstall receives `(Driver, State)` through `Retired`. DMA backing
must be static and separate from the movable driver and future.

The retirement handoff also consumes the loan. Each await must preserve
retire/abandon-safe state in the driver or durable backing. Runtime tests
cancel clock, transition, stop and retirement waits at successive polls,
then retire/uninstall the same owners.

Runner faults must end software accounting before the event stream reports
poison. `BluetoothRadio::end_faulted` takes only software event records; it
never releases a pool item or reads device-owned memory. Successful RUN marks
the idle list exposed; live insertion marks an event when its SRAM link enters
the running list, before the remaining publication actions. A refused start
publishes no RUN (including `SchedulerStartError::Interrupts`, which can have
published the head). Unexposed events end `NotExecuted`; possibly executed
ones end `Indeterminate`, which grants no memory release. Runtime fault
publication can precede these terminals, so its event consumer waits for the
driver's accounting completion. Failed teardown returns `BluetoothRetained`
with the same shared `Quarantined` used by the lifecycle core, keeping the
role, hardware and any PHY route without Drop; only confirmed retirement
returns reusable owners.

`PortPhase::next_event` ends with the common `EventEnd`: `Drained` promises
all admitted terminals were delivered; missing terminals violate the contract.
`Quarantined` immediately ends without taking queue or fault locks and leaves
unread owners in `PortEnd`. The phase keeps its final variant and cause in
`Local`, including across phase reborrows. The portable service fail-stops
on quarantine without disable or more draining. Commands keep `PortResult`.
