# IEEE 802.11 lower-MAC port

`oer-ieee80211-lower-mac` declares `Ieee80211LowerMacPort`, the radio port
between portable Wi-Fi MAC logic and the backend that puts frames on the air,
and its extension traits. It follows the five-part port shape of
[the architecture](../../../../docs/architecture.md#radio-ports) and is
sans-IO: it declares the port and its values and never waits on it.

## Contract

One submission is one hardware transmission attempt. The backend counts down
the caller's backoff, sends the PPDU once at the submitted rate, waits for the
solicited response and reports one completion. Retries, rate fallback, the
backoff draw, sequence and packet numbers and reordering stay above the port
unless the backend reports them in its `HardwareServices`.

| Part | Items |
| --- | --- |
| Submission | `tx_buffer(len)` lends a `TxBuffer` for an MPDU of `len` octets (`Ok(None)` when none is free, `Err` when the port cannot serve); the caller writes the MPDU up to its body and submits `TxAttempt<TxPayload<TxBuffer, TxBody>>`: caller `TxId`, `VifId`, `WmmAccessCategory`, the buffer, the body by ownership (`TxBody`, the composition's type, such as a network frame whose payload the caller does not copy) and the `TxResponse`, `PhyRate`, `Protection` (none, RTS/CTS, CTS-to-self), `KeySelector`, `TxPower`, `Backoff` and the attempt's `CoexPriority`. A refusal is `Refused { error: SubmitError, attempt }`: nothing is sent and the attempt comes back with its buffer and body. An unsubmitted buffer goes back through `release_tx_buffer`. The backend holds an admitted attempt's bodies until `reclaim_tx_bodies(id, each)` hands them back by subframe once the attempt ended (its completion reported, or a cancellation proved it over), so an event lost to a queue overflow loses no body and a retransmission sends the same owners; whether a backend sends a body from its owner's memory or copies it is its own choice |
| Events | `next_event` yields owned events viewed as `LowerMacEvent`: `Received { frame, RxMeta }`, whose frame `into_received` takes out as the backend's `RxBuffer` (dropping it returns the memory, so a consumer that keeps a frame longer than one step copies it), `TxCompleted(TxCompletion)` with `TxStatus`, ACK RSSI and SNR and the `BlockAckReport` (starting sequence, bitmap) of a BlockAckReq or A-MPDU, lifecycle terminals, `RxTooLong { length }` for a received MPDU longer than the backend's receive buffer, `Extension` for an event an extension trait views, and the terminal `Poisoned`. A loss is reported once as `EventsLost`, in place of the first dropped event |
| Controls | `apply(LowerMacSetting)`: `Channel`, interface configuration (`VifConfig`: address, `VifRole`, BSSID, `ReceiveFilter`, whose `PROBE_REQUESTS` rule admits the Probe Requests an access point answers, wildcard ones included), key removal, receive Block Ack agreements, the `Edca` parameter set, the global `TxGate`, the beacon receive priority (`RxBeaconPriority`) and an interface's HE BSS color (`HeBssColor`). `install_key` returns the `KeyHandle` attempts select |
| Capabilities | `LowerMacCapabilities`, the parametric limits: bands, widths, rates, `HardwareServices`, interfaces, transmit queues, longest MPDU, largest backoff, lowest power ceiling, coexistence levels, the PPDU formats of unicast no-ACK frames, each role's receive rules, key slots and receive Block Ack limits |
| Lifecycle | `lifecycle(Enable / Disable / Quiesce)`, each with a terminal `LifecycleEvent`, and `cancel(TxId)`, whose terminal event is the attempt's completion; a failed command ends with `LifecycleEvent::Failed { command, class: FailureClass }` |
| Clock | `now()` is the `Ieee80211Instant` (`oer_time::RadioInstant` of the port's `Ieee80211Radio` domain) of receive timestamps; `clock_info()` states its resolution and `RadioEpoch` and converts to monotonic time; `clock_sample()` reads both clocks back to back in the current generation, and `RxMeta::timestamp` is an `Ieee80211Stamp` that converts with a sample of its generation |

Every call but `next_event` is synchronous. The failure classes, `EventsLost`,
`Poisoned`, the lifecycle vocabulary, `CancelError`, the `Correlation` trait
`TxId` implements and `ClockInfo` are the shared ones of
[`oer-radio-port`](../../../radio/port/README.md), re-exported here.
Failures are `Rejected` (the inner `Err` of a call, or a `PortError` of that
class, such as a backend that is not installed), `Recoverable`
(`TxStatus::Aborted`, `TxStatus::Fault` or a `Recoverable` lifecycle
failure) or `Poisoned` (`LowerMacEvent::Poisoned` after every earlier event,
and an error of that class from every later call).

Rules a caller relies on:

- **Events.** The port has exactly one consumer of `next_event`. Several
  exchanges share it through `oer-ieee80211-upper-mac-service`'s
  `EventRouter`. Taking an event only dequeues it; a backend's timed work
  runs in its own runner, which the composition polls beside the consumer.
- **Loss.** After `EventsLost`, an attempt whose completion has not arrived
  is recovered by `cancel(id)`: an admitted cancel produces its completion,
  a refusal as `CancelError::NotRunning` proves that it ended.
- **Queues.** Each of `tx_queues` transmit queues holds one attempt in flight;
  another attempt for the same queue is `Busy`. With four queues, queue `n`
  serves the access category of ACI `n`; with one, every category shares it
  (`LowerMacCapabilities::tx_queue`). Completions correlate by `TxId`.
- **Buffers.** A submitted buffer is released when its completion is
  reported; the completion does not return it, so an event lost to an
  overflow loses no buffer. A retry re-encodes into a fresh buffer.
- **Backoff.** `Backoff::Slots(n)` is counted down as given, up to
  `max_backoff_slots`. `Backoff::HardwareDraw { cw_exponent }` is valid only
  when the backend reports `HardwareServices::BACKOFF_DRAW`. The portable
  draw is `oer-ieee80211-softmac`'s `EdcaContention`: CWmin doubled per failed
  attempt up to CWmax, reset when the frame ends, over an injected random
  source.
- **EDCA parameters.** `Edca(WmmParameterSet)` sets the AIFSN and TXOP
  limit each access category's queue contends with, and `CWmin`/`CWmax` for
  a backend that draws its own backoff; a backend starts with its defaults.
  The set applies whole: one record outside the backend's limits refuses it
  (`Unsupported`). A caller that draws the backoff above the port keeps the
  contention windows in its own planner, and keeps every aggregate and its
  BlockAck within its access category's TXOP limit (`PhyRate::max_ppdu_duration_micros`
  bounds a PPDU's time on air); a backend refuses no attempt by it.
- **Transmit gate.** `TxGate { open: false }` holds admitted attempts
  unpublished until it opens, for the station's own doze or a coexistence
  slice. A backend may refuse to close it while an attempt is published,
  because that attempt could end in a hardware timeout. Per-peer power-save
  buffering is software above the port.
- **Beacon receive priority.** `RxBeaconPriority` sets the coexistence
  priority the hardware receives beacons with: the station decides when
  (`BeaconWindow`, `Zero`, `Cleared`); the value of `BeaconWindow` is the
  radio system's priority of its beacon-window event, which only the
  backend sharing the RF with that system knows.
- **HE BSS color.** `HeBssColor { vif, color }` sets the color (0-63) an
  interface's HE PPDUs carry, from the access point's HE Operation and
  again whenever the BSS changes it.
- **Cancel.** `cancel(TxId)` guarantees the attempt's terminal event:
  `Aborted` for an attempt not yet published, and for a published one
  whatever it ends with, which may be its natural completion. Ending a
  published attempt on the air is `LowerMacCancelPublished`.
- **Receive filters.** A frame is delivered when a rule of any interface
  admits it, whatever superset the hardware passes; `VifConfig::admits` is
  the portable classification a backend narrows with. The `BSS_MEMBER` rules
  need a BSS, so a station without a BSSID can request only
  `OTHER_BSS_MANAGEMENT`.
- **Limits.** `SubmitError::Unsupported` and `SettingError::Unsupported`
  mean a value outside the declared limits and nothing else.

## Capability model

Structural optional features are extension traits over the base port. An
upper layer that needs one requires its trait bound; a backend that lacks the
feature does not implement it, so the feature cannot be requested.

| Extension | Operations |
| --- | --- |
| `LowerMacAmpdu` | `ampdu_buffer`, whose `push_mpdu(len, body)` takes each subframe's body by ownership and returns its octets before the body; `submit_ampdu(TxAttempt<AmpduPayload>)` answered by a BlockAck in the completion; `AmpduCapabilities` (subframes, PPDU formats, longest aggregate) |
| `LowerMacBeaconTiming` | `tsf`, `tsf_sample`, `set_tsf`, `set_tbtt(TbttSchedule)`, `stop_tbtt` with `TbttEvent`s viewed through `tbtt(event)`; `BeaconTimingCapabilities` state which roles each operation serves. TSF values are `VifTsf` (an instant of the `Ieee80211Tsf` domain with its interface); values of two interfaces do not combine (`TsfVifMismatch`). A `TsfSample` relates an interface's TSF to the radio clock (`TSF_DRIFT_PPM`, the sum of both timers' 802.11 bound) in a generation that changes when the TSF jumps; `TsfRelation` is the portable rule a backend's TSF writer applies to tell a jump from drift |
| `LowerMacMonitor` | `set_monitor`: every frame with a valid FCS is received; `MonitorCapabilities` state whether it runs beside receiving interfaces |
| `LowerMacCancelPublished` | `cancel_published(TxId)`: withdraw a published attempt from the air |
| `LowerMacAirReservation` | `submit_air_reservation(TxAttempt<AirReservation>)`: a CTS addressed to the interface itself whose Duration (at most `MAX_AIR_RESERVATION`, the NAV's 32 767 µs) keeps the stations that hear it silent; at a non-HT rate, with no key and no response. An owner that takes the radio off the channel reserves the air first |
| `LowerMacLiveRetune` | `retune_live(Channel)`: retune an enabled, TX-idle port without a lifecycle transition; success means reception is ready on that channel. Preserve both VIFs' TSF relations, the radio clock, interfaces, keys, BlockAck agreements, TX queue configuration and ownership, pending events, lent RX buffers, TX gate and absolute TBTT schedules. A disabled port or outstanding TX is `Busy`; settling latency requires qualification for the window budget |

Parametric limits of what a backend has are its capabilities, and a value
outside them is refused as `Unsupported`. Why a backend lacks a feature or a
value (hardware absent, glue not written, vendor knowledge not recovered,
policy decision pending) is recorded in its qualification catalog, not in
code.

The PHY and channel values come from
[`oer-ieee80211-mac`](../mac/src/phy.rs): `Channel` (2.4 GHz and 5 GHz with
20 MHz and 40 MHz geometry) and `PhyRate` (non-HT, HT and HE SU).
`RxEvidence` and `RxCryptoStatus` are the receive provenance values
`oer-ieee80211-softmac` re-exports as `MacRxEvidence` and
`MacRxCryptoStatus`, and softmac's `MacTxResult::HardwareFailure` carries a
`TxStatus`. `MacOperationOwnership::hardware_services` turns a softmac
service description into `HardwareServices`.

## Above the port

What a backend does not report in its `HardwareServices` is software above
the port, written once in portable packages for every backend:

| Work | Owner |
| --- | --- |
| Retry counters, limits and the Retry bit of an MPDU; the rate of each attempt through a `RateLadder` | `oer-ieee80211-upper-mac` `retry` |
| Which A-MPDU subframes the next attempt resends after a `BlockAckReport` (`BlockAckReport::acknowledges`), republication after a failed protection exchange, the BlockAckReq, individual retries, aging | `oer-ieee80211-upper-mac` `ampdu` |
| RTS/CTS or CTS-to-self and the control-frame rate of each PPDU | `oer-ieee80211-upper-mac` `protection` |
| One exchange as a sequence of `TxAttempt`s, and its statistics | `oer-ieee80211-upper-mac` `tx::TxPlanner`, driven over a port by `oer-ieee80211-upper-mac-service` |
| The backoff draw | `oer-ieee80211-softmac` `EdcaContention` |
| Transmit packet numbers and receive replay | `oer-ieee80211-mac` `ccmp` |
| Receive Block Ack reordering | `oer-ieee80211-mac` `block_ack::reorder` |

Limits, rate ladders and duration estimates in which implementations differ
are parameters of those algorithms; the Espressif values recovered from the
vendor stack are the family package `oer-espressif-ieee80211-policy`.

## Users of the port

| User | What it drives over the port |
| --- | --- |
| `oer-ieee80211-upper-mac-service` (`UpperMacTx`) | One frame exchange as a sequence of attempts of the transmit planner |
| `oer-ieee80211-sta-service` (`port`) | The whole station: scan, Open System and SAE joins, the WPA2 handshake with `install_key`, the connected data plane with receive Block Ack agreements, SA Query and disconnection, and its power manager over `LowerMacBeaconTiming` (which it requires), `TxGate` and `RxBeaconPriority`; `LowerMacMonitor` only for a scan whose station filters lack `OTHER_BSS_MANAGEMENT` |

The port has a single event consumer: `oer-ieee80211-upper-mac-service`'s
`EventRouter`, which every `UpperMacTx` and the station's `PortLink` read
from.

## Implementers

| Backend | Base port | `LowerMacAmpdu` | `LowerMacBeaconTiming` | `LowerMacMonitor` | `LowerMacCancelPublished` | `LowerMacAirReservation` | `LowerMacLiveRetune` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Host model (`model` feature, `LowerMacModel`) | Yes, four queues | Yes | Yes | Yes | Yes | Yes | Yes, with no admitted TX |
| ESP32-S31 (`Esp32s31LowerMac`) | Yes, four queues | HT and HE SU, when built with aggregate owners | Station TSF and TBTT; access-point TSF restart only | Yes, without receiving interfaces | No | No: whether its MAC sends a software Duration is open (#202) | No: enabled-port retuning with preserved state needs research and qualification (#218) |

The `model` module (built for the crate's tests and with the `model` feature)
implements the port and every extension with an in-memory backend to show
that they need no chip types. `LowerMacModel` keeps every admitted attempt in
flight until the test ends it (`complete`, `complete_with`), or ends each
attempt as soon as it is published with the outcomes queued by `respond`
(success, a given `BlockAckReport`, or a failure status); `submitted` records
what every admitted attempt carried; `queued_events`, `gate_open`,
`vif_config`, `channel` and `monitoring` read its state. Time enters as a
value: its radio clock (`now`) reads what the test last passed to
`set_now`, which never runs backwards. Packages that drive
the port test against it, as `oer-ieee80211-upper-mac-service` and
`oer-ieee80211-sta-service` do.

`ModelAir` joins several models as radios on one medium, so a test runs
real drivers against each other (an access point on one model, its stations
on others) instead of scripting one side. Each `step` carries every
published attempt (`published`): each other enabled radio on the sender's
primary channel receives its MPDUs as its interfaces' filters admit them,
and the attempt ends `Success` when it solicits no response (a group or
control frame) or a radio on the channel has an interface of its first
address, `AckTimeout` otherwise. The model ciphers nothing: a protected
frame arrives decrypted and verified when the receiver holds a pairwise key
of its transmitter (a group key for a group frame) and is not received
otherwise. An overheard CTS-to-self defers other radios' published attempts
until its Duration expires, independently of their interface receive
filters; a CTS addressed to one of their own interfaces does not reserve
their medium. The harness advances to each model's `nav_until()` as well as
the services' deadlines, converting that radio instant through its clock
sample. Receive metadata names the receiver's configured channel. Air time,
contention, loss and reception levels are not modelled.

The ESP32-S31 MAC converts its chip values in
[`mac/src/portable.rs`](../../../hardware/esp32s31/driver/ieee80211/mac/src/portable.rs):
`TxPhyRate` to and from `PhyRate`, the receive prefix `RxPhyInfo` to
`PhyRate` and `RxMeta`, and a completion to `TxStatus`. The ESP32-S31
implements the port in two layers:

- [`LowerMacCore`](../../../hardware/esp32s31/driver/ieee80211/src/lower_mac.rs)
  in `oer-esp32s31-ieee80211` is sans-IO and generic over the register seams
  (`LowerMacHardware`), so host tests drive it through the seams' test
  doubles.
- [`Esp32s31LowerMac`](../../../runtime/esp32s31/ieee80211/src/lower_mac.rs)
  in `oer-esp32s31-ieee80211-runtime` implements `Ieee80211LowerMacPort`,
  `LowerMacAmpdu` (for a core built with aggregate owners),
  `LowerMacBeaconTiming` and `LowerMacMonitor` over the core: one bounded
  queue per kind of event (completions and lifecycle terminals that cannot
  overflow, because the port refuses work whose terminal event would not
  fit; TBTTs; received frames with their own loss report), the MAC, power
  and receive interrupt entries, and the runner `Esp32s31LowerMac::run` with
  the publication watchdog and the PHY retune of `Enable`. A poisoned port
  parks its runner until a new install. Its body type is the composition's
  `O`; the core sends from its own DMA memory, so the port copies each body
  after its header into the lent slot or subframe as it is submitted or
  pushed (the core's own attempts take no body, `Infallible`), and holds the
  owners, one table entry per attempt within the owed completions, until
  they are reclaimed or an uninstall drops them. Received units reach it from the
  DMA receive transaction through `Esp32s31PortRxPublisher`, which
  classifies them as the direct path does (`IngressClass::of`: protected
  data bulk, every other frame critical, so the transaction's credit reserve
  keeps the last staging credits for management, control and EAPOL) and
  hands back a unit the received queue has no room for
  (`try_on_received`), so the transaction keeps it rather than losing it.

The ESP32-S31 station and access-point roles do not use the port yet. Its
operations map onto the S31 seams as follows:

| Port operation | ESP32-S31 implementation |
| --- | --- |
| `tx_buffer`, `TxBuffer` | A whole ordinary TX slot (`TxSlot`, pinned descriptor and DMA buffer); the MPDU is written after the metadata word. The attempt publishes that slot, so the frame is published where it was written, and the slot is lent again after the completion. `TX_BUFFERS` spare slots are lent |
| `submit` of one MPDU | `OrdinaryTxOwner::start_queued_single_attempt` in `src/ordinary_tx.rs`: one `TxHardware` publication at the submitted rate with the caller's protection, backoff (the queue's ten-bit contention-window field) and power ceiling; the owner's retry ladder, rate fallback, backoff draw and BSS protection selection are not applied |
| Queues | The four ordinary EDCA queues (voice, video, best effort, background), each with its own descriptor, completion bank and latched completion, timeout and collision state; one attempt per queue. Every MAC interrupt edge is offered to every published queue, which claims only its own state. A timeout abort forces the MAC-wide CCA for 16 µs, so aborts run one at a time and a queue timing out meanwhile aborts when that settle ends |
| `LowerMacAmpdu` | One `RetainedDmaAmpduTx` per lent aggregate, in single-attempt mode: `begin`, `commit_ht` (or `commit_he`) of every subframe, one `submit` (or `submit_he`); never `retain_for_ampdu_retry`. Subframes are `StableDmaBacking` leases of the integrator's `AmpduBackingSource` (as the station and access-point aggregate paths publish from), released to it on completion. HT and HE SU, at most the owner's slots (32 for HE) and 6490 octets (the vendor's MCS 0 ceiling, `rx11NRate2AMPDULimit`); `min_mpdu_start_spacing` selects the queue's `HtProtectionSpacing`; an HT publication follows `ht_ampdu_publication_config`; an HE one is an `HeAmpduTxConfig` carrying its access category's TXOP limit (`he_length_budget`, the empty delimiters of the recipient density) with the station's BSS color; both with the caller's protection, backoff and power ceiling. The completion's `BlockAckReport` is the BlockAck the hardware marks received (`HtAmpduTxCompletion::valid_block_ack`); without one the attempt failed |
| `Backoff` | `Slots` up to 1023; `HardwareDraw` is refused: the S31 draws in software and the hardware only counts down |
| `TxPower::MaxDbm` | The smaller of the calibrated pair and the ceiling, for the data and the RTS/CTS control frame, down to 0 dBm. S31 power codes follow the vendor's quarter-dBm target shifted right by two (`phy/src/tx/power.rs`), not a measured radiated power |
| `coex` | `Normal`: the static per-access-category priority of vendor data encapsulation (events 10-13 through `coex_pti_tab`). `Elevated`, for a single MPDU: a connection frame under the radio system's reconnect policy, with event 46's priority as the packet priority, the lesser of it and the slice event's as the scheduler priority, and a priority count of 4000 (`RadioCoexPriorities::connection_frame_priorities`). `Idle` and `Critical`, and `Elevated` aggregates, are refused: no reviewed event selects them |
| `TxResponse::None` to a unicast receiver | Legacy rates only, the one program with a response field (PLCP format zero); on-air behaviour is not yet confirmed on hardware. HT and HE are refused |
| `TxCompleted` | `take_tx_completion` through `TxCompletion::tx_status`; a collision detach is `Collision`, the hardware timeout abort `Aborted`; `ack_snr_db` from `ack_snr_sample`, no ACK RSSI |
| `TxResponse::BlockAck` | A BlockAckReq at a non-HT rate; `take_block_ack_completion` gives the `BlockAckReport` |
| `Received`, `RxMeta`, `RxBuffer` | `Esp32s31LowerMac::on_received` takes one unit of the receive producer (`LowerMacRxUnit`; on the target `Esp32s31StagedRx`, a staging-pool DMA buffer with its ring's ingress configuration), reads its `NormalizedRxFrame` in place, narrowed by `VifConfig::admits`, and `portable::rx_meta` on the configured channel. The `Esp32s31RxBuffer` is that unit with the MPDU's range (`NormalizedRxFrame::mpdu_offset`): nothing is copied, and the DMA buffer returns when the consumer drops it. A unit the port does not queue returns at once; an MPDU longer than the pool's buffers never reaches the port, so it reports no `RxTooLong` |
| `Channel` | 2.4 GHz, 20 and 40 MHz, while disabled; `Enable` retunes through `LowerMacRetune`, and a refused retune ends as a `Recoverable` `Failed { Enable }` with the port disabled |
| `Vif`, `ReceiveFilter` | The smallest superset policy: station link policy six (`StaLinkRxPolicyHardware`) for the `BSS_MEMBER` rules, station ESP-NOW policy six mode two (`StaEspNowRxPolicyHardware`) when `OTHER_BSS_MANAGEMENT` is requested (the broadcast BSSID outside a BSS), access-point policy eight (`ApRxPolicyHardware`) for the `BSS_MEMBER` and `PROBE_REQUESTS` rules. Refused: other BSSs' management for the access point. One station and one access point; the station address is the one the cold start published |
| `Edca` | `WifiTxRuntimePolicy::install_wmm`: all four records validated before any is installed; every attempt's AIFSN comes from its queue's record |
| `TxGate` | `set_power_save_tx_block` on every ordinary queue; closing is refused (`Busy`) while an attempt is published, and an attempt admitted behind the closed gate is held and published when it opens |
| `HeBssColor` | `WifiTxRuntimePolicy::install_he_bss_color` for the station interface, which every HE PPDU then carries; the access point has no HE |
| `RxBeaconPriority` | `hal_set_rx_beacon_pti` with the beacon-window priority of the core's `RadioCoexPriorities` (the runtime's `RadioCoex` over the radio system's coexistence view), or zero, as both priorities; `hal_clear_rx_beacon_pti` for `Cleared` |
| `install_key`, `RemoveKey` | CCMP-128 through `CcmpKeyHardware`: the station's pairwise and group slots, the access point's pairwise slots (lowest free association slot) and group slot |
| `AddRxBlockAck`, `RemoveRxBlockAck` | The eight ordinary banks of `RxBlockAckHardware` |
| `LowerMacBeaconTiming` | The station TSF through its single writer `StationTsf` (`station_tsf.rs`, over `StationTsfHardware`) and TBTT schedule (`StationTbttHardware`, the lead as the TBTT ahead time and the vendor's 1.5 ms wake window above it); `Esp32s31LowerMac::on_power_interrupt` reports the TBTT a `MacPowerWakeCause::StaTbtt` edge announces. The access-point TSF only restarts from zero through its single writer `AccessPointTsf` (`ap_tsf.rs`, over `ApTsfHardware`'s reset and stop). Each interface's TSF relation starts a new generation at a channel change, an interface configuration or removal, an access-point restart and a station set beyond the drift since its last sample plus that sample's microsecond; `tsf_sample` reads the TSF between two MAC-clock readings of one generation |
| `LowerMacMonitor` | The open promiscuous policy (`MacSnifferHardware`), only while no interface has a receive filter |
| `HardwareServices` | The hardware-owned operations of `ESP32S31_MAC_SERVICE_CAPABILITIES` in `mac/src/capabilities.rs` |

A published attempt cannot be withdrawn: the S31 abort path needs the
queue's hardware timeout edge, so `cancel` ends a held attempt as `Aborted`
and a published one with its own completion. What the ESP32-S31 lacks and
why (Trigger-based A-MPDU, the access-point TBTT schedule, the
access-point TSF read and arbitrary set, on-air cancel, unicast no-ACK at HT
and HE rates, coexistence levels other than `Normal`, 5 GHz) is recorded in
the
[Wi-Fi/PHY catalog](../../../../qualification/catalog/esp32s31/wifi-phy.toml).
