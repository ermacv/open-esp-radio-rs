# IEEE 802.11 upper-MAC transmit driver

`oer-ieee80211-upper-mac-service` drives the transmit planner of
[`oer-ieee80211-upper-mac`](../../../protocols/ieee80211/upper-mac/README.md)
over any [lower-MAC port](../../../protocols/ieee80211/lower-mac/README.md).
It is the only part of the transmit path that waits, and it waits only on
the port, so it runs under any executor and against the host model.

`UpperMacTx::new(port, vif, planner, first_id)` binds one interface.
`send_mpdu(frame, key, request, ladder, entropy, other_event)` and, for a
port with the `LowerMacAmpdu` extension,
`send_ampdu(AmpduFrames { subframes, key, min_mpdu_start_spacing }, request,
ladder, entropy, other_event)` run one exchange to its `TxReport`:

1. the planner plans an attempt (`TxAttemptPlan`);
2. the driver copies the caller's encoded MPDU, the selected subframes of
   the aggregate, or a BlockAckReq it encodes from the first subframe's
   addresses into a buffer the port lends, sets the Retry bit where the
   plan says so, and submits one `TxAttempt` with the plan's rate,
   protection, backoff, power and coexistence level;
3. it awaits the port's events until the attempt's completion arrives,
   handing every other event to `other_event`;
4. it feeds the completion and the port's radio time to the planner and
   repeats with the next plan until the exchange ends.

The caller's frames are read-only: a retransmission is a fresh copy of the
first encoding, so its sequence number and CCMP packet number repeat. A
refusal, a missing buffer, lost events or a poisoned port end the exchange
with an `UpperMacTxError`; a refused attempt's buffer goes back to the port.

The station and access-point roles do not use the driver yet. Its tests run
it over `oer-ieee80211-lower-mac`'s host model (the `model` feature):
delivery at the first attempt, ACK timeouts walking the rate ladder with the
Retry bit up to the retry limit, a CTS timeout, a partial BlockAck resending
only the missing subframes, a single missing subframe sent alone, the
contention window over a seeded entropy source, and packet numbers across
retries.
