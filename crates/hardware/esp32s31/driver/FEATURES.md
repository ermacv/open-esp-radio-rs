# ESP32-S31 radio subsystem source capabilities

This is the entry point for radio coverage and the shared ownership, clock and
power lifecycle. Detailed PHY algorithms and protocol capabilities have one
canonical inventory each, linked below. HAL and PAC are implementation and
ownership layers, not additional feature inventories.

The current source supports bounded single-protocol operation and substantial
shared primitives. It does not compose a complete multi-protocol lifetime:
concurrent owners, arbitration, sleep/wake, final shutdown and reconstruction
remain incomplete. Cold initialization alone does not establish those stages.

## Coverage index

| Domain | Inventory | Current qualification authority |
| --- | --- | --- |
| Whole radio | This document | No standalone radio target; protocol requirements below apply to their exact scopes |
| Shared PHY | [RF, analog and calibration](../phy/FEATURES.md) | No standalone PHY target; protocol targets contain shared-PHY requirements |
| Coexistence | [Arbitration and integration](coex/FEATURES.md) | No standalone coex target; Bluetooth includes an incomplete coexistence capability |
| Wi-Fi | [IEEE 802.11](ieee80211/FEATURES.md) | [wifi-sta.toml](../../../../qualification/targets/esp32s31/wifi-sta.toml) |
| Bluetooth | [LE and Classic](bluetooth/FEATURES.md) | [bluetooth-le.toml](../../../../qualification/targets/esp32s31/bluetooth-le.toml); no Classic qualification claim |
| IEEE 802.15.4 | [Radio/MAC](ieee802154/FEATURES.md) | [ieee802154.toml](../../../../qualification/targets/esp32s31/ieee802154.toml) |

`IMPLEMENTED` denotes a complete source path only for the exact row scope.
`PARTIAL` denotes a lower or narrower implementation with missing composition.
`FAIL-CLOSED` denotes an explicit guard against activation/reuse; `ABSENT`
denotes no production owner for the named operation. None is a hardware
qualification result. A register setter, host test or successful cold-start
transition cannot grant readiness to a different protocol or lifecycle stage.

## Exclusive ownership and client handoff

