---
name: block-ieee802154
description: Use when a task changes or investigates the IEEE 802.15.4 block of this repository — the portable 802.15.4 MAC and radio contracts, the Ieee802154RadioPort, the Espressif MAC engine ported from ESP-IDF, the radio role and runtime, the ESP32-S31 composition and interrupt route, the OpenThread adapter or Thread example, the 802.15.4 vendor host stand, or 802.15.4 and PHY-calibration HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# IEEE 802.15.4 block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

## Packages (path → role)

- Protocol, portable: `crates/protocols/ieee802154` (`oer-ieee802154`: MAC frames, CSMA, CSL, enhanced ACK, security, time sync, the radio state machine and the port). No portable service package.
- Family (Espressif, S31 and C5): engine `crates/hardware/espressif/ieee802154/engine` (`oer-espressif-ieee802154-engine`, the MAC driver ported from public ESP-IDF, `Ieee802154LowLevel`, host register model under feature `model`); shared MAC values `crates/hardware/ieee802154/pac`; trace points `crates/hardware/ieee802154/trace`; role `crates/roles/espressif/ieee802154/radio`; runtime `crates/runtime/espressif/ieee802154` (`Ieee802154Runtime`).
- ESP32-S31: PAC, HAL and PHY modules inside the chip crates (`hardware/esp32s31/{pac,hal}/src/ieee802154*`, `phy/src/ieee802154_client.rs`; `Ieee802154MacOwners` implements the engine's low level); composition `crates/composition/esp32s31/embassy/ieee802154` (`oer-esp32s31-ieee802154-system`, `Ieee802154System`); esp-hal interrupt route and time `crates/adapters/esp-hal/esp32s31/ieee802154`. `crates/hardware/esp32s31/driver/ieee802154/` holds only the capability page.
- OpenThread: `crates/adapters/openthread/ieee802154` (`OpenThreadRadio` over any port); example `examples/esp32s31/thread`.
- Facade: `crates/oer` features `ieee802154`, `esp32s31-ieee802154`, `embassy-ieee802154`, `openthread`. HIL: `hil/scenarios/ieee802154/`, workloads in `hil/host/family/ieee802154`, ESP32-C5 peers in `hil/peers/esp32c5-ieee802154` and `hil/peers/esp32c5-openthread`, agent `hil/targets/esp32s31/agent/src/product_hil/ieee802154/`.

## Port

`Ieee802154RadioPort` (`crates/protocols/ieee802154/src/port.rs`): every method but `next_event` is synchronous and takes the backend's lock only for its own duration. No extension traits: optional operations are `Ieee802154Capabilities`, settings outside the state machine go through `apply(RadioSetting)`, and a missing setting is refused `SettingError::Unsupported`. The role advertises `IEEE802154_RADIO_CAPABILITIES`. The backend is `Ieee802154Runtime` (`crates/runtime/espressif/ieee802154/src/lib.rs`): its `install` returns the port `Ieee802154Port` and the composition's `Ieee802154Control` (`port.rs`: pause and resume for shared-PHY maintenance, coexistence, statistics, the pending table, diagnostic hardware reads and the `uninstall` that consumes both). The S31 concrete type is `Ieee802154SystemRuntime` in a `static`; `start` returns the port (`Ieee802154SystemPort`) with the `Ieee802154System`, which keeps the control and gets the port back in `stop`. The interrupt entry, `run` and the clock stay on the runtime (`Ieee802154System::runtime()`).

## Shared contracts (change only with their owners)

`crates/radio/port` (`ClockInfo`, `EventsLost`, lifecycle, failure classes), `crates/time` (`Ieee802154Instant` = `RadioInstant<Ieee802154Radio>`, `Clock`, `Timer`), `crates/radio/coex` and `crates/hardware/espressif/coex`, the shared radio system `crates/runtime/esp32s31/radio` (`RadioGuard::join_ieee802154`) and `crates/composition/esp32s31/embassy/radio`, the PHY concurrent domain, the HAL split `crates/hardware/esp32s31/hal/src/root.rs` (`Ieee802154RadioPartition`), PAC and `registers/`.

## Traps

- The radio clock is the runtime's monotonic clock (`ClockInfo::MONOTONIC_MICROS`), unlike Wi-Fi's affine MAC time; the engine stamps each frame at its SFD with the platform microsecond clock. The esp-hal adapter's `now_micros` needs `esp_hal::init` first.
- The engine takes every deadline decision inside its ISR (`Ieee802154Engine::isr`: ACK and pending bit, enhanced ACK, ACK watchdog, next operation), as the vendor does; deferring them to a task cannot meet the deadlines ([driver port](../../../docs/vendor/esp32s31/ieee802154-driver-port.md)).
- CSMA-CA backoffs and retry delays are software, run by `Ieee802154System::run`, which the application polls beside the event consumer; `next_event` only takes events. Over OpenThread, `SubMac` runs CSMA-CA and retries itself.
- One event consumer. A full queue (16 events) drops the newest and reports `EventsLost` once in its place; the runtime never poisons: a broken sequence ends in a recoverable `RadioEvent::Fault` with the radio disabled.
- `Quiesced` holds the radio still: commands that write the MAC registers (operations, receive, sleep, addresses, PAN identifiers) and hardware settings are refused `Quiesced` until `Enabled`; PIB and pending-table configuration (`writes_registers` in the role) is admitted. Shared-PHY maintenance under the quiesced policy is this state too: the pause lends the hardware out and the consumer sees `Quiesced`/`Enabled`; a disabled radio gets neither event and keeps refusing as `Disabled`. The OpenThread adapter and the HIL session wait out the window and repeat the command. Under the default vendor policy tracking runs while receiving, so Thread HIL never exercises the pause.
- The runtime is a process singleton: modem source 132 has one handler and the MAC one set of owners, in one blocking mutex with the engine. Route order: activate the HAL interrupt owner, install the runtime, then `bind`; teardown reverses it. The handler lives in SRAM.
- The MAC DMA reaches internal SRAM only (`IEEE802154_DMA_WINDOW`): a frame in PSRAM is a DMA error and never reaches the air. `start` fails closed with `Ieee802154StartError::BuffersNotDmaVisible`. A frame received into the stub buffer is dropped.
- `EVENT_STATUS` is write-one-to-clear, sampled into an affine token no caller can forge or replay; a stale image fails closed before `ED_START`. A `STOP` write is not proof of DMA quiescence ([dataplane](../../../docs/vendor/esp32s31/ieee802154-dataplane.md), [control](../../../docs/vendor/esp32s31/ieee802154-control.md)).
- A failure before shared hardware work rolls back to the parked partition; a started PHY, clock or power transaction that fails stays fail-stop until reset.
- PHY: `maintain_phy` tracks while receiving under the vendor admission, and pauses, proves quiescence, tracks and resumes under the quiesced one; with another client active it waits. Joint quiesced tracking of several clients is not composed. RF stays open for the client's lifetime; there is no modem retention or light sleep with the pinned libphy.
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
