---
name: block-bluetooth
description: Use when a task changes or investigates the Bluetooth LE block of this repository — HCI, the Link Layer, the LE Controller core, the LeRadioPort contract, the in-process HCI transport and service loop, the ESP32-S31 Bluetooth driver, controller memory, radio role, runtime or composition, the Trouble GATT adapter, Bluetooth vendor comparison, or Bluetooth and Wi-Fi/BLE coexistence HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# Bluetooth LE block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## Packages (path → role)

- Protocols, portable sans-IO: `crates/protocols/bluetooth/` — `hci` (`oer-bluetooth-hci`, packets and codecs), `le/ll` (`oer-bluetooth-ll`, Link Layer PDUs, connection state, control procedures, encryption, DTM planner), `le/controller` (`oer-bluetooth-controller`, `LeController`: HCI command service, Link Layer roles, the event arbiter), `le/radio` (`oer-bluetooth-radio`, the port).
- Services and portable runtime: `crates/services/bluetooth/hci/transport` (`oer-bluetooth-hci-transport`, bounded in-process HCI between Host and Controller); `crates/runtime/bluetooth` (`oer-bluetooth-runtime`, `serve`: transport, Controller core and any `LeRadioPort`).
- ESP32-S31: driver `crates/hardware/esp32s31/driver/bluetooth` (`oer-esp32s31-bluetooth`: controller HAL and time, BLE PHY, interrupts, scheduler) and its `memory` (`oer-esp32s31-bluetooth-memory`); HAL partition `crates/hardware/esp32s31/hal/src/bluetooth.rs` (`ColdOwner` → `PoweredOwner` → `ClockedOwner` → `TaskOwner`); PHY client `crates/hardware/esp32s31/phy/src/bluetooth_client.rs`; role `crates/roles/esp32s31/bluetooth/radio` (`BluetoothRadio`); runtime `crates/runtime/esp32s31/bluetooth` (`BluetoothRuntime`); composition `crates/composition/esp32s31/embassy/bluetooth` (`oer-esp32s31-bluetooth-system`: `BluetoothParked`, `start_bluetooth_hci`); esp-hal interrupt routes and handlers in `crates/adapters/esp-hal/esp32s31/radio` (`bluetooth_interrupt.rs`, `bluetooth_handlers.rs`).
- Host binding: `crates/adapters/trouble/bluetooth/gatt` (`oer-bluetooth-gatt-trouble`, GATT over the Trouble Host, composed by the HIL firmware).
- Facade: `crates/oer` features `bluetooth`, `esp32s31-bluetooth`. No example application. HIL: `hil/scenarios/bluetooth/`, workloads and the Linux fixture in `hil/host/family/bluetooth`, Wi-Fi/BLE load in `hil/host/family/coexistence`; agent features `bluetooth-*` and `wifi-ble-coex` in `hil/targets/esp32s31/agent`.

## Port

`LeRadioPort` (`crates/protocols/bluetooth/le/radio/src/port.rs`) has no extension traits: optional roles, PHYs and link acknowledgement are `LeRadioCapabilities`, and a request they exclude is refused `Unsupported`. `clock`, `submit` and `next_outcome` are async (a fresh controller-time latch per request; the trait's documentation), `capabilities`, `clock_info`, `view` and `activity` synchronous. Cancel is `RadioRequest::Cancel`. The S31 backend is `BluetoothRuntime` (`runtime.rs`): its `install` returns the port `BluetoothPort` and the composition's `BluetoothControl` (`crates/runtime/esp32s31/bluetooth/src/port.rs`), whose `quiesce` (shared-PHY maintenance over `Quiesce`) and `uninstall` (consuming both) are the backend's own operations; no call is refused as not installed. The composition's `start` returns the port with the `BluetoothSystem`, `BluetoothHciService::run` borrows it and `BluetoothSystem::stop` takes it back. `NoRadio` is the host stub. `oer_bluetooth_runtime::serve` is the port's one outcome consumer.

## Shared contracts (change only with their owners)

`crates/radio/port` (`ClockInfo`, `RadioEpoch`, `EventsLost`, failure classes), `crates/time` (`LeInstant` = `RadioInstant<LeRadio>`, `Timer`), `crates/radio/coex` (`CoexPriority`), the shared radio system `crates/runtime/esp32s31/radio` and `crates/composition/esp32s31/embassy/radio`, the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`BluetoothPartition`) and `shared_radio.rs`, PAC and `registers/` (`model/peripherals/bluetooth-*`, `ble-*`).

## Traps

- The LE radio clock is the controller clock extended from its 32-bit latch, `RadioEpoch::Unrelated` to the monotonic clock (`BluetoothRuntime::clock_info`): never convert an `LeInstant` through monotonic time. The runtime samples it at least every `TIME_REFRESH` (60 s) to keep the extension; the modem LP timer is an alarm of its own domain, armed through a port operation, never an `oer_time::Timer`.
- Controller time has two raw ticks per microsecond; fractional ticks round down, and scheduler positions wrap only inside the driver (`driver/bluetooth/README.md`, "Controller time"). A raw captured anchor is not a packet-start timestamp (catalog section `bluetooth-peripheral-timing-limits`). A failed timestamp never opens or updates a connection time reference (`LeController`).
- Events never overlap: the Controller core resolves role conflicts through one arbiter before a request reaches the backend, and the backend refuses an overlapping window instead of moving it. The radio role holds no register; the caller serializes every entry (`BluetoothRadio`).
- `serve` ends on `EventsLost`: the core accounts every event by its end and cannot recover one lost in the gap. A request refused `Unsupported` is never retried on a timer; a schedule outside the radio epoch ends through `ServeExit::Planning`.
- Controller memory is initialized internal SRAM (`.dma.data.open_radio_bluetooth_*`, the `controller_memory!` sections), not a zeroed `.bss` region, and is claimed once per boot (`BluetoothMemoryError::AlreadyClaimed`): host bonds and connections do not survive a restart (source fact `bluetooth-same-storage-powered-restart`).
- Lifecycle: power and clock failures roll back to the parked client; any failure after the first Controller write keeps its owners fail-stop until reset. RF closes only when the last PHY client leaves the shared radio. Initialization and scheduler `RUN` are not RF evidence.
- PHY order: `esp_phy_enable(PHY_MODEM_BT)` and BTBB after the controller HAL init; unlike IEEE 802.15.4, Bluetooth keeps the transmit-on delay the first BTBB initialization writes (`bluetooth_client.rs`). The vendor's modem ETM channel zero belongs to IEEE 802.15.4 here, so production routes channel two (`verification/esp32s31/README.md`, BLE PHY register initialization).
- Three interrupt sources (124, 127, 133) go to one dispatcher; both HAL owners are installed before any CPU route is enabled (`bluetooth_interrupt.rs`).
- Narrow on purpose: LE 1M only for connections, no peripheral latency, one peripheral connection handle, 0 dBm for every role, LE 2M and Coded only in DTM, always awake on the main-XTAL profile (no modem or LP sleep). Feature bits stay clear until a production path is complete; a recovered register, SRAM layout, parser or DTM-only PHY never grants advertisement (catalog section `bluetooth-capability-advertisement`).
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
