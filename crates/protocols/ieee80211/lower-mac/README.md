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
| Submission | `tx_buffer(len)` lends a `TxBuffer`; the caller encodes the MPDU into it and submits `TxAttempt<TxPayload<TxBuffer>>`: caller `TxId`, `VifId`, `WmmAccessCategory`, the buffer with its `TxResponse`, `PhyRate`, `Protection` (none, RTS/CTS, CTS-to-self), `KeySelector`, `TxPower`, `Backoff` and the attempt's `CoexPriority`. A refusal is `Refused { error: SubmitError, attempt }`: nothing is sent and the attempt comes back with its buffer. An unsubmitted buffer goes back through `release_tx_buffer` |
| Events | `next_event` yields owned events viewed as `LowerMacEvent`: `Received { frame, RxMeta }`, `TxCompleted(TxCompletion)` with `TxStatus`, ACK RSSI and SNR and the `BlockAckReport` (starting sequence, bitmap) of a BlockAckReq or A-MPDU, lifecycle terminals, and `Extension` for an event an extension trait views. Overflow is reported once as `EventsLost` |
| Controls | `apply(LowerMacSetting)`: `Channel`, interface configuration (`VifConfig`: address, `VifRole`, BSSID, `ReceiveFilter`), key removal, receive Block Ack agreements and the global `TxGate`. `install_key` returns the `KeyHandle` attempts select |
| Capabilities | `LowerMacCapabilities`, the parametric limits: bands, widths, rates, `HardwareServices`, interfaces, transmit queues, longest MPDU, largest backoff, lowest power ceiling, coexistence levels, the PPDU formats of unicast no-ACK frames, each role's receive rules, key slots and receive Block Ack limits |
| Lifecycle | `Enable`, `Disable`, `Quiesce` and `Cancel(TxId)`, each with a terminal event; a failed command ends with `LifecycleEvent::Failed { command, class: FailureClass }` |
| Clock | `now()` is the `oer_time::RadioInstant` of receive timestamps |

Every call but `next_event` is synchronous. Failures are `Rejected` (the inner
`Err` of a call), `Recoverable` (`TxStatus::Aborted`, `TxStatus::Fault` or a
`Recoverable` lifecycle failure) or `Poisoned` (`Err(Self::Error)`).
`FailureClass` lives in this package until a second port reports lifecycle
failures as events and a shared contract package takes it over.

Rules a caller relies on:

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
- **Transmit gate.** `TxGate { open: false }` holds admitted attempts
  unpublished until it opens, for the station's own doze or a coexistence
  slice. A backend may refuse to close it while an attempt is published,
  because that attempt could end in a hardware timeout. Per-peer power-save
  buffering is software above the port.
- **Cancel.** `Cancel(TxId)` guarantees the attempt's terminal event:
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
| `LowerMacAmpdu` | `ampdu_buffer`, `submit_ampdu(TxAttempt<AmpduPayload>)` answered by a BlockAck in the completion, `AmpduCapabilities` (subframes, PPDU formats, longest aggregate) |
| `LowerMacBeaconTiming` | `tsf`, `set_tsf`, `set_tbtt(TbttSchedule)` with `TbttEvent`s viewed through `tbtt(event)`; `BeaconTimingCapabilities` state which roles each operation serves |
| `LowerMacMonitor` | `set_monitor`: every frame with a valid FCS is received; `MonitorCapabilities` state whether it runs beside receiving interfaces |
| `LowerMacCancelPublished` | `cancel_published(TxId)`: withdraw a published attempt from the air |

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

## Implementers