| Shared capability | Status | Owner and boundary |
| --- | --- | --- |
| Exclusive radio-root ownership | IMPLEMENTED | [HAL ownership](../hal/src/owner.rs) and PAC root leases prevent a second safe hardware claim. This is exclusivity, not concurrent radio sharing. |
| Inactive route release/reselection | IMPLEMENTED | Cold owners can return the protocol-neutral root and platform witness. Outstanding calibration restore state rejects release and retains the owner. This does not stop an active radio. |
| Shared RF/analog borrow | IMPLEMENTED | `PhyHal` / `SharedPhyHal` provide narrow access bounded by an owning protocol route. A borrow cannot acquire, release or recover its underlying PAC owner. |
| PHY client acquire/release state | PARTIAL | [PHY ownership](../phy/FEATURES.md#lifecycle-boundaries) has typed acquisition, tracking and release bookkeeping. The production supervisor quiesces an idle Wi-Fi frontier, checks stopped MAC/RX hardware, releases the client and drives the final-client RF-close/cold-release chain. Multi-protocol client release remains uncomposed. |
| Protocol-specific PHY handoff | PARTIAL | [PHY consumer matrix](../phy/FEATURES.md#protocol-consumer-composition) distinguishes Wi-Fi cold state, settled Bluetooth acquisition and incomplete IEEE timing/operational composition. Shared types do not make these paths equivalent. |
| Active protocol switch | ABSENT | No complete active-radio stop, RF/client release and transfer to another operational protocol is composed. Inactive root reselection is a narrower operation. |

## Cold power and clocks

| Shared capability | Status | Owner and boundary |
| --- | --- | --- |
| Cold modem/PHY power prerequisites | IMPLEMENTED | [HAL power sequence](../hal/src/power.rs) owns reset, PMU/ICG selection, bus/source clocks, shared clock maps and PHY/I2C prerequisites. Wi-Fi cold registers and IEEE task registers implement its backend. This does not assert that every protocol calls the same sequence. |
| Modem bus clock setup | IMPLEMENTED | The finite power sequence enables register access and validates semantic state before issuing the powered owner. |
| Shared modem clock map | IMPLEMENTED | The power sequence configures and checks the shared map. A cold sleep-state clock map is not a working sleep/wake policy. |
| Modem source-clock setup | IMPLEMENTED | Reviewed source selection is included in the cold transaction and checked in final readback. This is not live clock switching under multiple clients. |
| Baseband reset prerequisites | IMPLEMENTED | Reviewed cold reset pulses and released-state checks are source-owned. Complete runtime reset/quiescence across all radio clients is separate. |
| PHY calibration and analog-I2C clocks | IMPLEMENTED | Cold setup establishes calibration clocks, the I2C source and retained master clock before PHY registration. |
| Shared clock lifetime | PARTIAL | [Platform ownership](../../../adapters/esp-hal/esp32s31/radio/README.md) uses route-owned dependencies and narrow reservations. Production adapters do not compose a general concurrent-client clock/power lifetime. |
| Cold PHY registration | IMPLEMENTED | [PHY target runners](../phy/FEATURES.md) execute the bounded registration/calibration graph. Registration is not per-event RF readiness or proof of PLL lock. |

`power::execute_owned` performs its ordered writes, then samples platform,
modem and shared semantic observations and checks the required postconditions.
It does not read back every intermediate write. Failure retains the owner for
its supported retry path; a powered type is issued only after successful checks.
Protocol-specific MAC clocks, RF calibration and IRQ/DMA activation retain their
own owners and prerequisites.

## Active operation, power saving and shutdown

| Shared capability | Status | Owner and boundary |
| --- | --- | --- |
| Protocol active operation | PARTIAL | The linked protocol inventories define the supported subsets. Wi-Fi operation does not establish a reliable Bluetooth ACL link or a public IEEE 802.15.4 RF-ready service. |
| Shared PHY tracking | PARTIAL | PHY has bounded target tracking invocations and client/deadline models. Complete periodic scheduling and final tracking teardown are not composed across protocols. |
| Calibration state/cache | IMPLEMENTED | PHY owns schema- and identity-validated snapshots, cache export and cold partial replay. Cold release regenerates the cache from the final post-tracking state instead of returning the potentially stale cold-start snapshot. The cold graph restores semantic calibration products, rebuilds hardware-resident frequency/RX gain state and falls back to full calibration for invalid input. Retained RF wake keeps the same registered calibration epoch. |
| Shared RF powered-idle frontier | IMPLEMENTED | Final client release produces an affine owner proving an empty client set while retaining the physically powered radio and calibration state. Only this owner can enter the source-owned RF close transaction. |
| RF sleep / modem power-down | PARTIAL | PHY exposes exact RF close. The production Wi-Fi supervisor can either wake that closed RF domain while retaining registration, or continue through temperature-sensor shutdown, route-clock release, captured Wi-Fi clock/reset restoration and cold-owner reconstruction. These explicit idle transactions do not implement timed modem sleep or protocol power-save signaling. |
| RF wake / resume | PARTIAL | PHY implements a fail-stop retained RF wake transaction and the production Wi-Fi supervisor composes it from a fully stopped, final-client frontier through Wi-Fi client reacquisition and RX restoration. Bluetooth and IEEE 802.15.4 resume, scheduled sleep, cancellation policy above the owner actor and hardware qualification remain incomplete. |
| Full powered shutdown | PARTIAL | The Wi-Fi production supervisor composes runtime/IRQ quiescence, last-client release, exact RF/analog shutdown, route-owned clock/reset restoration and cold-owner return. Bluetooth, IEEE 802.15.4 and shared-protocol shutdown remain uncomposed. |
| Cold reconstruction after shutdown | IMPLEMENTED | The idle control path invokes the source-owned shutdown and `restart_esp32s31_radio`, which captures the final post-tracking cache and feeds it directly into validated cold registration. Repeated hardware lifecycle evidence covers this Wi-Fi boundary. |

Power saving has both shared and protocol-specific requirements. The shared
sleep/wake transaction cannot substitute for TIM/TWT, Bluetooth link timing,
IEEE receive windows or coexistence deadline policy. Conversely, implementing
those protocol policies does not establish shared RF shutdown or retention.
Children retain their precise statuses and link here for this common boundary.

## Concurrent ownership and arbitration

| Shared capability | Status | Owner and boundary |
| --- | --- | --- |
| Wi-Fi + Bluetooth concurrent composition | ABSENT | Current platform adapters independently own overlapping singleton types; the Bluetooth coordinator does not make a simultaneous Wi-Fi claim safe. |
| Wi-Fi + IEEE 802.15.4 concurrent composition | ABSENT | No complete joint platform ownership and RF arbitration lifecycle exists. |
| Bluetooth + IEEE 802.15.4 concurrent composition | ABSENT | No complete paired-radio owner and arbitration runtime exists. |
| Triple-radio composition | ABSENT | Shared primitives and separate protocol paths do not compose all three clients. |
| Coexistence hardware/core infrastructure | PARTIAL | [Coex inventory](coex/FEATURES.md) separates recovered tables/timers, validation-only HAL access and the absent live request/grant/release integration. |
| External RF arbitration boundary | PARTIAL | External coexistence transactions are investigated, but their platform-owned register/pin and activation lifetime is not published. This does not claim a generic antenna-switch API. |

## Evidence and implementation ownership

[HAL](../hal/src/owner.rs) defines semantic access and transitions. The
[PAC ownership contract](../pac/README.md) and
[reviewed register model](../../../../registers/esp32s31/model/device.toml) define
which hardware words those transitions may access. DMA, IRQ and Embassy
adapters provide implementation evidence for the capabilities above rather
than independent capability inventories.

[Qualification](../../../../qualification/README.md) is the readiness authority.
Its existing protocol manifests retain their current proof states; this map
creates no PHY, coex or whole-radio qualification target and makes no new HIL
claim. Complete concurrency and restart readiness require composed ownership
and hardware evidence, not the union of individually implemented primitives.
