# ESP32-S31 STA, AP and STA+AP datapath state

Date: 2026-08-29; updated 2026-08-30 after standalone-AP RX parity.

This record captures the source and HIL state after the split-radio/network
cutover and the first standalone-AP RX-service correction. Its purpose is to
prevent a partial AP correctness result from being mistaken for STA-level CPU
and throughput parity.

This is an engineering checkpoint, not a sealed qualification verdict. The
listed HIL runs were collected while developing the source set recorded by
this commit. Canonical qualification still requires a clean commit, a sealed
runner bundle and a matching target specification.

## Required outcome

All network-bearing Wi-Fi roles must use the same physical ownership model:

```text
Core0: Wi-Fi DMA -> 802.11 MAC -> CCMP -> BA/reorder
                                      |
                                      v
                             bounded affine SPSC
                                      |
                                      v
Core1:                             IP -> UDP/TCP -> sockets
```

For a standalone AP receiving one saturated HT40/MCS7 client, the intended
production result is the same class of result already demonstrated by STA RX:
no software or DMA loss, wide DMA batches, approximately the radio ceiling,
and less than 40% Core0 radio-task residence. This target must be demonstrated
by matched, low-overhead HIL; a deep-profile cycle sum is not interchangeable
with top-level task residence.

The existing STA evidence must also be stated precisely. STA has demonstrated
112.005 Mbit/s RX at 35.05% Core0 in run `1787925121968-003f9d82`. It has not
demonstrated every 120+ Mbit/s workload below 40% Core0: STA TX reaches about
120 Mbit/s with roughly 69.4% Core0, and a 42/40 Mbit/s duplex diagnostic used
about 59% radio-task residence. Therefore the AP acceptance goal is initially
an RX-only comparison against STA in the same lab and ELF class. TX and duplex
need independent residence budgets.

## Common architecture already present

The shipping role integrations now share these fundamentals:

- radio and network work are split between Core0 and Core1;
- the Core0-to-Core1 data boundary is a bounded affine SPSC rather than an
  Embassy channel;
- the RX DMA ring has 96 descriptors;
- descriptor-backed ownership retained above DMA is bounded to 32;
- the ordinary RX path remains zero-copy through the radio protocol owner;
- RX source IRQ moderation keeps the interrupt masked across a drain epoch and
  unmasks it only after a proven terminal drain;
- negotiated RX BlockAck is BA16;
- AP TX may build aggregates up to 32 MPDUs when peer credit permits it.

The 96/32 storage geometry and BA width are different concepts. The ring and
retained cap bound physical buffering. BA16 is the negotiated reorder window.
A live hardware register value of 64 is the S31 receive hardware bank width,
not proof that the peer negotiated BA64.

No dynamic replacement/page-pool path, unconditional terminal copy or BA32 RX
change was added to obtain the current AP result. Previous same-path
experiments showed that the tested replacement implementation added about
1,684 cycles per MPDU and that copy plus immediate release added 22.3% Core0
work without increasing the ceiling. Those mechanisms remain rejected as
unmeasured solutions to the current AP batching problem.

## Standalone STA path

The connected STA owner in
`driver/adapters/embassy/esp32s31-wifi/src/roles/station/connected/rx_service.rs`
uses one fused bounded turn:

1. consume already staged protocol work;
2. service one frozen DMA frontier;
3. consume newly staged protocol work with the remaining budget;
4. preserve a runnable continuation when synchronous consumption returned a
   staging credit for a frontier that had reported capacity pressure.

The idle protocol budget is the shared staged-owner bound, currently 32
frames. Active TX shortens it to four frames so RX protocol processing cannot
hide a terminal TX completion. Recycled-append continuation is adaptively
coalesced while the RX interrupt remains masked. The final STA RX run averaged
11.97 MPDUs per DMA service call; that amortization was the primary change
which reduced the former 95%+ Core0 residence to approximately 35%.

## Standalone AP path

