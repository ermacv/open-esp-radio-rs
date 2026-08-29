# ESP32-S31 STA, AP and STA+AP datapath state

Date: 2026-08-29.

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
received the following correctness and ownership changes:

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
  parse.

These changes corrected a severe AP overload failure. They did not yet make
the AP service geometry identical to STA:

- `ap_rx_protocol_turn_limit()` restricts an idle turn to half of the
  negotiated BA window, so BA16 gives only eight protocol frames;
- `BoundedRxTurn` charges both observation passes and serviced frames against
  that bound, so a pass which services no frame still consumes turn budget;
- returning `BudgetExhausted` at this narrow boundary can force immediate
  continuation and prevent the recycled-append adaptive coalescer from
  accumulating a wide DMA frontier.

This is the leading measured architectural difference, not yet a proven sole
cause of the remaining AP CPU/load gap.

## Standalone AP evidence

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

Consequently, two facts are established:

1. the AP packet-loss/`BUFFER_FULL` failure was a software service defect and
   is corrected in the measured workloads;
2. AP has not yet demonstrated STA-class Core0 residence or DMA batching.

It is not established that the remaining gap is caused by RF conditions,
cache layout, checksum, BA width or an irreducible AP protocol cost.

## Concurrent STA+AP path

The paired owner in
`driver/adapters/embassy/esp32s31-wifi/src/roles/concurrent/rx_service.rs`
already shares the split-radio/network resources and routes staged ownership
to the station or AP role. Its service algorithm is nevertheless older than
the current standalone paths:

- it services DMA before draining existing protocol backlog;
- it performs per-frame role dispatch through the paired consumer;
- when staging is saturated, it re-enters DMA after each four consumed frames;
- its classifier performs a normalized parse before the chosen role parses
  the frame again;
- the standalone AP fused-turn correction has not yet been extracted as a
  common primitive and applied here.

The per-four-frame refill is a bounded correctness policy, but it is not the
STA adaptive batching architecture. Paired STA+AP must therefore not be
declared performance-equivalent merely because it uses the same ring and SPSC.

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

The implementation is not yet fully unified. Standalone AP and concurrent
STA+AP still encode different turn limits and service ordering from STA. The
remaining work should remove those role-specific orchestration differences
before micro-optimizing AP parsing, changing BA width or attributing the gap
to cache/RF behavior.

## Next work and proof gates

The next sequence is:

1. Repair the diagnostic-task-poll linker layout by reducing or relocating
   diagnostic-only internal-SRAM ownership. Do not alter the production
   datapath to make the profiler link.
2. Add a matched, low-overhead standalone AP RX-only ceiling/residence run.
   Compare AP and STA on the same free channel, HT40/MCS7 policy, payload,
   duration, route and instrumentation class.
3. Replace the AP idle half-BA limit and observation-pass accounting with the
   same 32-frame fused budget used by STA, while retaining the four-frame
   active-TX fairness bound.
4. Extract the fused `pre-drain -> DMA -> post-drain` lifecycle and result
   mapping into one role-neutral primitive used by both STA and AP. Role code
   should provide admission/dispatch, not redefine DMA continuation policy.
5. Move concurrent STA+AP to that primitive; remove its DMA-first/per-four
   refill special case and avoid the duplicate normalized parse.
6. Re-measure before optimizing `admit_rx_data`, peer accounting or security
   parsing. Optimize only components which remain expensive in normalized
   cycles per serviced MPDU.
7. Characterize AP TX separately. Historical AP TX is about 113 Mbit/s versus
   STA near 120 Mbit/s; measure terminal completion to next aggregate
   publication rather than inferring a blocker from throughput alone.
8. Consider RX BA32 only after the common lifecycle is proven. BA32 may change
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

Until these gates pass, the correct current statement is: standalone AP is
functionally stable under the measured ceiling load, but its Core0 batching
and production residence are not yet equivalent to STA; concurrent STA+AP is
still one lifecycle revision behind standalone STA/AP.
