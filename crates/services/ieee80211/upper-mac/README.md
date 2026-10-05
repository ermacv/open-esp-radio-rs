# IEEE 802.11 upper-MAC transmit driver

`oer-ieee80211-upper-mac-service` drives the transmit planner of
[`oer-ieee80211-upper-mac`](../../../protocols/ieee80211/upper-mac/README.md)
over any [lower-MAC port](../../../protocols/ieee80211/lower-mac/README.md).
It is the only part of the transmit path that waits, and it waits only on
the port, so it runs under any executor and against the host model.

The port has exactly one event consumer: `EventRouter::new(port, first_id)`
is that consumer, and its `run()` future, which the composition polls beside
the backend's runner, takes every event and dispatches it:

- a completion goes to the exchange that registered its `TxId`
  (`register(id)` before the submission, `completion(id)`), so exchanges on
  different access categories run concurrently over one port;
- received frames and `RxTooLong` reports go to a bounded receive queue
  (`received()`), lifecycle terminals to `lifecycle()` and extension events
  such as TBTTs to `extension()`, each reporting its own `EventsLost` in
  place of the first entry it dropped;
- `EventsLost` from the port marks every exchange still waiting; the
  exchange cancels its attempt by its identity and either receives the
  completion or, when the cancel is refused as not running, learns from
  `resolve(id)` whether the completion still came or was lost in the gap;
- the terminal `Poisoned` event ends `run()` and every wait.

A service talks to the port through one `client::PortClient` per interface
(`PortClientEnv` names the port, the planner's HE TXOP budget, the rate
ladder and the backoff entropy; `PortClientConfig` the interface's VIF,
address, role, power and retry limit). The client reads the router's receive
and extension queues as `PortInput`s (`Frame(PortFrame)` in the port's own
`RxBuffer`, `Tbtt`, `EventsLost`, `Poisoned`), transmits MPDUs and A-MPDUs
through its `UpperMacTx`, and applies settings, the BSS's EDCA parameters,
its interface configuration, retunes and lifecycle commands, with every
failure a `PortClientError`. `PortMsdu` is the MSDU a service hands its
application: the port's buffer, or parts to copy. `queue::TxQueue` is a
service's first-in first-out ring of `PORT_TX_QUEUE` Ethernet frames of at
most `PORT_FRAME_CAPACITY` octets, each with its user priority, whose
`head_run` is the run of one priority an A-MPDU may carry.
`reorder::RxReorder<AGREEMENTS>` reorders the receive Block Ack agreements
of a service by peer and TID: `offer` releases an in-order MPDU at once as
the release's `CURRENT_SLOT`, which the caller delivers from the port's
buffer, copies only an MPDU its window keeps into `PORT_REORDER_SLOTS`
shared slots (asking the caller to deliver the window's oldest run first,
`MakeRoom`, when every slot is taken), and releases a kept run past its gap
once the caller's gap time passed (`arm_gaps`, `expire_due_gap`). The port's
agreement setting, replay checks and delivery stay the caller's. The
station and access-point services build on all three.

`UpperMacTx::new(&router, vif, planner)` binds one interface; the router
allocates attempt identities outside the backend-reserved range.
`send_mpdu(frame, key, request, ladder, entropy)` and, for a port with the
`LowerMacAmpdu` extension,
`send_ampdu(AmpduFrames { subframes, key, min_mpdu_start_spacing }, request,
ladder, entropy)` run one exchange to its `TxReport`:

1. the planner plans an attempt (`TxAttemptPlan`);
2. the driver copies the caller's encoded MPDU, the selected subframes of
   the aggregate, or a BlockAckReq it encodes from the first subframe's
   addresses into a buffer the port lends, sets the Retry bit where the
   plan says so, and submits one `TxAttempt` with the plan's rate,
   protection, backoff, power and coexistence level;
3. it awaits the attempt's completion from the router;
4. it feeds the completion and the port's radio time to the planner and
   repeats with the next plan until the exchange ends.

The caller's frames are read-only: a retransmission is a fresh copy of the
first encoding, so its sequence number and CCMP packet number repeat. A
refusal, a missing buffer or router slot, a completion lost in a gap, a
poisoned port or a port error end the exchange with an `UpperMacTxError`; a
refused attempt's buffer goes back to the port.

The portable station of `oer-ieee80211-sta-service` (`port`) sends every
frame through it; the ESP32-S31 roles do not use it yet. Its tests run
it over `oer-ieee80211-lower-mac`'s host model (the `model` feature):
delivery at the first attempt, ACK timeouts walking the rate ladder with the
Retry bit up to the retry limit, a CTS timeout, a partial BlockAck resending
only the missing subframes, a single missing subframe sent alone, the
contention window over a seeded entropy source, packet numbers across
retries, two concurrent exchanges on different access categories with
received frames between their completions, a loss recovered by cancelling
the attempt in flight, a completion lost in the gap, and a poisoned port
ending every exchange.
