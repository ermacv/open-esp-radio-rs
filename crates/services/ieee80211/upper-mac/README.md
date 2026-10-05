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
- received frames and `RxTooLong` reports go to the bounded receive queue
  of the interface they belong to (`received(vif)`), extension events such
  as TBTTs to the station's (`extension(vif)`), and lifecycle terminals to
  `lifecycle()`, each reporting its own `EventsLost` in place of the first
  entry it dropped. Every client attaches its interface (`attach(vif, role,
  address)`, up to `ROUTER_VIFS`, released when the `Attachment` drops): the
  only attached interface takes every frame, and a station and an access
  point on one port split them by their addresses
  (`oer-ieee80211-mac`'s `classify_sta_ap_rx`), a station that has not
  joined a BSS taking what neither proves is the access point's, as it
  scans. A frame no attached interface owns is counted (`unrouted_frames`)
  and dropped;
- `EventsLost` from the port marks every exchange still waiting; the
  exchange cancels its attempt by its identity and either receives the
  completion or, when the cancel is refused as not running, learns from
  `resolve(id)` whether the completion still came or was lost in the gap;
- the terminal `Poisoned` event ends `run()` and every wait.

A service talks to the port through one `client::PortClient` per interface
(`PortClientEnv` names the port, the planner's HE TXOP budget, the rate
ladder, the backoff entropy and its `Aggregation`, `aggregate::PortAmpduAggregation`
over a port with `LowerMacAmpdu` or `NoAggregation`; `PortClientConfig` the interface's VIF,
address, role, power and retry limit). `PortClient::new` attaches the
interface, refusing one another client holds; configuring the interface's
BSS (`configure`) routes that BSS's frames to it. The client reads its
interface's receive and extension queues as `PortInput`s (`Frame(PortFrame)` in the port's own
`RxBuffer`, `Tbtt`, `EventsLost`, `Poisoned`), transmits MPDUs and A-MPDUs
through its `UpperMacTx`, and applies settings, the BSS's EDCA parameters,
its interface configuration, retunes and lifecycle commands, with every
failure a `PortClientError`. `PortMsdu` is the MSDU a service hands its
application: the port's buffer, or parts to copy. A service sends no copy of a frame from a queue
of its own: it takes the network's owners from its source
(`oer-ieee80211-datapath`'s `DestinationTxQueues`) when it sends them, and
`frame` names the sizes it keeps (`PORT_FRAME_CAPACITY`,
`PORT_MPDU_HEADER_CAPACITY` for the header it encodes before an Ethernet
frame's payload, `PORT_MPDU_CAPACITY` for a whole MPDU) and splits an
Ethernet-II frame into the header a service encodes and the payload it
sends unchanged (`split_ethernet`); `NetworkBody<F>` is a network frame as
the port's `TxBody`, its payload after that header. A client's environment
names the network's frame once (`PortClientEnv::NetworkFrame`), and its port
takes `NetworkBody` of it (`PortBody`). An MPDU goes to the port as
`TxMpdu { header, body }`: a data frame's encoded header and its payload's
owner, or a management frame whole (`TxMpdu::whole`); each attempt writes
the header into the buffer the port lends and hands the port the body,
which comes back after the attempt for the next one, so a retransmission
sends the same owner. `aggregate::AmpduSubframes<O>` holds one A-MPDU, at
most `PORT_AMPDU_SUBFRAMES` (32) subframes: each one's header and body,
until `clear` drops the bodies when the exchange ended, and makes its
`AmpduRequest` (on-air lengths with FCS and MIC) and the `AmpduFrames` the
client sends; how many frames it carries is `oer-ieee80211-upper-mac`'s
`AmpduLimits`.
`rx_hold::PortRxHold<B, HELD>` stands between a service and a network
whose receive queue may be full. A service hands every MSDU on at once and
goes on taking the port's input, so beacons, EAPOL and its timers never
wait for the network; while the network refuses for room
(`NetworkRefusal::Full`), the hold keeps the MSDUs in their order and sends
them as room appears (`flush`). An MSDU in the port's buffer waits there, up
to `HELD`: held buffers are the port's receive memory, so a network that
stays full makes the port's producer discard bulk data before frames that
keep the link go short. An MSDU in parts (released late by a reorder window,
or one of an A-MSDU) borrows memory the hold cannot keep, so it is copied
into one of `COPIES` slots of `PORT_FRAME_CAPACITY` octets, which the
composition sizes. Beyond them an MSDU is dropped; every outcome is counted
(`PortRxHoldCounters`).
`reorder::RxReorder<AGREEMENTS>` reorders the receive Block Ack agreements
of a service by peer and TID: `offer` releases an in-order MPDU at once as
the release's `CURRENT_SLOT`, which the caller delivers from the port's
buffer, copies only an MPDU its window keeps into `PORT_REORDER_SLOTS`
shared slots (asking the caller to deliver the window's oldest run first,
`MakeRoom`, when every slot is taken), and releases a kept run past its gap
once the caller's gap time passed (`arm_gaps`, `expire_due_gap`). The port's
agreement setting, replay checks and delivery stay the caller's. The
station and access-point services build on all of them.

`UpperMacTx::new(&router, vif, planner)` binds one interface; the router
allocates attempt identities outside the backend-reserved range.
`send_mpdu(TxMpdu, key, request, ladder, entropy)` and, for a port with the
`LowerMacAmpdu` extension,
`send_ampdu(AmpduFrames { headers, bodies, key, min_mpdu_start_spacing },
request, ladder, entropy)` run one exchange to its `TxReport`:

1. the planner plans an attempt (`TxAttemptPlan`);
2. the driver writes the caller's header, the selected subframes' headers
   of the aggregate, or a BlockAckReq it encodes from the first subframe's
   addresses into a buffer the port lends, hands the port their bodies,
   sets the Retry bit where the plan says so, and submits one `TxAttempt`
   with the plan's rate, protection, backoff, power and coexistence level;
3. it awaits the attempt's completion from the router and reclaims the
   bodies to the subframes they came from (`BodiesHeld` when the port keeps
   those of an attempt that ended);
4. it feeds the completion and the port's radio time to the planner and
   repeats with the next plan until the exchange ends.

The caller's headers are read-only: a retransmission writes the first
encoding again, so its sequence number and CCMP packet number repeat, and
sends the same bodies. A refusal, a missing buffer or router slot, a
completion lost in a gap (whose bodies come back once the cancellation
proved the attempt over), a poisoned port (which keeps the bodies until its
reset) or a port error end the exchange with an `UpperMacTxError`; a refused
attempt's buffer goes back to the port.

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