The current standalone AP path in
`driver/adapters/embassy/esp32s31-wifi/src/roles/access_point/datapath.rs` has
received the following correctness, ownership and scheduling changes:

- AP DMA staging and AP protocol handling are explicit phases;
- AP protocol admission is synchronous inside the Core0 owner;
- an idle AP turn now follows `protocol pre-drain -> one DMA service ->
  protocol post-drain`, matching the important STA ordering property;
- the active-TX turn is bounded to four protocol frames;
- the physical TX domain is considered even when the AP-local MAC queue does
  not report pending work;
- peer-activity and power-save updates no longer take an additional protocol
  mailbox trip;
- association entropy and nonce material are generated for a fresh WPA2
  association rather than for each received data MPDU;
- ordinary RX preflight is in-place and avoids a duplicate AP peer/reorder
  parse;
- STA and AP use the same role-neutral `FusedRxTurn` budget/result mapping;
- empty AP protocol probes are gated by one complete readiness predicate and
  no longer consume frame budget;
- the ordinary WPA2 QoS frame uses a synchronous AP-specific short leaf while
  management, EAPOL, fragmentation, A-MSDU and reorder gaps retain the full
  correctness path;
- AP exports the same monotonic DMA completed-unit and staged-byte counters as
  STA to the common recycled-append continuation policy.

The final item fixed the dominant remaining scheduling defect. The AP adapter
had inherited the default zero `DatapathRxWorkCounters`, even though its DMA
producer had completed real work. Every `RecycledAppendPending` therefore
looked like an empty confirmation to the common adaptive policy. The policy
reselected its 64-us bootstrap delay indefinitely instead of advancing to the
bounded 1024-us long-frame level. This was an integration error, not an RF,
PSRAM, BA-width or PHY limitation.

## Standalone AP evidence

### RX parity result, 2026-08-30

Before exporting AP DMA work counters, coarse same-image run
`1788040809460-001b8462` delivered approximately 109 Mbit/s but performed
41,150 DMA calls for 111,680 MPDUs. It returned 38,369
`RecycledAppendPending` results; all 38,369 adaptive decisions falsely
reported zero completed units and zero staged bytes, so every decision used
64 us. Core0 radio cycles occupied approximately 90.3% of one 320-MHz core in
that lower-overhead coarse image. The more intrusive phase image measured
approximately 94.6%.

After the role-neutral counter correction, same-image run
`1788041102086-001b8e1a` produced this result:

| Counter | Before | Corrected |
| --- | ---: | ---: |
| delivered RX | ~109 Mbit/s | 109.0 Mbit/s |
| DMA calls | 41,150 | 7,736 |
| completed MPDUs | 111,680 | 111,718 |
| MPDUs per DMA call | 2.71 | 14.44 |
| calls with at least 8 MPDUs | 3 | 6,356 |
| real RX IRQ posts | 2,673 | 301 |
| Core0 coarse radio load | ~90.3% | 46.95% |

The corrected adaptive decisions observed 108,507 MPDUs and 174,455,768
staged bytes. Only 96 empty confirmations used 64 us; 6,462 decisions selected
the 1024-us class. Hardware `BUFFER_FULL`, FIFO overflow and transport errors
were zero.

The independent production-style task-residence image then passed two cycles
in run `1788041259260-001b90da`:

| Cycle | Delivered RX | Radio-task residence |
| --- | ---: | ---: |
| 1 | 109.4 Mbit/s | 6.303029 / 16.062447 s = 39.24% |
| 2 | 109.3 Mbit/s | 6.338854 / 16.058205 s = 39.47% |

This satisfies the initial standalone-AP RX parity gate: wide batches,
ceiling-class throughput and less than 40% Core0 task residence. No PSRAM or
PHY placement change was part of the causal correction.

### Typed ordinary-data parity update, 2026-08-30

