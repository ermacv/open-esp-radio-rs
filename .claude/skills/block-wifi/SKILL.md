---
name: block-wifi
description: Use when a task changes or investigates the IEEE 802.11 (Wi-Fi) block of this repository — 802.11 protocols, the lower-MAC port, upper MAC, station or access-point services and roles, RSN, the ESP32-S31 Wi-Fi driver, runtime or composition, or Wi-Fi HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# Wi-Fi block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports).

## Packages (path → role)

- Protocols, portable sans-IO: `crates/protocols/ieee80211/` — `mac` (frames, scan, fragmentation), `lower-mac` (the port), `upper-mac`, `softmac`, `sta`, `ap`, `datapath`, `security/rsn`, `trace`.
- Family policy: `crates/protocols/espressif/ieee80211/policy` (Espressif TX policy data).
- Services, portable: `crates/services/ieee80211/{sta,upper-mac,rsn}`; runtime glue `crates/runtime/ieee80211`.
- ESP32-S31: driver `crates/hardware/esp32s31/driver/ieee80211/{mac,dma}`, roles `crates/roles/esp32s31/ieee80211/{sta,ap}`, runtime `crates/runtime/esp32s31/ieee80211`, composition `crates/composition/esp32s31/embassy/ieee80211`, esp-hal adapter `crates/adapters/esp-hal/esp32s31/ieee80211`.
- Facade: `crates/oer` features `wifi`, `esp32s31-wifi`. Applications: `examples/esp32s31/{station,access-point,monitor}`. HIL: `hil/scenarios/ieee80211/`, workloads in `hil/host/family/ieee80211` (fixtures `-fixture`, air analysis `-evidence`).

## Port

`Ieee80211LowerMacPort` (`crates/protocols/ieee80211/lower-mac/src/port.rs:111`); optional operations are extension traits `LowerMacAmpdu`, `LowerMacBeaconTiming`, `LowerMacMonitor`, `LowerMacCancelPublished` (`extensions.rs`). The S31 backend is `Esp32s31LowerMac` (`crates/runtime/esp32s31/ieee80211/src/lower_mac.rs`). The S31 station roles use the portable `oer-ieee80211-sta-service` for scan, join and lifecycle.

## Shared contracts (change only with their owners)

`crates/radio/port` (`ClockInfo`, `ClockSample`, failure classes), `crates/time` (`Instant`, `RadioInstant`), `crates/radio/coex`, the shared radio system `crates/runtime/esp32s31/radio` and `crates/composition/esp32s31/embassy/radio`, the HAL split `crates/hardware/esp32s31/hal/src/root.rs`, PAC and `registers/`.

## Traps

- Time follows [clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

- Every port method but `next_event` is synchronous (`port.rs:74`).
- `RxEvidence` keeps provenance: never derive a value and report it `HardwareObserved` (`lower-mac/src/rx.rs:13`). `RxMeta.channel` is configuration, not a per-frame observation (`rx.rs:72`).
- The S31 port's radio clock is the MAC local time, `Affine` to the monotonic clock (`MAC_CLOCK_INFO`, `runtime/esp32s31/ieee80211/src/mac_clock.rs`): convert a port instant or stamp with `clock_sample()`, never by reading its microseconds as monotonic time.
- A received frame carries `RxTimes { handoff, stamp }` (`hardware/esp32s31/driver/ieee80211/mac/src/rx/pool.rs`): the monotonic executor handoff and the raw MAC local-time receive stamp. The RX path never reads a clock; a consumer that needs the reception time converts the stamp: the station's beacon TSF follow in MAC time (`connected_control.rs`, `mac_local_time() - stamp`), the scan through a `MacClockSnapshot` (`runtime/esp32s31/ieee80211/src/mac_clock.rs`). HE Trigger/NDPA deadlines count from the handoff.
- The S31 station TSF has one writer, `StationTsf` (`hardware/esp32s31/driver/ieee80211/src/station_tsf.rs`): the hardware write takes a `StationTsfWrite` only the owner constructs, and the owner records every write in the portable `TsfRelation` (`lower-mac/src/extensions.rs`), whose generation changes at a jump, not at a beacon follow's drift. The port core and the role's connected control (one per association) each hold one, created with an epoch from `MacClockHandle::tsf_epoch`, so generations never repeat across reconnections or radio starts; never add a write around it. The access-point TSF has the same shape: `AccessPointTsf` (`hardware/esp32s31/driver/ieee80211/mac/src/ap_tsf.rs`) holds the `AccessPointTsfWrite` for `ApTsfHardware`'s reset and stop, and each restart or stop is a new generation.
- The TSF is never modular: a set across 2^64, or one after the counter passed 2^64, is a jump (`TsfRelation::set`), and TWT planning refuses a window past 2^64 (`BeyondTsfRange`). A TWT wake plan carries the station TSF generation it was computed in (`StationTwtWakePlan`); a plan of another generation is stale and must never be armed. No hardware TWT wake exists yet (#85).
- The connected control (`ConnectedControl`, ~2.3 KB) is placed in a static slot (`runtime/esp32s31/ieee80211/src/roles/station/control_slot.rs`) and reached through a `PlacedControl` lease: configure it with `&mut` setters after placement, and never move it by value through services, teardown or faults, where each copy multiplies its size in the connected epoch's frames. Its `.psram.bss` cell is a `ZeroedStatic<MaybeUninit<_>>` written with `ConnectedControlSlot::new()` at the one-time claim: an empty slot is not all-zero bytes, and a zeroed region discards any other initializer ([statics in zeroed regions](../../../docs/architecture.md#statics-in-zeroed-regions)).
- The RX descriptor-to-buffer binding (`dma/src/rx_storage.rs`) stores each descriptor's offset to its buffer, so the arena's zero bytes are the identity binding the host `prepare_ring` path relies on; `dma_layout_rotated` writes a rotation.
- The station's power-save default is `SleepType::None` (`connected_control/power.rs:181`): RF sleeps only for coexistence, so ordinary HIL runs never exercise modem sleep.
- The TX completion and publication timeout is one composition constant, 250 ms (`composition/esp32s31/embassy/ieee80211/src/supervisor/mod.rs:227`).

## Checks

- `cargo test -p <package>`, then `cargo xtask check changed`.
- `cargo hil images check --class performance --type-check` for chip-side changes; `cargo xtask check network` for network-owner or manifest changes.
- HIL smoke: `cargo hil run icmp-latency tcp-bidirectional station-reconnect` (`hil-run` skill).

Skills: `protocol-change`, `driver-or-hardware-change`, `hil-run`, `vendor-evidence`, `qualification-entry` (catalog `qualification/catalog/esp32s31/wifi-phy.toml`), `push-and-ci`.

Package detail: the READMEs of [lower-mac](../../../crates/protocols/ieee80211/lower-mac/README.md), [upper-mac](../../../crates/protocols/ieee80211/upper-mac/README.md), [sta](../../../crates/protocols/ieee80211/sta/README.md), [station service](../../../crates/services/ieee80211/sta/README.md), [S31 station role](../../../crates/roles/esp32s31/ieee80211/sta/README.md), [S31 AP role](../../../crates/roles/esp32s31/ieee80211/ap/README.md) and [S31 composition](../../../crates/composition/esp32s31/embassy/ieee80211/README.md).
