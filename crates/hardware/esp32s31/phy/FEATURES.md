# ESP32-S31 shared PHY source capabilities

See the [whole-radio capability map](../driver/FEATURES.md) for shared ownership,
clock/power lifecycle and cross-protocol composition limits.

This document covers RF, analog, calibration and shared-PHY ownership below
protocol MAC/LL. Protocol modulation/rate support belongs to the
[Wi-Fi](../driver/ieee80211/FEATURES.md), [Bluetooth](../driver/bluetooth/FEATURES.md) and
[IEEE 802.15.4](../driver/ieee802154/FEATURES.md) inventories. CSI capture, CTE IQ
sample delivery and protocol antenna-selection policy remain there.
[Coexistence](../driver/coex/FEATURES.md) owns shared-RF arbitration coverage.

`IMPLEMENTED` means the exact PHY primitive has a complete source-owned
implementation for the stated scope. It does not imply that every protocol
composes that primitive or that RF performance has been qualified.

| Status | Meaning |
| --- | --- |
| IMPLEMENTED | The named bounded algorithm/transition and its stated execution boundary are source-owned. |
| PARTIAL | A subset or lower mechanism exists without the complete named lifecycle/composition. |
| FAIL-CLOSED | A typed boundary deliberately prevents the stated activation or reuse. |
| ABSENT | No complete source owner implements the named operation. |

Primitive rows describe the retained S31 calibration graphs and supported
inputs, not arbitrary RF configurations. Most graphs use explicit actions,
identity-bound completions and finite deadlines. The
[target port](src/target_port.rs) and [target executor](src/target_executor.rs)
complete MMIO, analog-I2C, PBus, measurement and delay edges; a pure transition
completion is not an observation of RF quality or even proof of PLL lock.

## RF and analog primitives

| PHY capability | Status | Source boundary |
| --- | --- | --- |
| Common cold registration | IMPLEMENTED | [Registration](src/calibration/registration.rs) and target runners execute the retained cold graph and issue target-bound registered state. Hardware lifetime after registration is separate. |
| RF/analog initialization prefix | IMPLEMENTED | [Cold graph](src/calibration/cold.rs) and [analog I2C graph](src/analog/i2c.rs) own ordered initialization with target completion. This is cold initialization, not resume from modem sleep. |
| Analog I2C access | IMPLEMENTED | Read/write, masked updates, command completion and RC-calibration operations are explicit bounded transitions; no vendor ABI callback is required. |
| PBus access and initialization | IMPLEMENTED | [PBus](src/analog/pbus.rs) owns command order, readiness and work-mode transitions. [Memory publication](src/analog/pbus/memory.rs) has a separate finite owner. |
| RFPLL frequency programming / initialization | IMPLEMENTED | [RFPLL](src/analog/rfpll.rs) owns frequency/SDM programming, calibration wait and capacitor search. Missed lock is outcome data and failed search has a typed failure; success of the outer call alone is not a lock claim. |
| Frequency-table synthesis/publication | IMPLEMENTED | [Frequency initialization](src/analog/frequency.rs) generates and publishes retained records incrementally. It does not keep a vendor-owned runtime table. |
| Wi-Fi channel retune | IMPLEMENTED | [Channel transition](src/channel.rs) and target HAL entry points own the supported Wi-Fi channel domain, temperature-dependent work and bounded frequency-ready observations; MAC restart has a composed variant. This is not a generic BT/IEEE channel API. |
| Crystal-duty calibration | IMPLEMENTED | [Crystal search](src/analog/crystal_duty.rs) composes RFPLL, RX-DC and signal-power operations for the retained cold profile. Preparation, search and restoration failures terminate either frequency pass and propagate through the RF parent into registration cleanup. |
| D-code calibration | IMPLEMENTED | [D-code transition](src/analog/dcode.rs) owns the retained frequency visits, CKGEN updates and calibration results. |
| Temperature read/conversion | IMPLEMENTED | [Temperature transition](src/analog/temperature.rs) owns sampling and conditional range changes. Invalid codes fail closed; the cold reset-code case is handled explicitly. |

