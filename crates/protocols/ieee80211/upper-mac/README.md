# IEEE 802.11 upper-MAC transmit policy

`oer-ieee80211-upper-mac` holds the transmit decisions that lie above the
[lower-MAC port](../lower-mac/README.md): one port submission is one hardware
transmission attempt, and everything a backend does not report as a
`HardwareServices` offload is decided here, once for every backend. The
package is sans-IO: completions and the radio time enter as values, attempt
plans and reports leave as values, and nothing waits.

| Module | Decides | Inputs |
| --- | --- | --- |
| `retry` | Whether an MPDU is sent again, whether that attempt sets the Retry bit, how the contention window changes, and the rate of each attempt | `TxStatus`, `RetryLimits`, a `RateLadder` |
| `aggregate` | How many queued frames one A-MPDU carries: the agreement's window, the port's capabilities, the recipient's Maximum A-MPDU Length and the TXOP limit with its BlockAck | `AmpduLimits` |
| `ampdu` | Which subframes of an A-MPDU the next attempt resends after its BlockAck, republication after a failed protection exchange, the BlockAckReq after repeated failures, individual retries, aging and the end of the exchange | `BlockAckReport`, `AmpduRetryPolicy` |
| `rate_control` | The first rate of a peer's MPDUs and A-MPDUs, one controller per peer (a station's access point, each station of an access point), learning from each exchange, starting from the `LinkMetric` (signal over the noise floor, required) of the frame that made the link; `FixedRateControl` | `RatePeer`, `TxReport` outcomes |
| `rate_selection` | The retry plan of each frame an interface sends, by what it is for (`FrameRates`: probe, control, data), learning from each terminal and from the association with its peer; `RateSelection` is the seam a service drives | `RatePeer`, `TxCompletion` |
| `protection` | RTS/CTS or CTS-to-self and the control-frame rate for each PPDU | `BssProtection`, dot11RTSThreshold, an `HeTxopRtsBudget` |
| `tx` | `TxPlanner`: the next `TxAttemptPlan` (rate, protection, backoff, power, coexistence, content and Retry bits) of an MPDU or A-MPDU exchange, and its `TxReport` | `TxRequest`, `TxCompletion`, `RadioInstant`, entropy |

The backoff is drawn by `oer-ieee80211-softmac`'s `EdcaContention`, one per
access category inside the planner: doubled after each failed attempt that
is retried, reset when the frame ends. Reports are the softmac contract's
`MacTxStatus` and `MacAmpduTxStatus`. The checked `WmmAccessCategory` selects
its cell in the four-entry contention array through the MAC's selector, the
same mapping used by the station's data admission pools.

`RateControl` observes a `NonZeroU16` transmission count with `MpduOutcome`
for an ordinary MPDU, or a checked `BlockAckTally` for an aggregate. A tally
cannot contain zero attempts or more acknowledgements than attempts. The
controller returns `AmpduUnavailable` directly when it has no aggregate
state, before changing its rate or ACK-SNR filter.

## Parameters and profiles

What differs between implementations is a parameter, never a branch on a
vendor:

- `RetryLimits`: dot11ShortRetryLimit, dot11LongRetryLimit and whether an
  ACK timeout counts against the short or the frame-class counter
  (`RetryLimits::IEEE_DEFAULT` is seven, four and by frame class).
- `RateLadder`: the rate of the attempt after `n` failures. `FixedRate`
  keeps the first rate; a rate-control owner supplies its own.
- `AmpduRetryPolicy`: MSDU lifetime and aging margin, the retry limit of an
  unanswered aggregate and of failed protection exchanges, and whether one
  missing subframe stays aggregated.
- `HeTxopRtsBudget`: how many HE SU APEP octets fit below a TXOP duration
  threshold; `ProtectEveryHeTxop` protects every individual HE PPDU while a
  threshold is advertised.

[`oer-espressif-ieee80211-policy`](../../espressif/ieee80211/policy/README.md)
supplies the Espressif values recovered from the vendor stack.

A-MPDU lifetime and aging margin are spans of the port's radio clock
(`Ieee80211Duration`). Aging uses the
same radio epoch as the commit instant and starts strictly after
`commit + lifetime - margin`; the exact first aged instant is checked before
admission. An unrepresentable threshold is `AgingThresholdOutsideEpoch`.
The S31 adapter retains the commit stamp's generation and rejects a sample
from another generation before consuming a completion or changing retry state.
Its STA/AP runtimes read the existing MAC `ReceptionClock`; monotonic timers
remain the source for waiting and transmit watchdogs.

WMM categories select one cell of an exact four-cell contention array.
`AmpduRequest` owns its bounded lengths; asking for a subframe outside that
request gives `None`, and the planner reports `SubframeOutsideAggregate`
before publishing an attempt. An unrepresentable subframe size admits no
frame and leaves the aggregate run unchanged. `ampdu::compact` operates on
fixed arrays of Copy metadata, never on affine MPDUs or payload owners.
It swaps selected entries into the prefix in order, without a full-array
temporary. Only the first 64 entries are addressable by its mask; callers
consume the returned prefix length and do not inspect the remaining suffix.
The crate uses the canonical strict lint block checked by tidy.

## What stays outside

The planner holds no frame bytes: the caller keeps its encoded MPDUs and
re-encodes each attempt, so a retransmission repeats the first encoding's
sequence number and CCMP packet number. Sequence numbers, packet numbers
(`oer-ieee80211-mac::ccmp::CcmpTxPacketNumber`) and the receive reorder
buffer (`oer-ieee80211-mac::block_ack::reorder`) are allocated and kept by
their owners in `oer-ieee80211-mac`. Adaptive rate control, which chooses a
frame's first rate from the reports, is the caller's.

The ESP32-S31 MAC (`oer-esp32s31-ieee80211-mac`) runs its ordinary retry,
A-MPDU retry and protection owners through these algorithms with adapters
from its chip rate and completion types.
