# IEEE 802.11 lower-MAC port

`oer-ieee80211-lower-mac` declares `Ieee80211LowerMacPort`, the radio port
between portable Wi-Fi MAC logic and the backend that puts frames on the air.
It follows the five-part port shape of
[the architecture](../../../../docs/architecture.md#radio-ports) and is
sans-IO: it declares the port and its values and never waits on it.

## Contract

One submission is one hardware transmission attempt. The backend counts down
the medium backoff, sends the PPDU once at the submitted rate, waits for the
solicited response and reports one completion. Retries, rate fallback,
sequence and packet numbers, reordering and the backoff draw stay above the
port unless the backend reports them in its capabilities.

| Part | Items |
| --- | --- |
| Submission | `submit(TxAttempt)`: caller `TxId`, `VifId`, `WmmAccessCategory`, one MPDU with its `TxResponse` or one A-MPDU (`AmpduSubmission`), `PhyRate`, `Protection` (none, RTS/CTS, CTS-to-self), `KeySelector`, `TxPower`. Refusal is `SubmitError`, and nothing is sent |
| Events | `next_event` yields owned events viewed as `LowerMacEvent`: `Received { frame, RxMeta }`, `TxCompleted(TxCompletion)` with `TxStatus`, ACK RSSI and SNR and the `BlockAckReport` (starting sequence, bitmap) of an A-MPDU, `Tbtt`, and lifecycle terminals. Overflow is reported once as `EventsLost` |
| Controls | `apply(LowerMacSetting)`: `Channel`, interface configuration (`VifConfig`: address, `VifRole`, BSSID, `ReceiveFilter`), key removal, receive Block Ack agreements, TSF set, TBTT schedule, power-save transmit hold and the `CoexPriority` hint. `install_key` returns the `KeyHandle` attempts select |
| Capabilities | `LowerMacCapabilities`: bands, widths, rates, `HardwareServices`, interfaces, queues, A-MPDU subframes, key slots and receive Block Ack limits |
| Lifecycle | `Enable`, `Disable`, `Quiesce` and `Cancel(TxId)`; a cancelled attempt ends with its completion (`TxStatus::Aborted`) |
| Clock | `now()` is the `oer_time::RadioInstant` of receive timestamps; `tsf(vif)` reads an interface's `Tsf` |

Every call but `next_event` is synchronous. Failures are `Rejected` (the inner
`Err` of a call), `Recoverable` (`TxStatus::Aborted` or `TxStatus::Fault`) or
`Poisoned` (`Err(Self::Error)`).

The PHY and channel values come from
[`oer-ieee80211-mac`](../mac/src/phy.rs): `Channel` (2.4 GHz and 5 GHz with
20 MHz and 40 MHz geometry) and `PhyRate` (non-HT, HT and HE SU).
`RxEvidence` and `RxCryptoStatus` are the receive provenance values
`oer-ieee80211-softmac` re-exports as `MacRxEvidence` and
`MacRxCryptoStatus`, and softmac's `MacTxResult::HardwareFailure` carries a
`TxStatus`. `MacOperationOwnership::hardware_services` turns a softmac
service description into `HardwareServices`.

## Implementers

The crate's tests implement the port with an in-memory model to show that it
needs no chip types.

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
  in `oer-esp32s31-ieee80211-runtime` implements `Ieee80211LowerMacPort` over
  the core: a bounded owned-event queue, the interrupt and receive entries,
  the publication watchdog and the PHY retune of `Enable`.

The station and access-point roles do not use the port yet. Its operations
map onto the S31 seams as follows:

| Port operation | ESP32-S31 implementation |
| --- | --- |
| `submit` of one MPDU | `OrdinaryTxOwner::start_single_attempt` in `src/ordinary_tx.rs`: one `TxHardware` publication at the submitted rate with the caller's protection; the owner's retry ladder, rate fallback and BSS protection selection are not applied. One attempt is in flight at a time (`Busy`) |
| `TxCompleted` | `take_tx_completion` through `TxCompletion::tx_status`; a collision detach is `Collision`, the hardware timeout abort `Aborted`; `ack_snr_db` from `ack_snr_sample`, no ACK RSSI |
| `TxResponse::BlockAck` | A BlockAckReq at a non-HT rate; `take_block_ack_completion` gives the `BlockAckReport` |
| `Received`, `RxMeta` | `Esp32s31LowerMac::on_received` takes the `NormalizedRxFrame` of `mac/src/rx.rs`; `portable::rx_meta` on the configured channel |
| `Channel` | 2.4 GHz, 20 and 40 MHz, while disabled; `Enable` retunes through `LowerMacRetune` |
| `Vif`, `ReceiveFilter` | `ReceiveFilter::NONE` or `BSS_MEMBER`: station policy six (`StaLinkRxPolicyHardware`, `StaApRegisterHardware` to close it) and access-point policy eight (`ApRxPolicyHardware`). One station and one access point; the station address is the one the cold start published |
| `install_key`, `RemoveKey` | CCMP-128 through `CcmpKeyHardware`: the station's pairwise and group slots, the access point's pairwise slots (lowest free association slot) and group slot |
| `AddRxBlockAck`, `RemoveRxBlockAck` | The eight ordinary banks of `RxBlockAckHardware` |
| `SetTsf`, `tsf` | The station TSF (`StationTsfHardware`); the access-point TSF only through `ApTsfHardware`'s reset, `SetTsf` to zero |
| `PowerSaveTxBlock` | A software hold: the admitted attempt stays unpublished until released |
| `HardwareServices` | The hardware-owned operations of `ESP32S31_MAC_SERVICE_CAPABILITIES` in `mac/src/capabilities.rs` |

Refused as unsupported: A-MPDU attempts (`max_ampdu_subframes` is zero),
`TxPower::MaxDbm`, individually addressed frames without an ACK, other
receive filters (promiscuous, other BSSs' management), `Tbtt`, reading or
setting the access-point TSF other than its reset, and `CoexPriority`. A
published attempt cannot be withdrawn: `Cancel` ends a held attempt as
`Aborted`, and a published one with its own completion.
