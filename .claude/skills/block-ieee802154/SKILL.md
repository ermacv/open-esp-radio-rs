---
name: block-ieee802154
description: Use when a task changes or investigates the IEEE 802.15.4 block of this repository — the portable 802.15.4 MAC values and port (owner shape), the Espressif MAC engine ported from ESP-IDF, the ESP32-S31 HAL and PHY parts of 802.15.4, the 802.15.4 vendor host stand, or rebuilding the 802.15.4 orchestration. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# IEEE 802.15.4 block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[ownership and refusals](../../../docs/architecture.md#ownership-and-refusals),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## State

`main` holds the block's recovered knowledge and pure algorithms only. The
orchestration was deleted by #770: the radio role and its legacy state machine
(`RadioStateMachine`, `RadioCommand`, `RadioEvent`), the runtime and its
backend, the ESP32-S31 composition, the esp-hal interrupt route, the HAL
polled ED/CCA and validation probes, the OpenThread adapter and the Thread
example. The tag `legacy-radio-2026-10-16` keeps them (and
`legacy-wifi-2026-10-16` keeps `main` before #756). The redesign is #563
decision 8: the engine runs in the P3 context with no mutex, body and engine
are linked lock-free (a mailbox, a configuration snapshot and SPSC events),
and CSMA and retries move above the port. Nothing on `main` runs the engine on
hardware.

## Packages (path → role)

- Protocol, portable: `crates/protocols/ieee802154` (`oer-ieee802154`): MAC frames, CSMA, CSL, enhanced ACK, security, time sync, the port's owner shape (`owner`, `port`), capabilities, channels, interfaces, power, and the host model of the owner shape, used by its own tests. No portable service package.
- Family (Espressif, S31 and C5):
  - engine `crates/hardware/espressif/ieee802154/engine` (`oer-espressif-ieee802154-engine`): the MAC driver ported from public ESP-IDF, `Ieee802154LowLevel`, a host register model for its own tests (`ll::model`, `cfg(test)`), and since #770 the capabilities table (`capabilities.rs`: `RADIO_CAPABILITIES`, `CCA_RESET_DBM`, `capabilities(levels, multipan)`). Its README records the `esp_ieee802154_enable` bring-up and teardown order;
  - shared MAC values `crates/hardware/ieee802154/pac`.
- ESP32-S31:
  - PAC, HAL and PHY modules inside the chip crates (`hardware/esp32s31/{pac,hal}/src/ieee802154*`, `phy/src/ieee802154_client.rs`);
  - `Ieee802154MacOwners` implements the engine's low level; the HAL lifecycle typestate is `role.rs`, and `policy.rs` stays whole;
  - `crates/hardware/esp32s31/driver/ieee802154/` holds only the capability page.
- Facade: `crates/oer` features `ieee802154` and `esp32s31-ieee802154` (the engine).
- No HIL image, scenario or host family. The reference peers `hil/peers/esp32c5-ieee802154` (the PHY cross-check's vendor side) and `hil/peers/esp32c5-openthread` (#774) stay.

## Shared contracts

Changing or removing existing public API of [the shared crates](../../../docs/architecture.md#changing-a-shared-contract) follows the shared-change procedure; an additive change is the block's own, and its PR names the crate it extends. This block uses these shared crates and items: `crates/radio/port` (#763), `crates/time` (`Ieee802154Instant` = `RadioInstant<Ieee802154Radio>`, `Clock`, `Timer`), `crates/radio/coex` and `crates/hardware/espressif/coex`, the PHY concurrent domain, the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`Ieee802154RadioPartition`) and `shared_radio.rs` (the `radio-system` lane, #771), PAC and `registers/`.

## Traps

- The radio clock was the runtime's monotonic clock (`ClockInfo::MONOTONIC_MICROS`), unlike Wi-Fi's affine MAC time; the engine stamps each frame at its SFD with the platform microsecond clock.
- The engine takes every deadline decision inside its ISR (`Ieee802154Engine::isr`: ACK and pending bit, enhanced ACK, ACK watchdog, next operation), as the vendor does: the ACK, enhanced-ACK, pending-bit and TX SFD frame-patch decisions must land inside the 192 µs turnaround and cannot move to a task ([driver port](../../../docs/vendor/esp32s31/ieee802154-driver-port.md)). A FROM_CPU doorbell raise reached its handler in ~2 µs, ≤ 4.74 µs under 60 Mbit/s Wi-Fi RX (the former HIL image `diagnostic-interrupt-latency`, tag `legacy-wifi-2026-10-16`).
- Configuration (addresses, interface enable, the pending table, promiscuous, auto-ACK, coordinator) applies in any state, as ESP-IDF's `esp_ieee802154_set_*` do.
- Modem source 132 has one handler and the MAC one set of owners. The route order was: activate the HAL interrupt owner, install, then bind; teardown reverses it. The handler lives in SRAM.
- The MAC DMA reaches internal SRAM only (`IEEE802154_DMA_WINDOW`): a frame in PSRAM is a DMA error and never reaches the air. A frame received into the stub buffer is dropped.
- `EVENT_STATUS` is write-one-to-clear, sampled into an affine token no caller can forge or replay; a stale image fails closed before `ED_START`. A `STOP` write is not proof of DMA quiescence ([dataplane](../../../docs/vendor/esp32s31/ieee802154-dataplane.md), [control](../../../docs/vendor/esp32s31/ieee802154-control.md)).
- Platform clock references go back to ESP-HAL only through their owner's explicit release under a lease: common power (`exit_common_power`), the modem clock planner (`disable_modem_clocks`) and the Bluetooth low-power clock (`deselect_bluetooth_low_power_clock`). With `CLK_COEX_EN` closed under the MAC, every ED is rejected as `EdCoexistenceReject`.
- A failure before shared hardware work rolls back to the parked partition; a started PHY, clock or power transaction that fails stays fail-stop until reset.
- PHY: tracking runs with the MAC receiving, as ESP-IDF's `phy_common.c` does; an operation that loses RF ends with its ordinary terminal (`TxStatus::CoexistenceRejected` for a denied transmission; a denied reception is a silent miss). RF stays open for the client's lifetime; there is no modem retention or light sleep with the pinned libphy. The first and last PHY client policy is in the PHY crate's README.
- Scheduled TX uses TIMER0 and the modem ETM, scheduled RX TIMER1; ETM channels zero and one belong to IEEE 802.15.4 (BLE routes channel two).
- Coexistence: the participant counting, status bits and join/leave order are in `oer-espressif-coex`'s README; external coexistence does nothing on the pinned libcoexist. `recent_rssi` reads the shared BTBB byte of the last reception, whichever protocol received it.
- Security: a retransmission keeps its frame counter and key index; only a CSL receiver gets a new counter per retry.
- No readiness claim beyond the catalog: Thread, Zigbee and Matter are host-only, and CSMA/CA and CSL are outside the qualified capabilities.

## Checks

- `cargo test -p <package>` (the register model of the engine is test-only), then `cargo xtask check changed`; the vendor host stand is its own workspace: `cargo test --manifest-path verification/esp32s31/host/ieee802154/Cargo.toml`.
- RV32: `cargo check -p oer-esp32s31-hal -p oer-espressif-ieee802154-engine --target riscv32imafc-unknown-none-elf`.
- Vendor comparison: the host stand's `compare` and its `ieee802154-host` shard (`vendor-evidence` skill).

Skills: `protocol-change`, `driver-or-hardware-change`, `vendor-evidence`, `qualification-entry` (catalog `qualification/catalog/esp32s31/ieee802154.toml`; no 802.15.4 program exists), `push-and-ci`.

Package detail: the READMEs of the [engine](../../../crates/hardware/espressif/ieee802154/engine/README.md), the [vendor host stand](../../../verification/esp32s31/host/ieee802154/README.md) and the [capability page](../../../crates/hardware/esp32s31/driver/ieee802154/FEATURES.md); the protocol documents itself in its crate-root rustdoc. Vendor behavior: `docs/vendor/esp32s31/ieee802154-*.md`.