A later same-image-class comparison isolated a remaining AP-specific cost.
Task-residence image CRC `472a5ac8` delivered 110.985 and 112.447 Mbit/s in AP
run `1788055115367-00202358`, but occupied 43.806% and 44.213% Core0. The same
CRC delivered 114.080 Mbit/s in STA run `1788055291761-00202bf2` at 36.280%
Core0. OpenWrt reported MCS7/40 MHz and at most two retries in the AP run and
one retry in the STA run. This proved an AP software-path delta under matched
DMA, scheduler and memory placement; it was not evidence for an RF or PSRAM
limitation.

The AP ordinary-data path was then cut over to a compact typed transaction:

- `Esp32s31ApOrdinaryPairwiseRxRequest` carries only peer, replay lane and
  CCMP header; fragment prepare/commit state is absent by construction;
- the fast dispatcher declines non-ordinary or active-fragment state without
  mutation, and the caller explicitly enters the complete slow graph;
- peer/key replay admission and admitted-data activity use the same
  generation-bound AP peer binding;
- the current staged owner publishes directly in place; the copying/reorder
  path remains reserved for A-MSDU, fragments and gaps;
- success-only role observation is compiled out of the production ordinary
  leaf while remaining present in diagnostics and tests.

No DMA ring, retained-owner cap, BA width, PSRAM placement, PHY or checksum
setting changed in this cutover. The final task-residence image CRC
`f4d832db` passed standalone AP run `1788057272037-00209cb0`:

| Cycle | Delivered RX | Core0 radio | Core1 network | OpenWrt retries/failed |
| --- | ---: | ---: | ---: | ---: |
| 1 | 112.514 Mbit/s | 38.858% | 37.438% | 0 / 0 |
| 2 | 112.300 Mbit/s | 38.792% | 37.427% | 0 / 0 |

The same CRC passed three concurrent STA+AP RX repetitions in run
`1788057035885-0020927e`. Each role received approximately 32.5 Mbit/s; Core0
radio residence was 39.553%, 39.774% and 39.681% (39.669% mean), and Core1
network residence was 27.601%, 27.705% and 27.711%. This closes the current
RX-only `<40%` residence gate for both standalone AP ceiling traffic and the
measured paired ingress workload. TX and four-flow duplex still require their
own budgets.

### Earlier correctness checkpoint

The first corrected 42/40 Mbit/s duplex run was
`1788031620404-0018f648`:

| Counter | Before | Corrected | Change |
| --- | ---: | ---: | ---: |
| DMA service calls | 25.9k | 21.4k | -17.3% |
| AP protocol cycles | 734.5M | 675.5M | -8.0% |
| complete runner cycles | 1.662B | 1.613B | -2.9% |

All 35,672 staged frames were serviced. Software drops, DMA `BUFFER_FULL` and
FIFO overflow were zero, and the BA state remained valid.

Run `1788031825449-0018fdc1` raised both requested directions to 65 Mbit/s and
measured 57.2--57.7 Mbit/s AP RX plus 45.9--47.4 Mbit/s AP TX, or
103.6--104.6 Mbit/s combined. It processed 48.98k/49.33k MPDUs without
software drops, `BUFFER_FULL` or FIFO overflow. Before the correction, the
old path lost approximately 22--23k MPDUs and reached `BUFFER_FULL` counts up
to 31 under the same class of load.

The corrected AP averaged approximately 1.66 MPDUs per DMA call in the 42/40
run and 2.70 in the 65/65 run. Observed AP frontiers reached 11--20 MPDUs, so
the narrow average is not evidence that the radio only delivered small
aggregates. Incoming AP traffic averaged about 15.8 MPDUs per BA16 aggregate,
while AP outbound traffic averaged about 31.98 MPDUs per aggregate. Both the
RX BA16 agreement and TX BA32 aggregation are therefore operational.

Deep AP diagnostics attributed roughly 68% of the interval to the instrumented
Core0 runner around 104 Mbit/s combined. That number proves that substantial
work remains, but it is not a production occupancy verdict: the diagnostics
collect detailed per-phase counters and change the ELF and instruction work.
A low-overhead task-residence build was attempted with
`access-point-single-client-ceiling-bidirectional-task-poll`, but run
`1788032470027-00194fd5` did not build because runtime SRAM owners overlap the
8-KiB bootstrap handoff margin. Increasing flash capacity to 4 MiB does not
resolve that internal-SRAM placement conflict.

