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
`PhyRate` and `RxMeta`, and a completion to `TxStatus`. The ESP32-S31 does not
implement the port yet. Its register seams map onto the port as follows:

| Port operation | ESP32-S31 seam |
| --- | --- |
| `submit`, `TxCompleted` | `TxHardware` prepare/start of one queue and `take_tx_completion` in `mac/src/tx.rs`; A-MPDUs through `HtAmpduHardware` and `take_block_ack_completion` |
| `TxStatus`, `ack_snr_db` | `TxCompletion::disposition` and `ack_snr_sample` in `mac/src/tx.rs`; the S31 reports no ACK RSSI |
| `Received`, `RxMeta` | `RxDma` and `decode_normalized_rx_metadata` in `mac/src/rx.rs`; the prefix gives rate, RSSI and aggregation, not noise floor or timestamp |
| `install_key`, `RemoveKey` | `CcmpKeyHardware` in `mac/src/crypto.rs` |
| `Vif`, `ReceiveFilter` | `StaApRegisterHardware` and `ApRxPolicyHardware` over `MacStaApReceivePlan` in `mac/src/sta_ap_registers.rs` and `mac/src/ap_policy.rs` |
| `AddRxBlockAck`, `RemoveRxBlockAck` | `RxBlockAckHardware` in `mac/src/rx/hardware.rs` |
| `SetTsf`, `Tbtt`, `tsf` | `ApTsfHardware` in `mac/src/ap_tsf.rs` resets, starts and stops the access-point TSF |
| `HardwareServices` | `ESP32S31_MAC_SERVICE_CAPABILITIES` in `mac/src/capabilities.rs`: FCS, immediate ACK, backoff countdown, CCMP transform, receive Block Ack matching and transmit Block Ack capture |
