---
name: block-bluetooth
description: Use when a task changes or investigates the Bluetooth LE block of this repository — HCI codecs, the Link Layer, the LE Controller's pure timing and receive pieces, the LE radio values, the ESP32-S31 Bluetooth driver and controller memory, Bluetooth vendor comparison, or rebuilding the Bluetooth orchestration. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# Bluetooth LE block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[ownership and refusals](../../../docs/architecture.md#ownership-and-refusals),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## State

`main` holds the block's recovered knowledge and pure algorithms only. The
orchestration was deleted by #770: the port backend (`LeRadioBackend`), the
service loop (`serve`), the in-process HCI transport, the S31 radio role,
runtime and composition, the Trouble GATT adapter, the Controller's planning
core and HCI command service, the Bluetooth trace, and the Bluetooth HIL. The
tag `legacy-radio-2026-10-16` keeps them (and `legacy-wifi-2026-10-16` keeps
`main` before #756). The redesign is #563 decision 8: the upper link layer
sans-IO in the body, the lower link layer in the P2 driver, a port shaped like
the lower-MAC port. Nothing on `main` runs a Controller or reaches the air.

## Packages (path → role)

- Protocols, portable sans-IO: `crates/protocols/bluetooth/` —
  - `hci` (`oer-bluetooth-hci`): packets, codecs, bootstrap, credits and the Controller-to-Host packet validation (`transport/packet.rs`);
  - `le/ll` (`oer-bluetooth-ll`): CSA#1/#2, channel map, instants, widening, DLE, session key and CCM, advertising and scan codecs, the duplicate filter, and DTM packet generation and report counting;
  - `le/controller` (`oer-bluetooth-controller`): the pure pieces only — authenticated RX decode (`receive.rs`), peripheral timing and advertising geometry (`timing.rs`, with `EpochExhausted`), the `0xFC01` diagnostic codec, and the vendor dynamic coexistence priorities (`coexistence.rs`);
  - `le/radio` (`oer-bluetooth-radio`): the `LeRadio` clock domain, channels, PDUs, capabilities, `Supported` and the request value types (no correlation ID).
- ESP32-S31:
  - driver `crates/hardware/esp32s31/driver/bluetooth` (`oer-esp32s31-bluetooth`): controller HAL and time, BLE PHY, interrupts, scheduler. Since #770 it also holds the vendor PTI lanes (`coexistence.rs`), the connection-event policy (`connection_policy.rs`) and the `coexHook` status bits (`coex_status.rs`); its README records the bring-up and teardown order;
  - its `memory` (`oer-esp32s31-bluetooth-memory`);
  - HAL partition `crates/hardware/esp32s31/hal/src/bluetooth.rs` (`ColdOwner` → `PoweredOwner` → `ClockedOwner` → `TaskOwner`);
  - PHY client `crates/hardware/esp32s31/phy/src/bluetooth_client.rs`;
  - `bluetooth_address.rs` (address = Wi-Fi STA base + 1) in `crates/adapters/esp-hal/esp32s31/radio`.
- Facade: `crates/oer` features `bluetooth` (HCI and LL) and `esp32s31-bluetooth` (the hardware engine).
- No HIL image, scenario or host family. The reference peer `hil/peers/esp32c5-ble-dtm` stays for the interop rebuild (#773).

## Shared contracts

Changing or removing existing public API of [the shared crates](../../../docs/architecture.md#changing-a-shared-contract) follows the shared-change procedure; an additive change is the block's own, and its PR names the crate it extends. This block uses these shared crates and items: `crates/radio/port` (#763), `crates/time` (`LeInstant` = `RadioInstant<LeRadio>`, `Timer`), `crates/radio/coex` (`CoexPriority`), the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`BluetoothPartition`) and `shared_radio.rs` (the `radio-system` lane, #771), PAC and `registers/` (`model/peripherals/bluetooth-*`, `ble-*`).

## Traps

- The LE radio clock is the controller clock extended from its 32-bit latch, with no relation to the monotonic clock: never convert an `LeInstant` through monotonic time. The modem LP timer is an alarm of its own domain, never an `oer_time::Timer`.
- Controller time has two raw ticks per microsecond; fractional ticks round down, and scheduler positions wrap only inside the driver (`driver/bluetooth/README.md`, "Controller time"). A raw captured anchor is not a packet-start timestamp (`driver/bluetooth/FEATURES.md`, "Peripheral timing limits"). The long recurring receive wait counts two-microsecond units, so a backend rounds an odd wait up.
- Controller memory is initialized internal SRAM (`.dma.data.open_radio_bluetooth_*`, the `controller_memory!` sections), not a zeroed `.bss` region, and is claimed once per boot (`BluetoothMemoryError::AlreadyClaimed`).
- Lifecycle facts for the rebuild: power and clock failures roll back to the parked client; any failure after the first Controller write keeps its owners fail-stop until reset. RF closes only when the last PHY client leaves. Initialization and scheduler `RUN` are not RF evidence.
- PHY order: `esp_phy_enable(PHY_MODEM_BT)` and BTBB after the controller HAL init; unlike IEEE 802.15.4, Bluetooth keeps the transmit-on delay the first BTBB initialization writes (`bluetooth_client.rs`). The vendor's modem ETM channel zero belongs to IEEE 802.15.4 here, so production routes channel two (`verification/esp32s31/README.md`, BLE PHY register initialization).
- Three interrupt sources (124, 127, 133) go to one dispatcher at level 3 on core 0; both HAL owners must be installed before any CPU route is enabled (driver README, "Bring-up and teardown order").
- The hardware scheduler keeps the air timing, so a primary interrupt only services the Controller's status and wakes the driver; every decision belongs to the driver ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals), #643, #563 decision 2).
- Narrow on purpose: LE 1M only for connections, no peripheral latency, 0 dBm for every role, LE 2M and Coded only in DTM, always awake on the main-XTAL profile. Feature bits stay clear until a production path is complete; a recovered register, SRAM layout, parser or DTM-only PHY never grants advertisement (`driver/bluetooth/FEATURES.md`, "Capability advertisement").
- `ControllerPoweredTaskRuntime::publish_scan_start` is `unsafe`: its caller must reset the scanner graph and publish the receive chains first, which the deleted radio role did.
- The controller archives' symbols are obfuscated and change between releases: a vendor leaf names the symbol of the pinned release; source names come from `verification/esp32s31/facts/names/` (grep only).

## Checks

- `cargo test -p <package>`.
- RV32: `cargo check -p oer-esp32s31-bluetooth --target riscv32imafc-unknown-none-elf`.
- Vendor comparison: the `bluetooth` scenario over the probes in `verification/esp32s31/probes/bluetooth`.

Package detail: [S31 driver](../../../crates/hardware/esp32s31/driver/bluetooth/README.md) and its [feature page](../../../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md); the other packages document themselves in their crate-root rustdoc. Vendor behavior: `docs/vendor/esp32s31/bluetooth-*.md`.
