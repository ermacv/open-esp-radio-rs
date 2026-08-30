# ESP32-S31 STA, AP and STA+AP datapath state

Date: 2026-08-29; updated 2026-08-30 after standalone-AP RX parity, the AP TX
aggregate-owner cutover and the STA active-DATAPATH execution cutover.

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

The current one-direction evidence must be stated precisely. STA has
demonstrated 112.005 Mbit/s RX at 35.05% Core0 in run
`1787925121968-003f9d82`, and the active-DATAPATH cutover below demonstrated
124.139 Mbit/s TX at 34.30--34.39% Core0 in runs
`1788086992979-00279b2d` and `1788087158341-00279d4f`. Standalone AP has
independently demonstrated both RX and TX below 40%. Duplex still has its own
workload and residence budget; one-direction headroom is not a duplex claim.

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
- standalone STA and AP both execute the common `DatapathRunner` directly at
  a flat Core0 execution boundary; only their role-specific lifecycle and
  protocol leaves differ.

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

### AP TX aggregate-owner cutover, 2026-08-30

The next AP-only delta was in TX ownership rather than PHY or memory
placement. The station aggregate owner filled the inactive A-MPDU arena from
the currently ready network FIFO prefix while the active PPDU was on air. The
AP owner originally crossed the generic DATAPATH scheduler once per Ethernet
frame. Enabling that common lookahead directly restored approximately 123
Mbit/s, but raised Core0 residence to approximately 58%; it repeated scheduler
and AP owner transitions for every MPDU.

The production cutover keeps AP peer, key, power-save and TID policy inside the
AP role while making the ready-prefix operation batch-owned:

1. the common scheduler waits until the exact missing prefix for the preferred
   negotiated batch is published;
2. it transfers one lease to the AP owner;
3. the AP owner synchronously drains only the currently ready, compatible FIFO
   prefix into standby;
4. after terminal completion, a bounded saturated chain may start the complete
   prepared successor without reconstructing the outer future;
5. stop, due control, fresh RX and due recycle-only RX continuation all retain
   priority at every physical transaction boundary.

Lookahead remains limited to a single-interface network-owned TX. Paired
STA+AP and control-owned TX do not use it, so a physical interface arbitration
decision cannot be hidden inside one role's batch drain.

Task-residence run `1788079938522-00265c73` used image CRC `e5d0a804` and
measured:

| Cycle | AP TX | Core0 radio residence | OpenWrt link |
| --- | ---: | ---: | --- |
| 1 | 122.087 Mbit/s | 5.585007 / 16 s = 34.91% | MCS7/HT40/SGI, 0 retry / 0 failed |
| 2 | 122.124 Mbit/s | 5.610024 / 16 s = 35.06% | MCS7/HT40/SGI, 0 retry / 0 failed |

The final observer-free production image CRC `25d215e5` then passed all six
qualification windows in run `1788082145529-0026f86c`. Delivered throughput
was 120.619, 121.103, 120.869, 120.999, 120.861 and 120.905 Mbit/s. OpenWrt
reported MCS7/HT40/SGI for AP-to-client traffic in every window, with zero TX
retries, zero failed transmissions and zero TID0 AQM drops. This is the final
source-set throughput confirmation; the separate task-residence image above
provides the low-overhead Core0 bound.

The same source also passed the observer-free AP bidirectional gate in run
`1788080132783-00266195`. Six 65/65-Mbit/s offer cycles delivered
106.225--109.429 Mbit/s combined. The matched observer-free station control
`1788081108430-00267310` delivered 104.353 Mbit/s combined at the same offer
and payload size. Therefore the current approximately 108-Mbit/s AP duplex
ceiling is not evidence that AP has a slower common datapath than STA.

The duplex directions remain asymmetric. With the OpenWrt AP client in its
automatic guard-interval mode, AP received 55--57 Mbit/s and transmitted
50--53 Mbit/s. Driver-observation run `1788080456808-00266610` proved that the
incoming data was MCS7/HT40/LGI, that the independent monitor and AP agreed on
the unique BlockAck'ed MPDU count, and that the AP path had no software drop,
DMA `BUFFER_FULL`, FIFO overflow, MIC failure, duplicate or reorder gap. The
OpenWrt sender simultaneously reported 2.35k--2.49k AQM drops. Missing UDP
sequences in that run were therefore absent before AP delivery, not lost in
the AP DMA/reorder/network path.

Forcing the OpenWrt client to SGI moved this diagnostic configuration across a
capacity boundary. The per-phase image reported 19/65 overload discards and
one `BUFFER_FULL=29` cycle; the lighter correctness image reported
`BUFFER_FULL=48`. Those results prove that the diagnostic path cannot sustain
the faster incoming burst indefinitely. They do not establish a production
buffer failure because both images compile driver observation into the
per-frame hot path and the fail-closed HIL catalog intentionally forbids a
fixed-MCS/GI claim from an observer-free image. No ring-size, SRAM-placement,
copy or replacement-buffer change is justified by this evidence alone.

### STA active-DATAPATH execution cutover, 2026-08-30

Matched phase counters did not localize the remaining standalone-STA TX cost
to A-MPDU preparation or publication. In task-residence runs before the
cutover, STA delivered 120.5--122.3 Mbit/s while the Core0 radio task occupied
47.2--48.0% of the 16-second interval. The same common TX primitives in AP
delivered approximately 122.1 Mbit/s at 34.9--35.1% Core0. The complete STA
runner retained roughly 5,000 additional cycles per datagram outside the
instrumented aggregate phases. Those counters alone did not exclude checksum,
airtime or another uninstrumented leaf; the controlled execution-boundary A/B
below established the causal component.