## RX and TX calibration primitives

| PHY capability | Status | Source boundary |
| --- | --- | --- |
| DC/IQ estimator | IMPLEMENTED | [Estimator](src/calibration/estimator.rs) exposes readiness, result extraction and cleanup as caller-completed actions. |
| RX DC-offset calibration | IMPLEMENTED | [RX-DC](src/rx/dc_offset.rs) owns the retained finite calibration loop and nested estimator graph. |
| RX gain table initialization | IMPLEMENTED | [RX gain](src/rx/gain.rs) generates/publishes gain memory from copied calibration outcomes; it does not substitute for RX-DC or RX-IQ calibration. |
| RX gain calibration | IMPLEMENTED | [Gain calibration](src/rx/gain_calibration.rs) owns recovered DC/IQ/RFPLL calibration steps with explicit hardware completions. |
| RX IQ calibration/correction setup | IMPLEMENTED | [RX IQ](src/rx/iq.rs) owns the retained receive-IQ calibration graph, estimator deadlines and cleanup. It does not claim sensitivity or correction quality for every protocol. |
| RX saturation check | IMPLEMENTED | [Saturation transition](src/rx/saturation.rs) owns the bounded PBus/sample sequence. It is not an autonomous lifetime saturation-recovery policy. |
| Receive signal-power measurement | IMPLEMENTED | [Signal power](src/rx/signal_power.rs) owns estimator sequencing and derived power results; absolute RF measurement accuracy is not claimed. |
| TX DC-offset calibration | IMPLEMENTED | [TX-DC](src/tx/dc_offset.rs) owns retained calibration results, comparator operations and restoration for the stated calibration inputs. |
| TX IQ calibration | IMPLEMENTED | [TX IQ](src/tx/iq.rs) composes finite cover/search, SAR, RFPLL, PBus and I2C transitions. |
| Power-detector reference calibration | IMPLEMENTED | [Detector](src/tx/power_detector.rs) owns reference-code measurements and restoration with bounded readiness sampling. |
| TX DC power-detector calibration | IMPLEMENTED | [DC detector](src/tx/dc_power_detector.rs) owns the finite search graph and external measurement edges. |
| TX calibration measurement primitives | IMPLEMENTED | [Shared TX calibration](src/tx/calibration.rs) owns attenuation search, tone/SAR operations and calibration-environment transitions. |
| TX power-control calibration | IMPLEMENTED | [Power calibration](src/tx/power.rs) owns the retained calibration/search graph. Complete regulatory or measured radiated-power equivalence is not implied. |
| Wi-Fi target-power profile/ceiling | IMPLEMENTED | The power profile resolves per-rate codes and applies a configured ceiling. Codes are gain-table indices, not measured dBm; this is not the IEEE 802.15.4 calibrated provider. |
| Bluetooth calibration state/table primitives | IMPLEMENTED | [Bluetooth calibration](src/calibration/bluetooth.rs) retains explicitly named gain conversions and calibration/publication operations. It does not establish a Bluetooth LL or connected RF lifecycle. |

## Calibration state and tracking