Consequently, the current evidence establishes:

1. the AP packet-loss/`BUFFER_FULL` failure was a software service defect and
   is corrected in the measured workloads;
2. the remaining high runner frequency was a second software integration
   defect: AP hid DMA work from the common adaptive continuation;
3. standalone AP now demonstrates STA-class DMA batching and less than 40%
   production radio-task residence at approximately 109 Mbit/s RX.

It is not established that the remaining gap is caused by RF conditions,
cache layout, checksum, BA width or an irreducible AP protocol cost.

## Concurrent STA+AP path

The paired owner in
`driver/adapters/embassy/esp32s31-wifi/src/roles/concurrent/rx_service.rs`
shares the split-radio/network resources, routes staged ownership to the
station or AP role and now uses the same role-neutral `FusedRxTurn` accounting
as the standalone owners. The active-TX quantum remains four frames; the idle
turn uses the full staged-owner bound. The paired classifier still performs
per-frame role routing, but ordinary AP frames enter the same compact typed
leaf measured by standalone AP.

Run `1788057035885-0020927e` is the current low-overhead paired RX gate. It
shows that the shared orchestration is no longer a blocker at the measured
65-Mbit/s aggregate ingress offer. This does not establish four-flow duplex or
TX parity; those paths exercise physical TX ownership and completion cadence
which are absent from the RX-only gate.

## Current assessment

The architectural direction is correct:

- Core0 retains physical radio ownership and protocol work that depends on
  DMA/BA/CCMP ordering;
- Core1 owns the IP stack and sockets;
- descriptor capacity, packet lifetime and network scheduling are bounded;
- interrupt moderation and delayed recycled-append continuation amortize
  service boundaries without early unmasking;
- AP correctness was improved without introducing copying or a second
  cross-core allocator.

Standalone STA, AP and concurrent STA+AP now share the physical turn and
continuation facts which determine saturated RX batching. Their control-plane
leaves remain intentionally role-specific: AP resolves peer, controlled port,
replay generation and power-save state, while STA owns one associated peer.
The ordinary Ethernet transaction is compact and synchronous in both cases.

## Next work and proof gates

The next sequence is:

1. Keep the low-overhead paired STA+AP RX gate and add a separate duplex
   residence gate so functional routing success cannot be mistaken for TX
   ownership parity.
2. Avoid the paired classifier's duplicate normalized parse without merging
   STA/AP control-plane state.
3. Re-measure before further optimizing `admit_rx_data`, peer accounting or security
   parsing. Optimize only components which remain expensive in normalized
   cycles per serviced MPDU.
4. Characterize AP TX separately. Historical AP TX is about 113 Mbit/s versus
   STA near 120 Mbit/s; measure terminal completion to next aggregate
   publication rather than inferring a blocker from throughput alone.
5. Validate monitor and scanner work accounting at their finite service
   boundaries; they need the same truthful physical counters but not the
   saturated network protocol leaf.
6. Consider RX BA32 only after the common lifecycle is proven. BA32 may change
   airtime or burst tolerance, but it is not a substitute for fixing service
   cadence.

For standalone AP RX parity, require all of the following in matched HIL:

- at least 10 MPDUs per DMA service call under saturated long-frame traffic;
- zero software drop, DMA `BUFFER_FULL` and FIFO overflow;
- no BA/replay/reorder integrity regression;
- Core0 radio-task residence below 40%;
- delivered throughput comparable to STA under the same observed air input;
- independent air and route evidence so a quiet AP, laptop WLAN route or host
  bottleneck cannot masquerade as a target optimization.

The correct current statement is: standalone AP RX has reached the requested
STA performance class in the measured HT40 ceiling workload, and concurrent
STA+AP RX is below 40% Core0 at the measured 65-Mbit/s aggregate offer. AP TX
and duplex retain independent performance budgets.