The causal difference was execution topology. AP creates one
`DatapathRunner` and awaits it near the physical supervisor root. STA formerly
polled the same long-lived runner through connected transaction, connected
phase, attempt runner and reconnect lifecycle futures on every wake. The
post-LTO connected STA future was approximately 162 KiB of code, compared
with an approximately 27-KiB AP service future. A controlled child-task A/B
removed only that repeated parent poll chain and raised TX while reducing
measured Core0 work.

The production design is therefore a hybrid, not a duplicate AP stack:

1. scan, authentication, association, WPA2 and reconnect/backoff remain owned
   by the existing finite STA lifecycle;
2. after the lifecycle constructs the exact connected `DatapathRunner`, it
   transfers that non-`Send` owner to one executor-affine Core0 active actor;
3. the parent waits only for actor completion or a station command and never
   polls the packet runner while it is active;
4. the actor returns the complete runner before any teardown is attempted;
5. the original parent synchronously classifies simultaneous command/link
   exits, revokes RX admission, drains the MAC frontend, parks the IRQ route
   and restores every reusable owner in the original order.

The actor does not move radio work to another core, add a packet queue, change
DMA-buffer lifetime or create an alternate datapath. It is a poll-topology
boundary around the already-common `DatapathRunner`. AP remains direct because
its active service is already flat; forcing AP through an unnecessary mailbox
would add ownership machinery without removing a measured parent chain.

The first causal task-boundary image CRC `11ae9491` provided this same-ELF
result before the terminal classifier was moved into the common adapter:

| Role/cycle | Delivered TX | Core0 radio residence | Average hot poll |
| --- | ---: | ---: | ---: |
| STA | 123.849 Mbit/s | 5.018300 / 16.001258 s = 31.36% | 34.21 us |
| AP cycle 1 | 122.984 Mbit/s | 5.370918 / 16.000529 s = 33.57% | 36.46 us |
| AP cycle 2 | 122.868 Mbit/s | 5.364921 / 16.000246 s = 33.53% | 36.46 us |

The STA host floor was 123.767 Mbit/s, with zero missing, reordered or
duplicate datagrams. The diagnostic sink samples `mcycle` locally and merges
one batch per 256 polls into the existing HIL task counters; it does not log or
perform cross-module atomics per packet poll. Hardware reported the configured
320-MHz CPU clock during the run.

The final source image CRC `0ef27cee` repeated both roles after the common
classifier refactor:

| Role/cycle | Delivered TX | Core0 radio residence |
| --- | ---: | ---: |
| STA run 1 | 124.139 Mbit/s | 5.488235 / 16.000358 s = 34.30% |
| STA run 2 | 124.139 Mbit/s | 5.502680 / 16.000333 s = 34.39% |
| AP cycle 1 | 123.163 Mbit/s | 5.312207 / 16.001769 s = 33.20% |
| AP cycle 2 | 123.237 Mbit/s | 5.302982 / 16.001290 s = 33.14% |

Both final STA runs delivered the exact same byte/datagram count, and both
host floors were above 124 Mbit/s. The 2.9-point difference between the first
and final STA actor ELF is real and repeatable for these layouts, but no cache
counter experiment isolated its cause. It is recorded as layout sensitivity,
not attributed to cache or treated as a reason to pin a magic function offset.
The architectural acceptance fact is the stable reduction from approximately
48% to 34.3--34.4% with higher throughput, while AP remains at 33.1--33.2% in
the exact same final ELF.

The final production correctness image (CRC `7007324e`, with the diagnostic
feature absent) then passed run `1788087347746-0027a3e4`: ten cold boots and
three teardown/reassociation cycles per boot, 30/30 reconnects total. CPU0
minimum free stack remained 25,516 bytes in every epoch. This proves repeated
task-slot reuse and exact owner restoration; the throughput run alone would
not prove that lifecycle property.

The cutover increases the diagnostic application image to 3,161,072 bytes and
the correctness image to 3,292,144 bytes, both inside the reviewed 4-MiB
application partition. The code-size cost is accepted only together with the
measured 16-point Core0 reduction and the reconnect proof. A future reduction
must preserve both HIL gates; merging the actor back into the nested STA
lifecycle is not an acceptable size optimization.

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
  cross-core allocator;
- STA lifecycle policy remains explicit, but its long-lived active packet
  runner no longer pays for the complete lifecycle poll topology.

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
4. Keep AP TX as an independent gate: observer-free throughput must remain at
   least 120 Mbit/s and low-overhead Core0 task residence below 40%. Do not
   infer duplex headroom from this one-direction result.
5. Keep the new STA TX task-residence gate at 120 Mbit/s or higher and below
   40% Core0, plus the 10-boot/30-reconnect ownership gate. Treat either
   failure as an active-DATAPATH execution regression.
6. Validate monitor and scanner work accounting at their finite service
   boundaries; they need the same truthful physical counters but not the
   saturated network protocol leaf.
7. Consider RX BA32 only after the common lifecycle is proven. BA32 may change
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

The correct current statement is: standalone STA and AP TX both demonstrate
123+ Mbit/s at 33.1--34.4% Core0 in the final same ELF; standalone AP RX
has reached the requested STA performance class; and concurrent STA+AP RX is
below 40% Core0 at the measured 65-Mbit/s aggregate offer. Duplex retains its
own performance and residence budget.