| PHY capability | Status | Source boundary |
| --- | --- | --- |
| Common / Wi-Fi / Bluetooth calibration state | IMPLEMENTED | [State](src/state.rs) holds semantic calibration/configuration values rather than a vendor parameter-memory image. |
| Calibration snapshot | IMPLEMENTED | Typed snapshot carries schema, identity and retained results. Snapshot construction is not hardware restoration. |
| Calibration cache representation/export | IMPLEMENTED | `PhyCalibrationCache` owns a snapshot and validates its schema/identity. Persistence/storage belongs to the caller. |
| Full initial calibration | IMPLEMENTED | All currently admitted registration paths execute full calibration. Output reports the selected path and can return a fresh cache. |
| Cache-backed cold restore / partial cold calibration | FAIL-CLOSED | Registration does not own complete hardware replay after reset. Any supplied cache selects `FullAfterRejectedCache` and full calibration; restoring software flags cannot skip hardware work. |
| Runtime parameter-tracking invocation | IMPLEMENTED | [Parameter graph](src/tracking/parameters.rs), bounded executor and target runners complete selected RFPLL, calibration, power, I2C and temperature children while retaining the client owner. Periodic scheduling is separate. |
| Runtime calibration-tracking invocation | IMPLEMENTED | [Calibration tracking](src/tracking/calibration.rs) evaluates common, Wi-Fi and Bluetooth/IEEE temperature references and owns selected child order plus quiesce/restore operations. This is not cold-cache replay. |
| TX power tracking | IMPLEMENTED | [Power tracking](src/tracking/power.rs) provides the selected parameter-tracking child with target execution. |
| Wi-Fi I2C parameter tracking | IMPLEMENTED | [I2C tracking](src/tracking/i2c.rs) owns the Wi-Fi-specific child; its existence does not create periodic service. |
| Temperature-triggered compensation | IMPLEMENTED | Tracking uses typed measurements, retained references and threshold decisions for the selected calibration graphs. No arbitrary environmental compensation or RF performance guarantee is implied. |
| Read-only tracking demand | IMPLEMENTED | [Schedule](src/tracking/schedule.rs) reports inactive, absolute due time or outstanding demand without advancing source timestamps. [Tracking contracts](src/tracking/README.md) separate demand, execution and physical admission. A demand is not an RF grant. |
| Registered-policy inspection | IMPLEMENTED | `RegisteredPhyRadio::inspect_tracking` and `RegisteredWifiPhy::inspect_tracking` borrow the existing owner and reuse source predicates. Evaluation deadline and retained thermal conditions are separate; temperature age, independent job admission and RF safety are not inferred. |
| Periodic tracking service | PARTIAL | [Client state](src/state/client.rs) evaluates tracking deadlines and target runners can execute requests. Wi-Fi exposes due-only `WifiStopped::maintain_phy` with an owned HAL `WifiAccess` retaining registers and inactive IRQ setup across execution; admission and release check stopped MAC and RX walker; the supervisor invokes it at elapsed deadlines between logical roles after bounded MAC stop, RX-ring halt and IRQ quiesce. Maintenance restores the prior live RX/IRQ handoff without resuming MAC; the following role applies its policy and resumes MAC. Continuous active-role periodic maintenance is not yet connected. Complete long-running scheduling, cancellation and teardown are not composed for every protocol. |

## Protocol consumer composition

Statuses here apply to protocol composition, independently of primitive status.
`PARTIAL` can mean an available target entry point without a public operational
caller. Shared registration does not prove that every calibration branch is
selected, every channel is usable or all radio clients can run concurrently.