| Backend | Base port | `LowerMacAmpdu` | `LowerMacBeaconTiming` | `LowerMacMonitor` | `LowerMacCancelPublished` |
| --- | --- | --- | --- | --- | --- |
| Host model (the crate's tests) | Yes, four queues | Yes | Yes | Yes | Yes |
| ESP32-S31 (`Esp32s31LowerMac`) | Yes, four queues | HT only, when built with aggregate owners | Station TSF and TBTT; access-point TSF restart only | Yes, without receiving interfaces | No |

The crate's tests implement the port and every extension with an in-memory
model to show that they need no chip types.

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
  `LowerMacBeaconTiming` and `LowerMacMonitor` over the core: a bounded
  owned-event queue, the MAC, power and receive interrupt entries, the
  publication watchdog and the PHY retune of `Enable`.

The station and access-point roles do not use the port yet. Its operations
map onto the S31 seams as follows:

| Port operation | ESP32-S31 implementation |
| --- | --- |
| `tx_buffer`, `TxBuffer` | A whole ordinary TX slot (`TxSlot`, pinned descriptor and DMA buffer); the MPDU is written after the metadata word. The attempt publishes that slot, so the frame is published where it was written, and the slot is lent again after the completion. `TX_BUFFERS` spare slots are lent |
| `submit` of one MPDU | `OrdinaryTxOwner::start_queued_single_attempt` in `src/ordinary_tx.rs`: one `TxHardware` publication at the submitted rate with the caller's protection, backoff (the queue's ten-bit contention-window field) and power ceiling; the owner's retry ladder, rate fallback, backoff draw and BSS protection selection are not applied |
| Queues | The four ordinary EDCA queues (voice, video, best effort, background), each with its own descriptor, completion bank and latched completion, timeout and collision state; one attempt per queue. Every MAC interrupt edge is offered to every published queue, which claims only its own state. A timeout abort forces the MAC-wide CCA for 16 µs, so aborts run one at a time and a queue timing out meanwhile aborts when that settle ends |
| `LowerMacAmpdu` | One `RetainedDmaAmpduTx` per lent aggregate, in single-attempt mode: `begin`, `commit_ht` of every subframe, one `submit`; never `retain_for_ampdu_retry`. Subframes are `StableDmaBacking` leases of the integrator's `AmpduBackingSource` (as the station and access-point aggregate paths publish from), released to it on completion. HT only, at most the owner's slots and 6490 octets (the vendor's MCS 0 ceiling, `rx11NRate2AMPDULimit`); `min_mpdu_start_spacing` selects the queue's `HtProtectionSpacing`; the publication follows `ht_ampdu_publication_config` with the caller's protection, backoff and power ceiling. The completion's `BlockAckReport` is the BlockAck the hardware marks received (`HtAmpduTxCompletion::valid_block_ack`); without one the attempt failed |
| `Backoff` | `Slots` up to 1023; `HardwareDraw` is refused: the S31 draws in software and the hardware only counts down |
| `TxPower::MaxDbm` | The smaller of the calibrated pair and the ceiling, for the data and the RTS/CTS control frame, down to 0 dBm. S31 power codes follow the vendor's quarter-dBm target shifted right by two (`phy/src/tx/power.rs`), not a measured radiated power |
| `coex` | `Normal` only: the static per-access-category priority of vendor data encapsulation (events 10-13 through `coex_pti_tab`). Other levels are refused; their event mapping is a pending policy decision |
| `TxResponse::None` to a unicast receiver | Legacy rates only, the one program with a response field (PLCP format zero); on-air behaviour is not yet confirmed on hardware. HT and HE are refused |
| `TxCompleted` | `take_tx_completion` through `TxCompletion::tx_status`; a collision detach is `Collision`, the hardware timeout abort `Aborted`; `ack_snr_db` from `ack_snr_sample`, no ACK RSSI |
| `TxResponse::BlockAck` | A BlockAckReq at a non-HT rate; `take_block_ack_completion` gives the `BlockAckReport` |
| `Received`, `RxMeta` | `Esp32s31LowerMac::on_received` takes the `NormalizedRxFrame` of `mac/src/rx.rs`, narrowed by `VifConfig::admits`; `portable::rx_meta` on the configured channel |
| `Channel` | 2.4 GHz, 20 and 40 MHz, while disabled; `Enable` retunes through `LowerMacRetune`, and a refused retune ends as a `Recoverable` `Failed { Enable }` with the port disabled |
| `Vif`, `ReceiveFilter` | The smallest superset policy: station link policy six (`StaLinkRxPolicyHardware`) for the `BSS_MEMBER` rules, station ESP-NOW policy six mode two (`StaEspNowRxPolicyHardware`) when `OTHER_BSS_MANAGEMENT` is requested (the broadcast BSSID outside a BSS), access-point policy eight (`ApRxPolicyHardware`) for the `BSS_MEMBER` rules. Refused: other BSSs' management for the access point. One station and one access point; the station address is the one the cold start published |
| `TxGate` | `set_power_save_tx_block` on every ordinary queue; closing is refused (`Busy`) while an attempt is published, and an attempt admitted behind the closed gate is held and published when it opens |
| `install_key`, `RemoveKey` | CCMP-128 through `CcmpKeyHardware`: the station's pairwise and group slots, the access point's pairwise slots (lowest free association slot) and group slot |
| `AddRxBlockAck`, `RemoveRxBlockAck` | The eight ordinary banks of `RxBlockAckHardware` |
| `LowerMacBeaconTiming` | The station TSF (`StationTsfHardware`) and TBTT schedule (`StationTbttHardware`, the lead as the TBTT ahead time and the vendor's 1.5 ms wake window above it); `Esp32s31LowerMac::on_power_interrupt` reports the TBTT a `MacPowerWakeCause::StaTbtt` edge announces. The access-point TSF only restarts from zero through `ApTsfHardware`'s reset |
| `LowerMacMonitor` | The open promiscuous policy (`MacSnifferHardware`), only while no interface has a receive filter |
| `HardwareServices` | The hardware-owned operations of `ESP32S31_MAC_SERVICE_CAPABILITIES` in `mac/src/capabilities.rs` |

A published attempt cannot be withdrawn: the S31 abort path needs the
queue's hardware timeout edge, so `Cancel` ends a held attempt as `Aborted`
and a published one with its own completion. What the ESP32-S31 lacks and
why (HE and Trigger-based A-MPDU, the access-point TBTT schedule, the
access-point TSF read and arbitrary set, on-air cancel, unicast no-ACK at HT
and HE rates, coexistence levels other than `Normal`, 5 GHz) is recorded in
the
[Wi-Fi/PHY catalog](../../../../qualification/catalog/esp32s31/wifi-phy.toml).