| PHY operation | Wi-Fi | Bluetooth | IEEE 802.15.4 |
| --- | --- | --- | --- |
| Cold registration/calibration entry | IMPLEMENTED: production cold start invokes the target runner | IMPLEMENTED: common-PHY power/calibration-clock readback precedes the borrowed target runner, retaining the I2C clock lease in the task owner | PARTIAL: typed target registration route exists; whole-radio service is incomplete |
| Protocol-client ownership | IMPLEMENTED: cold start acquires Wi-Fi and retains the registered radio through MAC startup; runtime retains `RegisteredWifiPhy` and the acquired client scheduler | IMPLEMENTED: acquisition and initial tracking settle before BTBB entry | PARTIAL: registered client/foundation/timing wrappers exist without complete operational ownership |
| Initial tracking before client handoff | IMPLEMENTED: cold start evaluates the monotonic acquisition timestamp and settles any required tracking before initial channel selection and MAC entry | IMPLEMENTED: PHY setup drives the initial request to a terminal result | PARTIAL: target runner and pending/settled states exist without a complete public service |
| Protocol channel switching | IMPLEMENTED: channel HAL and MAC-restart entry points are composed | PARTIAL: DTM/event-specific frequency inputs exist, not a general shared retune lifetime | PARTIAL: MAC channel policy and BTBB timing exist; RF retune readiness remains incomplete |
| Periodic parameter/calibration tracking | PARTIAL: runner and deadline model exist; no complete periodic service is established by cold start | PARTIAL: initial tracking exists; periodic Controller maintenance remains incomplete | PARTIAL: target tracking entry exists; RF/runtime service remains incomplete |
| Operational power selection | IMPLEMENTED: configured Wi-Fi ceiling reaches per-rate MAC codes | PARTIAL: default event power encoding, not general live power control | PARTIAL: provider-index resolution lacks calibrated provider/MMIO composition |
| Resume after RF sleep | ABSENT | ABSENT | ABSENT |
| Complete last-client RF/analog shutdown | ABSENT | ABSENT | ABSENT |

The relevant callers are [Wi-Fi cold start](../driver/ieee80211/src/cold_start.rs),
[Bluetooth PHY setup](../driver/bluetooth/src/phy.rs),
[registered shared radio](src/registered_radio.rs),
[registered Bluetooth](src/registered_bluetooth.rs) and
[registered IEEE 802.15.4](src/registered_ieee802154.rs).
The IEEE timing-ready state explicitly excludes full RF readiness, shared PLL
refcount ownership, IRQ routing, DMA ownership and operational MAC service.

## Lifecycle boundaries

| PHY capability | Status | Current source boundary |
| --- | --- | --- |
| Client acquire/release bookkeeping | IMPLEMENTED | Client state rejects duplicate acquisition/invalid release, retains pending tracking and records last-client disposition. This is state ownership, not physical power release. |
| Target-bound registration/client proof | IMPLEMENTED | Registered wrappers couple state/proof to the hardware epoch; target runners produce the completion authority. A model-only result is insufficient. |
| Cold RF activation | PARTIAL | Registration and the Wi-Fi cold-power path compose initialization; protocol-wide reusable wake semantics are not supplied by that cold path. |
| RF wake from retained sleep | ABSENT | No complete shared sleep/resume transaction restores RF/PHY/baseband and retained calibration for all clients. |
| RF sleep/power-down | ABSENT | No complete operational shared-PHY sleep transaction exists. Calibration quiesce/restore is a temporary algorithm step, not modem sleep. |
| Stop tracking / release last client | PARTIAL | Model release disarms tracking state on the last client. Physical timer stop, final PHY-client release and analog shutdown are not one completed lifetime. |
| RF and analog shutdown | ABSENT | No complete last-owner hardware shutdown/reconstruction transaction is composed. |
| Re-registration after owned shutdown | PARTIAL | Fresh cold registration exists, but an owned shutdown-to-cold-restart cycle remains incomplete. |
| Tracking failure containment | FAIL-CLOSED | Failed target tracking consumes the unique request into a poisoned epoch. Target futures must reach a terminal result; cancellation requires out-of-band hardware reset rather than reuse of partial state. |

## Qualification boundary

There is no standalone PHY qualification target. Current readiness requirements
are carried by [Wi-Fi](../../../../qualification/targets/esp32s31/wifi-sta.toml),
[Bluetooth](../../../../qualification/targets/esp32s31/bluetooth-le.toml) and
[IEEE 802.15.4](../../../../qualification/targets/esp32s31/ieee802154.toml).
This inventory does not promote their proof states.

PLL lock, channel correctness, calibrated TX power, RX sensitivity after retune,
and calibration stability across sleep/wake require their own hardware evidence.
A completed source transition or protocol cold start cannot substitute for those
measurements. Cache restore and complete shutdown remain explicit unsupported
boundaries even when individual calibration algorithms are implemented.
