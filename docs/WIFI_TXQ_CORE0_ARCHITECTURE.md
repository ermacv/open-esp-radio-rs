# Wi-Fi TXQ and Core0-load architecture audit

Status: working architecture decision, 2026-08-31. This document separates
measured facts from design conclusions. It is not qualification evidence.

> **Current planning status:** the implemented AP-only burst candidate/grant
> shadow, its same-ELF cost and the next radio-wide Xarxa/Core0 refactor stages
> are defined in
> [`WIFI_EGRESS_REFACTOR_CHECKPOINT.md`](WIFI_EGRESS_REFACTOR_CHECKPOINT.md).
> Later references here to a not-yet-implemented candidate stream are retained
> as historical context, not as the current plan.

## Decision

The target is achieved without growing DMA SRAM and without constructing a
complete intermediate Ethernet frame. The production stack-side indexed path
now delivers 119.85--120.64 Mbit/s in two consecutive reset-isolated
observer-free gates (12/12 cycles); the matching resolved-link coarse image delivers 119.75--119.79 Mbit/s
with 38.94--39.10% Core0 residence using the existing 67 physical TX credits.
It keeps UDP payload ownership in the stack's PSRAM packet arena, selects a
resolved egress queue first, and performs the ordinary final Ethernet/IP/UDP
emission directly into an internal-SRAM TX token. There is no additional
complete-frame PSRAM-to-SRAM materialization.

The older 98-slot direct A/B delivered about 121 Mbit/s with about 34% Core0
residence, but spent 52,576 additional internal-SRAM bytes to hide an
interleaved producer queue. The staged 67-slot TXQ also recovered
119.62--119.97 Mbit/s, but copied every completed Ethernet frame and raised
Core0 residence to 68--69%. Those controls remain useful because they isolate
the two constraints. The accepted direction is now the third point in the
design space: queue selection before final backing plus direct construction.

The 68--69% Core0 result is not an intrinsic cost of the radio, CCMP or
BlockAck path. It belongs to the current scheduled staging implementation,
which promotes one PSRAM frame at a time on Core0 and performs a complete
ownership transaction around every copy. TXQ topology and materialization
placement are separate decisions: the former is now supported by throughput
evidence, while the latter still fails the Core0 gate.

The original proposal to move complete-frame materialization wholesale to
Core1 was tested and rejected. It reduced Core0 residence below 40%, but moved
the ceiling to Core1 and reduced throughput to 90.62 Mbit/s. Production must
instead preserve the successful ordering:

1. keep transport payloads in a bounded, independently removable PSRAM packet
   arena;
2. classify queued handles before internal-SRAM/DMA admission;
3. select one eligible queue by hierarchical airtime policy and dequeue a
   bounded burst;
4. construct that burst directly into the fixed global internal-SRAM working
   set;
5. validate the resolved `VIF + peer generation + TID/AC` at the device/radio
   boundary;
6. retain complete-frame staging only as a diagnostic/fallback experiment,
   not the normal data path;
7. keep both the 98-slot direct path and 67-slot one-copy path as causal
   controls.

This is the topology of mac80211 software TXQs adapted to the S31 memory
constraint. The Linux implementation itself is not reusable, but its
`classify -> software TXQ -> airtime selection -> burst dequeue -> short
hardware queue` boundary is applicable.

A bounded Xarxa egress change is now justified and has passed the two-client
performance gate. It need not teach Xarxa Wi-Fi association policy: Xarxa owns
removable packet handles and generic resolved-link queues, while the
driver/radio owns peer generations, airtime and hardware admission. The
current production selector coalesces IP queues that resolve to the same
generic link destination, but it is local to one UDP socket. The remaining
design work is an interface-wide grant boundary across sockets/protocols,
driver-side `VIF + peer generation + TID/AC` validation and measured airtime
accounting. Neither Core1 CPU copy nor AXI-GDMA removes enough total work to
substitute for that ordering.

## Measured state

All percentages below are task residence, not an assumption derived from
throughput. The relevant two-client HT40 runs are recorded in
[`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md).

| Path | DMA slots | Aggregate throughput | Core0 radio | Core1 network + UDP TX | Established result |
| --- | ---: | ---: | ---: | ---: | --- |
| direct A/B | 98 | 120.77--121.39 Mbit/s | 33.91--34.15% | 61.16--61.22% | the radio path can sustain the target below 40% Core0 |
| former FIFO production profile | 67 | 90.26--90.91 Mbit/s | not instrumented | not instrumented | post-lifecycle-control gate confirms early-admission fragmentation independently of stale credits |
| IP-indexed production profile | 67 | 119.56--120.13 Mbit/s | observer-free | observer-free | six of six post-cutover cycles pass with equal flows and exact delivery |
| resolved-link indexed production profile | 67 | 119.85--120.64 Mbit/s | 38.94--39.10% in matching coarse image | observer-free in production gate | 12 of 12 post-lifecycle-hardening cycles pass with equal flows and exact delivery |
| saturated plus sparse peer, clean residence image | 67 | 118.68--119.12 Mbit/s | 39.94--40.15% | 79.34--79.93% | all ten sparse datagrams per cycle delivered; no BA32 wait; diagnostic timer overhead remains |
| direct diagnostic | 67 | 96.61--97.20 Mbit/s | 39.41--39.56% | 49.55--50.14% | earlier residence A/B of the same small-pool topology |
| scheduled PSRAM staging | 67 | 119.62--119.97 Mbit/s | 68.43--69.02% | 63.75--64.70% | corrected TXQ recovers aggregate throughput but one-frame promotion remains too expensive |
| scheduled batched PSRAM staging | 67 | 116.12--117.47 Mbit/s | 57.54--58.04% | 65.91--66.65% | typed batching removes about 11 points of Core0 work but remains above the gate |
| scheduled Core1 materialization | 67 | 90.62 Mbit/s | 32.66% | 72.18% | same ELF; moves the ceiling to Core1 and is rejected |

The 98-slot direct result is a causal A/B baseline, not the configuration
shipped by `main` and not a scalable design. It uses
31 additional 1,696-byte internal-SRAM slots, or 52,576 bytes, to provide
look-ahead for two interleaved peers. Extending that rule by one BA window per
peer cannot support the AP client limit.

The staged path proves the complementary fact: a peer-homogeneous software
queue can recover full A-MPDU throughput with the 67-slot physical pool. Its
failure is CPU placement and transaction granularity, not its queue topology.

### Phase-5 direct pre-DMA selection: causal result

The stack-level selection question is now closed by a same-ELF discriminator
which changes only the order in which two already classified UDP destinations
ask for the existing 67 direct-DMA credits. The grouped path emits a bounded
32-datagram run for one destination before moving to the other; the control
retains the former `A, B, A, B, ...` order. Neither path has a PSRAM frame
staging pool, a promotion copy, a larger SRAM pool, or a different radio
configuration.

The low-overhead coarse runs used runtime CRC-32 `cbec3f52`:

| Path | Run | Throughput | Core0 cycles/wall | Core0 cycles/datagram | publish cycles/datagram | service cycles/datagram |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| grouped direct-67 | `1788136572078-00352034` cycle 0 | 119.550 Mbit/s | 36.746% | 11,582.6 | 2,698.2 | 1,380.2 |
| grouped direct-67 | `1788136572078-00352034` cycle 1 | 119.521 Mbit/s | 36.678% | 11,564.1 | 2,672.4 | 1,385.5 |
| interleaved direct-67 | `1788136800699-00352c3d` cycle 0 | 92.653 Mbit/s | 41.441% | 16,854.5 | 4,382.5 | 1,407.5 |
| interleaved direct-67 | `1788136800699-00352c3d` cycle 1 | 91.911 Mbit/s | 41.137% | 16,865.8 | 4,393.1 | 1,419.4 |

Both grouped cycles delivered each flow at 59.75--59.77 Mbit/s with exact
`missing/reordered/duplicates = 0/0/0`. The grouped path therefore reaches the
two-client radio ceiling and the less-than-40-percent Core0 goal with the
fixed small SRAM pool. Per datagram it removes about 31% of Core0 cycles. TX
service/BlockAck work is nearly unchanged; the directly measured publication
cost falls by about 1.7k cycles/datagram. Total retired instructions fall from
about 4.43k to 3.88k per datagram and CPI falls from 3.80 to 2.99, so the
remaining improvement is both less scheduler/control work and fewer stalls.

The more intrusive task-poll pair, runtime CRC-32 `b225f7f3`, explains the
publication result. In the stable grouped cycle, all 4,974 aggregates were
BA32 and scheduler return-to-loop time was only 53.8 ms. The interleaved
control also eventually formed almost exclusively BA32 aggregates, but spent
2.53--2.78 seconds in scheduler return-to-loop and 3.39--3.61 seconds between
completion and the next publication. It needed roughly 24--27k scheduler
passes for only 3.9k aggregates, versus roughly one pass per aggregate for the
grouped path. The defect is therefore not merely a smaller final A-MPDU: early
physical admission creates per-peer head-of-line gaps which idle the radio
while the selected queue waits for enough matching DMA owners.

This establishes the production boundary precisely: select the peer/TID queue
before requesting final SRAM backing, then construct the selected burst
directly into the existing physical pool. The HIL producer grouping is only a
causal discriminator; it is not the production fairness scheduler.

The first coarse build also exposed a feature-composition defect. The coarse
feature inherited the complete task-residence image and therefore linked the
staging, Core1-materializer and PSRAM-DMA A/B owners into a supposedly
production-like image. The protected 8-KiB bootstrap handoff assertion rejected
the resulting SRAM layout. Coarse telemetry now depends only on the shared
connected-datapath poll observer and its own phase counters. The successful
image retained the safety margin; no production TX/RX pool was reduced.

### Shared-pool lifecycle defect and repair

The first repeatability run of the direct pre-DMA selector exposed two stable
states in one ELF: one cycle sustained about 119 Mbit/s with BA32 while another
sustained about 102 Mbit/s with BA31. PHY evidence was identical (HT40, MCS7,
SGI), the host reported no loss or reordering, and the radio reported no retry
or failed-frame pressure. Treating this as radio or cache variance would have
been incorrect.

An exact ownership snapshot was added for every one of the 67 physical TX
credits:

```text
free
+ ready for the sampled VIF
+ ready for other VIFs
+ ingress reserves
+ application reserves
+ Core1 tokens in flight
+ radio owned
= 67
```

Run `1788154469086-0038d985`, runtime CRC-32 `cc29d8d4`, localized the slow
state. Across 4,405 partial-frontier samples, the queue contained 8,812
foreign-VIF owners, or 2.0005 per sample. The two ingress reserves were also
present as expected; they were not the defect. The two unexpected owners were
unclaimed frames left in the stopped STA VIF's ready queue. Because AP and STA
share one physical SRAM pool, those stale owners reduced the AP standby
aggregate by two credits. The slow cycle therefore produced 4,341 partial
aggregates out of 4,342 and delivered 102.238 Mbit/s. The fast cycle produced
5,059 full aggregates out of 5,060 and delivered 119.163 Mbit/s.

The lifecycle repair reclaims every unclaimed ready owner belonging to an
inactive VIF whenever either network link changes state. It deliberately does
not steal an active/prepared radio owner: the radio role must first complete
its typed terminal stop boundary. A focused host regression publishes stale
STA owners, transitions STA down, and proves that every physical credit can
then be claimed by the still-active AP.

Post-repair run `1788154890211-0038f7f6`, runtime CRC-32 `24403552`, passed both
16-second cycles at 116.383 and 118.462 Mbit/s with exact host delivery and no
radio retries or failures. Cycle 0 produced 4,941 full aggregates and one
partial aggregate; cycle 1 produced 5,029 full aggregates and two partial
aggregates. No steady partial frontier remained, and the sampled partials had
zero foreign-VIF ready owners. Core0 radio-task residence was 37.75% and
38.39%, respectively.

This establishes a correctness rule for the fixed shared pool: association
and link lifecycle must reclaim per-VIF software backlog independently of the
radio ring lifecycle. Focused regressions cover all three relevant orderings:

1. a ready owner exists before link-down and shutdown reclaims it;
2. link-down drains first while a previously issued, synchronous
   `TxToken::consume()` publishes afterward, and the publisher reclaims it;
3. a stale network poll acquires and consumes a token while the VIF is already
   inactive, and publication-time link state still rejects the ready owner.

Publication-time state is authoritative; there is no issue-time exception.
Shutdown-side and publisher-side cleanup therefore cover the complete
software frontier without stealing a prepared/radio-owned frame. The active
hot path pays one atomic state test. Increasing the pool or adding per-peer
SRAM would only hide this ownership defect.

### Production cutover

The former runtime `DirectDmaStackEgressQueueDiagnostic` switch has been
removed. The S31 Wi-Fi product composition now enables the maintained keyed
egress API and the pinned adapter returns an `EgressSchedule` with a
32-packet run limit while the link is up. All HIL Wi-Fi images use an
indexed 128-packet CPU-owned UDP backlog in PSRAM; the physical DMA-visible
SRAM pool remains 67 slots. At the start of a burst Xarxa resolves queued IP
destinations into routes and asks the device for their physical scheduling
domain. STA coalesces all Ethernet routes into its single radio peer. AP
resolves authorized unicast destinations through a Core0-published 15-entry
peer directory and keys them by slot plus association epoch; unknown and
group destinations retain their complete Ethernet identity. An unresolved IP
destination retains its IP identity so neighbour discovery cannot be
starved. Final Ethernet/IP/UDP bytes are still constructed directly in the
SRAM slots, so the cutover introduces no complete-frame copy.

The observer-free pre-cutover control was
`1788155370026-00390a0c`, runtime CRC-32 `95328358`. All three independent
repetitions remained in the former FIFO ceiling at 90.261, 90.910 and 90.835
Mbit/s despite HT40 MCS7 SGI and zero retry/failure evidence. This cleanly
separates the earlier stale-VIF lifecycle defect from the persistent queue
geometry defect.

The first production-selector run `1788155980257-0039306f` recovered
119.12--120.63 Mbit/s in four recorded cycles, but its initial cycle delivered
104.77 Mbit/s. That run motivated the publisher-side half of the lifecycle
closure above; it is not a passing gate.

Post-closure observer-free run `1788156858836-003949fc`, runtime CRC-32
`a788ec7c`, passed all six cycles across three reset-isolated repetitions:

| Repetition/cycle | Aggregate throughput | Flow 0 | Flow 1 |
| --- | ---: | ---: | ---: |
| 1/0 | 119.675 Mbit/s | 59.838 | 59.837 |
| 1/1 | 120.084 Mbit/s | 60.042 | 60.042 |
| 2/0 | 119.561 Mbit/s | 59.781 | 59.780 |
| 2/1 | 120.135 Mbit/s | 60.068 | 60.067 |
| 3/0 | 120.084 Mbit/s | 60.042 | 60.041 |
| 3/1 | 119.974 Mbit/s | 59.988 | 59.987 |

Every flow reported zero missing, reordered and duplicate datagrams; the
OpenWrt link reported HT40 MCS7 SGI with zero retries and failures. Coarse run
`1788156398130-00393f2f`, runtime CRC-32 `1e8b0498`, independently delivered
119.370 and 120.013 Mbit/s. It formed 5,067/5,070 and 5,095/5,097 full BA32
aggregates. Exact ownership sampling saw only one transient foreign ready
credit over both cycles, not a retained inactive VIF. Core0 radio residence was
39.65% and 39.76% of the 320-MHz wall budget.

The first resolved-link implementation still exposed intermittent slow
states. Run `1788158436660-0039884f` used one runtime CRC and produced two
ceiling cycles (120.43 and 120.15 Mbit/s), then 102.80 and 88.98 Mbit/s. The
last two cycles still reported HT40 MCS7 SGI; one had one retry/failure on an
irrelevant tiny reverse-link packet and the other had none. The slow state was
therefore not attributable to PHY rate or retry pressure. Coarse run
`1788158648577-00398e99`, runtime CRC-32 `c5111d0c`, proved that the resolved
selector itself can sustain 119.748 and 119.790 Mbit/s at 39.10 and 38.94%
Core0 residence. It formed 5,083/5,086 and 5,085/5,088 full BA32 submissions.

The remaining lifecycle exception allowed a token issued after link-down to
publish into the inactive VIF. Publication now rejects inactive ownership
regardless of issue-time state. Post-fix production run
`1788159136670-0039994b`, runtime CRC-32 `6ba2f774`, passed all six cycles:

| Repetition/cycle | Aggregate throughput | Flow 0 | Flow 1 |
| --- | ---: | ---: | ---: |
| 1/0 | 119.848 Mbit/s | 59.924 | 59.924 |
| 1/1 | 120.390 Mbit/s | 60.195 | 60.195 |
| 2/0 | 120.149 Mbit/s | 60.075 | 60.074 |
| 2/1 | 120.139 Mbit/s | 60.070 | 60.069 |
| 3/0 | 120.121 Mbit/s | 60.061 | 60.060 |
| 3/1 | 120.171 Mbit/s | 60.086 | 60.085 |

Every cycle reported HT40 MCS7 SGI, zero retries/failures, and zero missing,
reordered or duplicate datagrams for both clients. An immediate second run of
the same image and CRC, `1788159543971-0039a175`, passed another six cycles at
119.926--120.641 Mbit/s with the same exact-delivery and radio evidence. The
post-fix result is therefore 12/12 cycles across six reset-isolated
repetitions. This closes the measured
two-client throughput and fixed-SRAM gate. It does not yet qualify 8--15-peer
fairness: the current Xarxa index has 16 IP queue keys, selection is local to
one UDP socket, and service is equal packet bursts rather than airtime. The
next evolution must coordinate all interface egress, validate the resolved
link address against current `VIF + peer generation + TID/AC`, charge estimated
airtime, and reconcile that estimate from BlockAck/rate/retry completion.

### Fifteen-client scaling prerequisites

The fixed DMA working set is no longer the first client-count limit, but two
software limits must be treated separately.

First, Xarxa's generated production configuration defaulted to only eight
neighbor-cache entries while the AP role supports 15 associated clients. The
ESP32-S31 product composition now enables an explicit Embassy forwarding
feature which selects 16 Xarxa neighbor entries. A compile-time assertion
guards that feature edge. The host regression learns all 15 unicast clients by
ARP, then revisits every client with UDP after the complete set is resident;
every frame uses the retained destination MAC rather than falling back to ARP.
This change does not alter the 67-slot DMA pool.

Second, software backlog depth determines whether every simultaneously
saturated peer can already hold one complete BA32 candidate. A resolved-link
selector test accepts 480 fully interleaved packets (`15 * 32`) and emits 15
contiguous BA32 runs without changing SRAM ownership. Its 128-packet control
contains only nine packets for the first peer and must rotate before BA32.
Therefore the current 128-packet HIL arena is sufficient evidence for two
clients, but it is not a 15-saturated-client qualification. Increasing this
capacity consumes PSRAM packet storage, not DMA SRAM, and must be paired with
bounded admission/AQM rather than made unbounded.

The 16-neighbor target image uses runtime CRC-32 `40d8806a`. Production HIL
run `1788160343102-0039b252` passed six two-client cycles at
119.027--120.342 Mbit/s. Single-client run `1788160604782-0039c8b3` passed six
cycles at 120.697--121.613 Mbit/s, so the larger neighbor state did not cost
the fast path. Coarse run `1788161083785-0039e75f`, runtime CRC-32 `e7661f35`,
measured:

| Cycle | Throughput | Core0 cycles/wall | Core0 task residence | Core1 network + UDP TX |
| --- | ---: | ---: | ---: | ---: |
| 0 | 119.790 Mbit/s | 38.42% | 39.20% | 82.31% |
| 1 | 118.702 Mbit/s | 38.10% | 38.87% | 81.68% |

Both cycles formed essentially only BA32 aggregates. This preserves the Core0
gate but identifies Core1 as the tight compute budget. Any multi-client
scheduler must therefore use O(1) active-set operations and charge service at
burst/completion granularity; per-packet full-peer scans or another payload
copy are not acceptable.

### Phase-4 Core1 materializer result

The final same-ELF comparison used application SHA-256
`8c671b73e0e25730047fa9131c354db40299a486e4525c81fe34cb9c05550e77`.
Only the runtime `tx_buffer` discriminator changed.

The Core0-copy control
`1788134164601-00349a6a/diagnostic-ap-two-client-udp-tx-psram-copy` passed two
16-second cycles at 116.12 and 117.47 Mbit/s. Core0 radio residence was 57.54%
and 58.04%. Core1 network plus UDP-TX residence was 65.91% and 66.65%.

The Core1-copy run
`1788133978130-00349300/diagnostic-ap-two-client-udp-tx-core1-materializer`
ran the complete traffic interval at 90.62 Mbit/s. It submitted and completed
3,863 batches containing 115,230 frames, or 29.83 frames/batch, with one
transient destination-credit shortage. Core0 radio residence fell to 32.66%,
but Core1 network plus UDP-TX residence rose to 72.18%. Per admitted datagram,
Core0 radio residence fell from roughly 58.2 to 42.5 microseconds while Core1
network plus UDP work rose from roughly 66.8 to 93.8 microseconds. The owner
handoff therefore increases total CPU residence as well as moving it.

This A/B establishes all of the following without inferring from throughput:

- the software TXQ still forms almost full BA32 bursts on the slow run, so the
  90.62-Mbit/s result is not a return of peer-interleaving fragmentation;
- moving the existing copy transaction to Core1 is sufficient to put Core0
  below 40%;
- Core1 has no remaining headroom for that transaction and the push-style
  stack, so the move cannot preserve the radio ceiling;
- the next production candidate must remove or avoid work, not merely move it
  between the two already busy cores.

The Core1 run ended with a socket-drain error and AP stop fault after the
measured interval. It is therefore negative diagnostic evidence, not a passed
qualification record. Earlier bring-up failures in `1788133353798-00342cb1`
and `1788133568822-00346085` exposed respectively a missing Core1 wake re-arm
and a stale synchronous scheduler invariant after an asynchronous completion.
Both ownership defects have focused regressions; neither result is included in
the performance comparison.

Fresh post-repair evidence is
`1788124533883-0032a684/diagnostic-ap-two-client-udp-tx-psram-copy`: both
16-second cycles completed at 119.62 and 119.97 Mbit/s, and both clients had
exact `missing/reordered/duplicates = 0/0/0`. Radio-task residence was 68.43%
and 69.02%. The matching observer-free production profile run
`1788124787568-0032b45f/access-point-two-client-ceiling-tx` executed three
independent repetitions and failed at 95.42, 95.58 and 92.97 Mbit/s. The two
paths use the same 67-credit resource constant; the difference is scheduling
before versus after physical DMA admission.

### Phase-1 promotion profile

The same-ELF two-client HIL run
`1788121638849-0031be17/diagnostic-ap-two-client-udp-tx-promotion-phases`
closed the per-frame promotion split with two independent 12-second traffic
cycles. Aggregate payload throughput was 117.85 and 118.68 Mbit/s, with zero
host-observed loss, duplication or reordering. Both cycles promoted every
attempted frame and reported no destination-credit shortage.

| Promotion phase | Cycle 0 cycles/frame | Cycle 1 cycles/frame | Instructions/frame |
| --- | ---: | ---: | ---: |
| credit claim | 177.65 | 177.53 | 62.9 |
| destination claim | 397.35 | 398.70 | 156.0 |
| payload copy | 6,513.09 | 6,535.09 | 975.1 |
| destination publication | 266.85 | 267.70 | 128.0 |
| source release | 721.94 | 718.17 | 254.7 |
| READY-to-RADIO claim | 358.14 | 359.71 | 181.0 |
| unattributed residual | 6.00 | 6.00 | 2.0 |
| **complete promotion** | **8,441.01** | **8,462.90** | **1,759.8** |

At 320 MHz the complete promotion transaction occupied 26.40--26.65
percentage points of one core during the measured intervals. Payload copy
alone occupied 20.37--20.58 points and 77.16--77.22% of promotion cycles.
The remaining ownership bookkeeping cost about 1,928 cycles/frame, or roughly
six core percentage points at this packet rate.

This establishes two boundaries. Batch claim/publication/source-return is
worth optimizing, but cannot by itself reduce the measured staged path from
about 61% to below 40% Core0. Conversely, copying is not an inferred residual:
it is the directly measured dominant phase. Typed batching is still required
because it creates the ownership unit on which Core1 CPU or AXI-GDMA can copy
while Core0 continues radio work.

### Phase-2 ordering regression and repair

The first staged multi-peer implementation violated its own FIFO contract on
the no-DMA-credit rollback edge. It removed an older frame from a peer queue,
called `try_promote()`, and appended the returned owner to the tail when no
internal-SRAM destination was available. Younger frames already in that queue
could therefore overtake it. Pair rollback repeated the error for both
speculatively removed frames. This is a software ordering defect, not packet
loss and not a BlockAck failure.

Clean-main task-residence runs `1788122780734-0031f197` and
`1788123001205-00322bc7` exposed 30,499 and 4,495 late flow-0 datagrams while
reporting zero terminal missing and duplicate datagrams. The corrected queue
has explicit `push_back` for new ingress and `push_front` for rollback; a pair
is restored in reverse insertion order to reconstruct the original prefix.
The host regression test reconstructs `10, 11` ahead of already queued
`12, 13` and requires dequeue order `10, 11, 12, 13`.

After the fix, staged task-residence runs
`1788124022670-0032890d/diagnostic-ap-two-client-udp-tx-psram-copy` and
`1788124533883-0032a684/diagnostic-ap-two-client-udp-tx-psram-copy` completed
all four 16-second cycles at 118.47--119.97 Mbit/s. Each client reported exact
`missing/reordered/duplicates = 0/0/0`. Core0 radio residence was
67.93--69.02%; this restores correctness and throughput but confirms that one
CPU copy per frame is not the production Core0 solution.

The direct task-poll control
`1788124170097-00329255/diagnostic-ap-two-client-udp-tx-task-poll` also had
zero host reorder while observing 9/3 partial BlockAck completions and 61/51
empty BlockAck completions. All 127,093/125,764 aggregate subframes were
ultimately acknowledged. Partial/empty BA activity is therefore present but
is not sufficient to produce the staged ordering failure.

That control initially could not link on clean `main`: task-poll inherited the
complete task-residence feature, including the staged packet pool, PSRAM-DMA
probe and AXI-GDMA probe, even though task-poll scenarios cannot select them.
The runtime now has a small shared connected-datapath poll feature; residence
keeps its same-image TX probes, while task-poll enables only the poll observer
and driver observation. The control then linked with all SRAM placement audits
passing. This was a feature-composition regression, not a request to reduce
the protected bootstrap margin.

### Phase-3 typed batch promotion

The AP standby path now selects one peer/TID burst, reserves all required
physical destinations before moving any source owner, copies the selected
frames, and commits the resulting DMA owners as one typed transaction. A
failed reservation restores the exact FIFO prefix. If only part of a BA
window can be backed at that instant, the selector creates a smaller valid
burst instead of waiting while an active radio transaction needs service.

The first whole-window implementation demonstrated why physical capacity must
bound selection before reservation. Run
`1788125737146-00330551/diagnostic-ap-two-client-udp-tx-promotion-phases`
entered 3,054 no-credit retries in its second cycle and fell to 101.46
Mbit/s. After exposing the instantaneous promotion capacity, run
`1788125996608-003313a5` had zero no-credit retries and bounded preparation
calls, but delivered only 113.56--116.41 Mbit/s.

That remaining regression was a producer-overlap defect in the first batch
contract. It returned each PSRAM source credit while copying, but deferred the
sole Core1 wake until the complete roughly 30-frame copy had finished. The
per-frame control instead exposed the first returned credit immediately, so
Core1 could refill software backlog concurrently with the remainder of the
Core0 copy. The corrected contract is edge-triggered: wake on a real
`free_staged: empty -> non-empty` transition. It normally emits one wake per
batch, but emits another if Core1 drains the pool and a later return creates a
new readiness edge.

The corrected HIL run
`1788127405823-00334558/diagnostic-ap-two-client-udp-tx-promotion-phases`
delivered 118.05 and 118.04 Mbit/s. Both flows reported exact
`missing/reordered/duplicates = 0/0/0`, every promotion succeeded and no DMA
credit retry occurred. Complete promotion cost 8,096 and 8,102 cycles/frame,
versus 8,441 and 8,463 for the one-frame control. Source return plus readiness
publication rose from the over-deferred batch's roughly 366 to 487--490
cycles/frame, but remained well below the control's 718--722 cycles/frame.
The measured result is therefore a roughly 4% promotion-cycle reduction with
the original phase-image throughput restored; batching does not remove the
dominant payload copy.

The matching low-overhead residence run
`1788127620162-00334be6/diagnostic-ap-two-client-udp-tx-psram-copy` delivered
119.05 and 117.56 Mbit/s with exact ordering for both flows. Core0 radio-task
residence was 57.34% and 56.83%, down from 68.43--69.02% for the corrected
one-frame promotion on the same image class. This establishes an approximately
11--12 percentage-point Core0 reduction for transaction batching, but it still
fails the less-than-40% production gate. The remaining materialization
placement work is therefore required, not optional cleanup.

The failed attempt to combine the complete driver observer with the coarse
phase image is recorded as run `1788127195458-00333962`: the linker correctly
rejected an overlap with the protected 8-KiB bootstrap handoff margin. That
overlay was removed. Coarse and observer-heavy measurements remain separate
rather than weakening the SRAM safety boundary.

### What the GDMA experiment establishes

The AXI-GDMA HIL copied cached PSRAM into uncached internal SRAM correctly at
64, 1,536, 4,032, 4,096 and 49,152 bytes. For one 49,152-byte batch:

| Operation | Cycles | Retired instructions |
| --- | ---: | ---: |
| CPU bulk copy | 87,345 | 34,701 |
| blocking GDMA | 173,047 | 36,154 |
| interrupt-driven GDMA | 177,278 | 13,605 |
| CPU copy plus identical next-batch preparation | 1,231,007 | 919,456 |
| GDMA overlapped with that preparation | 1,208,535 | 897,577 |

GDMA is about twice as slow as the CPU bulk copy in elapsed latency. It is not
a per-frame replacement for `copy_from_slice`. Its useful property is lower
CPU work while another independent batch is being prepared: the asynchronous
copy retired 62.4% fewer instructions than blocking GDMA, and the overlapped
experiment saved 1.8% elapsed cycles and 2.4% instructions relative to the CPU
control. A production decision needs the scatter/gather and radio-pipeline A/B;
the microbenchmark alone does not prove a throughput or residence benefit.

The later scatter/gather test used the actual aggregate geometry: 32 separate
1,514-byte PSRAM sources and 32 separate internal-SRAM destinations. Run
`1788129709172-003396eb` measured:

| Operation | Active cycles/batch | Wall cycles/batch | Retired instructions |
| --- | ---: | ---: | ---: |
| CPU scatter copy | 80,092 | 80,092 | 31,044 |
| asynchronous AXI-GDMA SG | 66,752 | 181,682 | 14,916 |

AXI-GDMA therefore saves only 13,340 active cycles per 32-frame batch, or
about 417 cycles/frame, while taking 2.27 times as long to complete. This is
only about 5% of the measured roughly 8.1-kcycle complete promotion cost. It
can be useful only if its latency overlaps an independently useful radio or
network turn. It cannot by itself move the measured Core0 residence from
roughly 57% below the 40% gate, so product integration is deliberately
deferred until the Core1 materializer A/B establishes whether asynchronous
pipeline overlap is actually available.

At 320 MHz, the measured direct-to-staged Core0 increase of roughly 27
microseconds per delivered datagram is about 8.6 thousand cycles. The bulk
copy measurement is about 2.7 thousand cycles per 1,536-byte frame. These are
not identical workloads, so subtraction is only a sizing inference, but it
shows that payload movement alone cannot explain the staged cost. Per-frame
claim, atomic state changes, source return, wake publication, cache work and
the changed preparation path must be measured separately.

## Current ownership path

The current staged AP path is:

```text
Core1
    Xarxa socket queue
        -> Driver::transmit() reserves one staged token
        -> TxToken::consume() constructs complete Ethernet in PSRAM
        -> per-VIF ready channel publishes an index

Core0
    claim next per-VIF frame
        -> inspect Ethernet destination
        -> AP power-save / peer validation
        -> retain nonmatching owners in ApActiveFrameQueues
        -> round-robin one peer/TID
        -> for every selected frame:
             claim one free internal-SRAM slot
             copy PSRAM -> SRAM
             publish slot NETWORK -> READY
             return PSRAM index and possibly wake Core1
             claim READY -> RADIO
             encode 802.11 + CCMP in place
        -> publish current or standby A-MPDU
```

Useful pieces already exist:

- complete software-frame ownership is independent from DMA ownership;
- `PinnedNetworkTxFrame::Staged` cannot be passed to Wi-Fi DMA;
- AP has bounded intrusive per-flow FIFO links instead of moving payload;
- active and standby A-MPDU arenas already permit look-ahead while a radio
  transaction is in flight;
- an exhausted DMA credit returns the exact staged owner without loss;
- speculative admission failure restores that owner at its original FIFO
  prefix rather than appending it behind younger frames.

The remaining architectural defects are:

1. `ApActiveFrameQueues` is AP-local and combines generic TXQ mechanics with
   AP peer, power-save and aggregate policy.
2. The per-peer queues are filled opportunistically while searching the one
   per-VIF FIFO, instead of being a first-class classified ingress stage.
3. `ApTxFlowKey` contains a MAC address and TID 0, but no VIF identity, peer
   slot or association generation. Reassociation of the same MAC must not
   inherit queued ownership or scheduler debt.
4. The current 16-flow capacity covers 15 peers only because AP data is TID 0.
   It is not a per-peer/per-TID design.
5. Round-robin queue selection is frame fairness, not airtime fairness.
6. Queued, selected and hardware-pending work are not charged as three
   independent resources.
7. Active, power-save and group traffic share one 66-owner arena without an
   explicit data/control/reserve admission policy.
8. `try_promote()` performs a complete copy and ownership transition for each
   frame, so a 32-frame aggregate executes 32 claims, 32 publications, 32
   source returns and up to 32 producer-wake decisions.
9. Core0 reads the staged payload for classification and materialization even
   though Core1 just constructed it.

## Exact mac80211 comparison

The mac80211 contract explicitly says its intermediate queues are per-station,
per-TID software queues whose purposes include short hardware queues and
fairness between stations and interfaces. The driver can ask for an eligible
queue with `ieee80211_next_txq()`, dequeue from that already selected queue,
and return it after the scheduling turn. See the upstream
[mac80211 software TXQ contract](https://github.com/torvalds/linux/blob/master/include/net/mac80211.h).

The ordering in upstream `tx.c` is important:

1. mac80211 identifies the receiver station and queue class;
2. it enqueues the already-owned `skb` in that station/TID's `fq_tin`;
3. `ieee80211_next_txq()` selects an eligible TXQ using station deficit and
   pending airtime;
4. `ieee80211_tx_dequeue()` removes a packet and charges estimated airtime to
   the below-scheduler pending total.

These boundaries are visible in the upstream
[enqueue/dequeue and airtime scheduler](https://github.com/torvalds/linux/blob/master/net/mac80211/tx.c).
AQL has separate low/high per-station limits and a global pending-airtime
threshold; it is a limit on work already admitted below the software
scheduler, not the software backlog size.

The mt76 driver demonstrates the expected burst behavior. After
`ieee80211_next_txq()` selects one queue, `mt76_txq_send_burst()` repeatedly
dequeues from that same TXQ until AQL or hardware capacity stops it, then kicks
the hardware queue once. It does not alternate hardware admission per frame.
See upstream [mt76 TX scheduling](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/mediatek/mt76/tx.c).
Iwlwifi's mac80211 path likewise drains the supplied TXQ until its hardware
queue is stopped; see
[iwlwifi `iwl_mvm_mac_itxq_xmit`](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/intel/iwlwifi/mvm/mac80211.c).

The applicable mapping is:

| mac80211 | S31 open-radio |
| --- | --- |
| owned `skb` above hardware admission | `SoftwareTxFrame` owning one complete PSRAM frame |
| per-STA/per-TID `ieee80211_txq` | `TxQueueKey { vif, peer_slot, generation, tid }` |
| active TXQ list | active bitmap plus intrusive queue list |
| airtime deficit | weighted per-VIF and per-peer deficit |
| AQL pending airtime | estimated airtime already in promotion/current/standby/radio ownership |
| driver burst dequeue | one `TxBurstPlan` from one queue |
| short hardware queue | fixed internal-SRAM slots plus current/standby A-MPDU |
| TX status | BA/retry result and actual-airtime reconciliation |

What does not transfer directly is equally important. Linux system memory can
normally be DMA-mapped after scheduling; S31 Wi-Fi DMA cannot consume the
tested PSRAM address. Espressif's own S31 guidance therefore separates the
upper-layer packet from static hardware TX buffers when PSRAM is enabled. See
the official [S31 Wi-Fi performance and buffer guide](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-guides/wifi-driver/wifi-performance-and-power-save.html).
The selected software owner must still be materialized into internal SRAM.

## Where the TXQ should live

The first production TXQ should live in this repository, at the boundary
between the completed network frame and Wi-Fi role processing. It should not
live in Xarxa and should not remain embedded in the AP role.

Xarxa is the IP/transport stack. Mac80211 is the 802.11 MAC layer. The proper
analogue of mac80211's station/TID classifier is therefore our Wi-Fi adapter,
which owns association generation, power-save state, BlockAck, keys and PHY
policy. Teaching Xarxa those concepts would invert the layering.

No third-party API change is required for post-emission classification.
`TxToken::consume()` already has the complete frame on Core1. Immediately
after the emission closure returns, the terminal open-radio driver can derive
a small immutable sidecar:

```text
PublishedTx {
    packet_index,
    length,
    destination,
    ether_type,
    traffic_class,
}
```

Only the sidecar crosses with the packet index. Core0 validates it against the
current role and converts it to a generation-bound `TxQueueKey`. It need not
scan PSRAM to classify every frame. The Ethernet/IP bytes remain the authority
for diagnostic validation, while the sidecar is the fast path.

With the current push-style network driver, the PSRAM software pool must hold
enough interleaved owners for active peers. In the worst deliberately
alternating case, obtaining a full BA32 backlog for one of `N` peers can
require approximately `32 * N` complete software frames. That is acceptable
as a bounded software-memory cost, not as internal-SRAM/DMA cost, but it needs
explicit byte, age and per-peer limits. The pool must be elastic and global;
it must not statically reserve a complete BA window for every associated peer.

This is also where the current 67-entry staged experiment is incomplete for
8--15 saturated peers. It proves the two-peer topology but does not prove that
67 PSRAM frames are a sufficient software backlog for all required workloads.

## Target owner graph

```text
Core1 network
    construct complete Ethernet in SoftwareTxFrame (PSRAM)
    derive immutable egress sidecar
                |
                | affine SPSC publication
                v
Core0 Wi-Fi TXQ ingress
    validate VIF + peer slot + generation + power state
    enqueue packet index in per-key intrusive FIFO
                |
                v
hierarchical scheduler
    VIF -> AC -> peer/TID weighted airtime DRR
    AQL-like pending-airtime and DMA-credit gate
                |
                v
TxBurstPlan (one queue, bounded frame count and airtime)
    owns source packet indices + reserved SRAM destination leases
                |
        +-------+----------------+
        |                        |
        v                        v
Core1 CPU materializer      AXI-GDMA materializer
    batch copy                  SG batch copy
        |                        |
        +-----------+------------+
                    v
PreparedTxBurst (internal SRAM owners)
    Core0 802.11/CCMP encode
    standby A-MPDU publication
                    |
                    v
BA/retry completion
    release owners
    charge actual airtime
    correct pending estimate
```

The materializer is a strategy behind one ownership contract. CPU-on-Core1
and AXI-GDMA must be same-functionality A/B modes, not separate queue designs.
Core0 CPU-copy remains a control.

The radio already has current and standby A-MPDU arenas. The promotion API
must exploit that: start preparation for the next aggregate while the current
exchange is in flight. An asynchronous materialization must not be awaited in
a function which owns a terminal TX service obligation. It needs an explicit
`PromotionInFlight` state and a completion event alongside TX IRQ, RX service
and deadline events.

## Resource model and fairness

Three independent credits are required:

1. `QueueCredit`: bounded PSRAM packet/byte/age ownership per VIF and peer.
2. `AirtimeCredit`: scheduler deficit and estimated pending airtime.
3. `DmaCredit`: fixed internal-SRAM owners for current/standby/control work.

Queue admission must leave a control reserve. ARP, NDISC, EAPOL, management,
BAR/BA and beacon-related traffic cannot depend on bulk data credits. A
sleeping peer may retain bounded PSRAM backlog but is removed from the active
airtime set and owns no persistent bulk DMA reservation.

The initial scheduler can preserve current round-robin decisions while the
owner graph changes. The fairness cutover follows only after counters agree.
The final policy is hierarchical because an AP VIF with many clients must not
receive `N` times the physical-radio weight of a simultaneous STA VIF:

```text
physical radio
    -> VIF weighted DRR
        -> access category
            -> peer/TID airtime DRR
                -> one bounded aggregate burst
```

Admission charges estimated airtime when a frame enters the below-scheduler
promotion/current/standby frontier. Completion replaces that estimate with
actual PHY/retry airtime. A slow or retrying peer therefore consumes more
deficit with fewer frames and cannot pin the SRAM working set.

## How deeply Xarxa must change

### Level 0: no Xarxa change — recommended production baseline

Construct complete Ethernet frames in the existing staged PSRAM token,
classify after `consume`, queue owners in the Wi-Fi adapter, then materialize a
selected batch. This is sufficient for correct software TXQs, fixed DMA SRAM
and airtime fairness. It retains one additional PSRAM-to-SRAM materialization
relative to the direct path.

### Level 1: optional pre-emission egress hook — bounded experiment

The local sibling prototypes add a default `TxToken::set_egress_meta()` hook
to Xarxa and Embassy. It reports destination after route/neighbour resolution
but before `consume`. It can support:

- per-peer software-queue admission before choosing final backing;
- an affine direct-SRAM grant for a selected/empty-backlog peer;
- avoiding a PSRAM write for proven direct-bypass hits.

It cannot make Xarxa emit “the next packet for selected peer A”; Xarxa has
already chosen the socket packet before the hook. It therefore cannot replace
the driver-side software TXQ. Missing middleware forwarding silently disables
the optimization, so the fork has a real maintenance cost. Keep it only if a
same-ELF HIL demonstrates a useful direct-hit rate or admission benefit.

The main repository still pins published Xarxa and Embassy revisions; the
hook prototypes in the sibling working trees are uncommitted and are not a
production dependency.

#### Egress-hook experiment result: rejected for the current architecture

The same-ELF experiment reserved concrete internal-SRAM owners for one
destination and let `TxToken::set_egress_meta()` switch a token from its
mandatory staged fallback to that owner. A single tagged publication frontier
preserved ordering between direct and staged frames.

The first implementation repeatedly revoked and reissued the horizon. Run
`1788130917970-0033cc55` exposed the resulting churn: its first cycle delivered
only about 69.3 Mbit/s, issued 33,207 grants and revoked 26,546 of them. After
changing the contract to append/top-up one affine destination horizon, run
`1788131321832-0033dbfb` was exact and delivered 118.76 and 118.18 Mbit/s, but
only 8,314/161,369 (5.15%) and 1,963/160,589 (1.22%) emissions used direct
SRAM. Core0 radio-task residence was 64.09% and 63.43%, worse than the roughly
57% staged-batch control.

This result does not reject software TXQs. The fixed-67-slot TXQ remains the
causal recovery from roughly 95 to 118--120 Mbit/s by restoring per-peer burst
geometry. It rejects the pre-emission hook as the foundation for avoiding the
copy: a push-style IP stack has already selected the socket packet before the
destination hint is known, so a one-destination Wi-Fi grant cannot steer most
emissions from two simultaneously saturated flows. Reducing staged admission
also shortened the useful backlog and increased Core0 work. The hook and its
third-party dependency delta must not enter production. A future direct bypass
may be reconsidered only after the queued path is complete and only with a
new measured benefit.

### Level 2: owned UDP packet storage — deep, later A/B

Xarxa's current UDP TX ring exposes a borrowed payload during dispatch. It
cannot transfer an independently returnable packet owner. An owned path would
need a fixed packet pool with header headroom, an acquire/write/commit socket
API, route/fragmentation cancellation returns and a driver-facing submission
contract. It can remove the socket-ring-to-complete-frame payload copy when an
application writes directly into the owner. It does not remove the required
PSRAM-to-SRAM Wi-Fi materialization.

This change is UDP-specific. TCP must retain unacknowledged stream bytes and
may segment them differently on retransmission, so it needs a separate design.

### Level 3: pull-selected Xarxa egress — largest change, now justified for A/B

A true stack-level pull API would let the radio grant a peer/TID and ask Xarxa
to materialize a matching queued transport packet directly into SRAM. That
could avoid the complete PSRAM frame for direct hits, but it changes socket
polling, route/neighbour handling, fragmentation, the driver trait and every
middleware adapter. It also couples a generic IP stack to link-scheduler
selection. Consider it only if bounded PSRAM TXQs plus batch materialization
meet Core0 but fail Core1 or memory/latency gates at 8--15 clients.

The Core1 A/B has now met that trigger, and a source audit makes the required
depth precise. Xarxa's `Interface::socket_egress()` iterates sockets, then a
UDP socket calls `PacketBuffer::dequeue_with()` on the head of its single TX
ring. Only inside that callback does `dispatch_ip()` perform route/neighbour
resolution. `device.transmit()` is currently called before that resolution.
The experimental `set_egress_meta()` moved the resolved MAC before
`TxToken::consume()`, but it did not move it before FIFO packet selection.

That distinction is fundamental for the two-client workload: one UDP socket
contains alternating destinations. A driver grant for peer A cannot make the
stack skip the peer-B packet at the FIFO head. A true no-copy queued design
therefore needs one of the following explicit contracts:

- per-egress-key packet queues (or a removable indexed packet pool) above
  `dispatch_ip`, so the radio scheduler can select a key and dequeue one of its
  packets;
- a driver admission predicate supplied before `dequeue_with`, with socket
  storage able to locate a matching packet rather than only its head;
- an owned datagram API where the producer classifies and transfers a packet
  owner into a per-key queue before network dispatch.

Changing only `embassy-net-driver::TxToken` cannot provide this selection. It
can choose backing for the packet Xarxa already chose, which explains the
measured 1.22--5.15% direct-hit rate but cannot reproduce mac80211's
`next_txq() -> dequeue burst` order.

#### First pull-selection A/B (2026-08-31)

A diagnostic Xarxa policy now selects a bounded run from one UDP destination
before `device.transmit()` claims a final SRAM slot. The HIL producer remains
strictly interleaved, the physical direct-DMA pool remains 67 slots and no
PSRAM-to-SRAM staging copy is introduced. The implementation is deliberately
diagnostic: it selects by IP destination and removes entries out of a single
packet ring with tombstones. It is not the production peer/TID scheduler.

Run `1788138098810-00356779` proved the semantics after removing an accidental
quadratic prefix walk: both 16-second cycles delivered every datagram in exact
per-flow order and reached 117.95 and 119.54 Mbit/s aggregate. Thus packet
selection before SRAM/DMA admission can recover the two-client ceiling without
growing the physical pool and without the rejected terminal frame copy.

The less intrusive Core0 run `1788138481897-00357795` had one disturbed first
cycle at 80.90 Mbit/s and one saturated cycle at 119.45 Mbit/s. The saturated
cycle measured:

- Core0 radio residence 42.02%, 13,255 cycles and 3,929 instructions per
  delivered datagram, CPI 3.373;
- TX publication 2,510 cycles/datagram and TX service 1,386 cycles/datagram;
- `TxToken::consume` 13,178 cycles/datagram, of which frame emission accounted
  for 12,410 cycles/datagram.

That result must not be compared causally with the earlier grouped-producer run
`1788136572078-00352034`, which used runtime CRC `cbec3f52` and reached 119.55
Mbit/s at 36.75% Core0. Adding the diagnostic stack policy changed code layout
and produced CRC `5519b09f`.

The required same-ELF grouped control is run `1788138993039-00358bcd`. On CRC
`5519b09f` it reached 119.19 and 119.84 Mbit/s; the second cycle measured
42.05% Core0, 13,223 cycles and 3,915 instructions per datagram, CPI 3.377.
Publication/service were 2,472/1,399 cycles per datagram. Against that control,
the stack selector's 42.02%, 13,255 cycles, 3,929 instructions and CPI 3.373
are effectively equal. The selector therefore has no demonstrated Core0 cost.
The 36.75-to-42-percent shift follows the compile-time feature/layout delta,
not the runtime queue algorithm, and must be localized separately.

Task-residence runs are retained only as diagnostic context. The stack-selected
run reported roughly 11.2 seconds in the network task plus 0.78 seconds in the
UDP producer per 16-second cycle, but the grouped control on the identical
instrumented CRC varied from 87.1 to 100.7 Mbit/s. Those absolute percentages
are too intrusive/layout-sensitive to serve as a production CPU verdict.

This A/B accepts the API direction and keeps the current storage algorithm only
as a bounded two-client diagnostic: it has not caused a measured Core0
regression, but its scan/tombstone complexity is unsuitable for 8--15 peers.
The scalable candidate still needs a removable indexed owner pool with O(1)
per-peer/TID heads, or equivalent per-key queues. It must schedule on the
resolved link peer plus generation and TID/AC, not UDP/IP destination. Selection
returns an affine admission grant before final SRAM allocation; the global
PSRAM packet capacity may scale with bounded backlog, while internal DMA SRAM
must remain independent of associated peer count.

A follow-up code-structure change restored the ordinary FIFO dispatch as a
separate function and moved the diagnostic destination selector into a
non-inlined function. This reduces the hot instruction working set rather than
choosing an address offset. The resulting same-ELF CRC `f504c14d` produced:

- grouped control run `1788139208263-00359389`: 118.95 Mbit/s at 37.69% Core0
  in its saturated cycle;
- stack-selected run `1788139424643-0035995c`: 120.01 Mbit/s at 38.50% Core0
  in its saturated cycle, with exact and equal delivery to both clients.

At comparable ceiling throughput, stack selection costs 0.81 Core0 percentage
points, 148 cycles and 20 retired instructions per delivered datagram. TX
publication/service are 2,449/1,358 cycles per datagram versus 2,490/1,400 for
the grouped control. The fixed-67-slot no-terminal-copy candidate therefore
passes the two-client throughput and Core0 gates. Both runs still contain a
separate 103--104 Mbit/s cycle with proportionally lower Core0 residence; that
variability is not CPU saturation and requires BA/retry/radio-idle evidence.

Reset-separated follow-up run `1788140099012-0035b54d` then produced two exact,
equal-flow cycles at 119.40 and 119.74 Mbit/s with Core0 residence 38.78% and
38.41%. They consumed 12,239/12,088 cycles and 3,913/3,894 instructions per
datagram. The application CRC changed to `0f669ce8`, but the emitted hot-code
discriminator remained `1344005468`; no claim is made that the hot layout
changed without a map comparison. Across two resets, all three saturated
stack-selected cycles after function isolation satisfy the `<40%` gate.

Task-poll run `1788139572072-00359e79` supplies that phase evidence without
being a ceiling measurement. Its three intrusive cycles were stable at
89.6--90.4 Mbit/s. Every one retained exact/equal flow delivery, 3,813--3,849
completed aggregates and 121,679--122,783 acknowledged subframes. More than
91% of aggregates stopped at the 32-frame limit; there were zero individual
retries, collisions and hardware timeouts, with only 7--11 partial BA samples.
The queue therefore supplies full BA32 work and the radio completes it.

The diagnostic itself inserted roughly 3.54--3.78 seconds total
completion-to-next-publication latency, including 2.88--3.07 seconds between
return and the next scheduler loop. That accounts for its lower ceiling and
confirms why task-poll images cannot be used as production throughput or CPU
verdicts. It also rejects queue starvation and BA breakage as the explanation
for the current selector path.

#### Compact indexed selector result

The next implementation replaced repeated packet-ring scans with stable
32-bit handles and one intrusive FIFO per destination. A handle names a live
metadata slot by wrapping sequence distance; it is neither an address nor an
owner. Each metadata entry stores its payload sequence and the next handle in
its destination FIFO. Successful out-of-global-order dispatch tombstones the
entry, while contiguous completed/padding prefixes reclaim the underlying
payload ring. Selection and per-destination advance are O(1); the one-time
index activation is bounded by the live socket queue.

Run `1788141182663-0035deb0`, application CRC `8b642927`, produced two exact
and equal-flow cycles:

| Cycle | Aggregate TX | Core0 cycles/wall | Core0 cycles/datagram | TX publication cycles/datagram | TX service cycles/datagram |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 118.791 Mbit/s | 37.877% | 12,015 | 2,560 | 1,422 |
| 1 | 119.490 Mbit/s | 37.851% | 11,937 | 2,514 | 1,391 |

Core1 retained 8,616 bytes of its 16-KiB stack at the worst reported stage,
above the 4-KiB requirement. The physical TX pool remained 67 slots.

The same-ELF grouped-producer control
`1788141408103-0035e4ff`, also CRC `8b642927`, did not supply equivalent radio
work: its cycles reached only 92.742 and 104.538 Mbit/s. The first consumed
44.49% Core0 and the second 33.03%. This is not evidence that an index has
negative cost; it proves that, in this layout and queue state, stack-side
selection itself is the mechanism which restores the peer-homogeneous
frontier. Comparing only high-throughput cycles across different CRCs would
miss that causal result.

The indexed prototype is now hardened against its generic failure modes. More
than sixteen simultaneously live destinations causes a temporary FIFO drain,
not a panic or a false socket-capacity error. FIFO/index policy changes clear
intrusive links transactionally. Host tests cover fifteen interleaved peers,
seventeen-peer overflow, failed emission with exact-head retention, enqueue
after lazy activation and FIFO re-entry.

The hardening build, CRC `c1427d9d`, repeated the result in
`1788141775143-0035f865`: 117.947 and 119.502 Mbit/s at 37.31% and 37.34%
Core0, again with exact/equal flows. Its current same-CRC unordered control
`1788141988693-0036014b` reached 119.583 Mbit/s at 37.71% in one cycle but
collapsed to 88.664 Mbit/s at 41.93% in the other. The indexed path therefore
does not exchange CPU for throughput; it removes the unstable inter-peer
fragmentation while retaining the fast cycle's Core0 cost.

A later fairness experiment tried to rotate away from the selected destination
whenever its emission callback returned an error. This was incorrect. In the
current stack the callback error most commonly means that the *global* SRAM TX
pool is temporarily exhausted, not that this destination is progressless. The
failed build `0d2eae6c` reached only 90.124 and 92.297 Mbit/s. Returning to the
exact hardened source restored application CRC `c1427d9d`; run
`1788142765796-003635ce` then reached 119.079 Mbit/s in its first cycle. That
cycle recorded 176,811 admission attempts and 161,793 successes: 15,018 normal
global-credit misses would have truncated/rotated the peer burst under the
rejected rule. The second cycle reached 103.016 Mbit/s, but retained the same
11.884 kcycles/datagram Core0 cost as the fast cycle's 11.881 kcycles/datagram.
It therefore remains a separate radio-work-rate variation, not evidence that
the Core0 implementation became more expensive.

The aggregate-observer control `1788143111421-00365b1a` directly checked the
radio transaction. Across three cycles it delivered 89.611--90.814 Mbit/s in
the deliberately intrusive telemetry image. Every one of 368,177 submitted
MPDUs was acknowledged; hardware timeout, collision and ordinary retry counts
were zero. Of 11,585 completed aggregates, 9,179 were full BA32 and almost all
remaining aggregates contained 31 MPDUs. Thus the current destination selector
can preserve full aggregation and the measured radio/BA path is healthy. The
observer image's throughput is not a production ceiling.

The next admission API must consequently distinguish at least:

- `GlobalExhausted`: retain the current queue and remaining burst exactly;
- `PeerDeferred`: keep that peer's head, rotate to another eligible key;
- `Granted`: consume one affine credit and emit into its SRAM token.

An untyped `Option<TxToken>` cannot express that distinction. Fairness must not
be implemented by interpreting every missing token as a peer-specific failure.

One storage defect remains before production cutover: out-of-order completion
can reclaim payload bytes only when the oldest global prefix completes. An
inactive/unresolvable peer can therefore pin PSRAM ring capacity even though
other per-destination queues continue to dispatch. The production packet
arena needs independently removable payload owners (or equivalent bounded
per-key storage). This is a PSRAM ownership/reclaim issue, not a reason to
return complete frames to the Core0 one-copy path.

### Core1 wake and stack-dispatch localization

The resolved-link cutover fixed the radio queue geometry, but the first coarse
observer still placed Core1 network plus UDP-TX residence close to 89% at the
two-client ceiling. That number could not be treated as an architectural
property: the observer performs relaxed atomic accounting in every network
poll and TX phase. A sequence of same-ELF runtime discriminators now separates
three possible causes without changing the 67-slot SRAM pool, BA window or
radio code.

First, the radio-credit return is already aggregate-batched. Run
`1788167082695-003aebb6` recorded 5,042 full BA32 runs and 5,042
`free_tx: empty -> non-empty` wakes in its first measured cycle. The second
cycle recorded 4,966 full runs and 4,965 wakes. Core0 therefore does not wake
Core1 once per MPDU. The approximately three network polls per aggregate seen
before the next change are cooperative-stack scheduling probes, not a
cross-core per-frame IRQ or wake storm.

Second, Embassy now suppresses socket-originated egress wakes while a
cooperative poll is already blocked on global device credit. The scheduled
credit probe and the real device/radio waker remain armed, so this does not
hide newly returned SRAM credits or ingress work. Same-image runs used
application SHA-256
`7d62a7db2551b1ebea6375baecf798dc1726e3da5c2edb8a0e35b2bec505f080`
and runtime-ELF SHA-256
`0e26246d40458261d980e8533661262bb663e92f843d09c76ddd338fbb390f76`:

- control `1788167605996-003b11ae` retained 20,031/20,065 network polls and
  14,980/14,998 credit exits per 16-second cycle;
- suppressed `1788167808992-003b1b5a` retained 15,169/15,166 polls and
  10,123/10,123 credit exits.

This removes about 24% of network polls and one third of credit exits, but
network residence improves by only about 0.20--0.22 seconds per cycle. It is a
correct bounded scheduling cleanup, not the main Core1 cost.

Third, Xarxa previously returned from UDP dispatch after every successful
packet. A saturated BA32 run therefore repeated interface/socket selection
roughly once per datagram even though the current resolved queue remained
eligible. `EgressSchedule` carries a separate socket dispatch quantum.
Production uses four packets per socket dispatch, while the longer 32-packet
run remains the independent fairness/aggregation limit. The network runner's
total per-turn egress budget is still 32; this change neither waits for BA32
nor makes one socket unbounded.

The single-dispatch control and four-packet path below are one binary:
application SHA-256
`a34e3031b33d99ac4ce9ada46a3a95378bb4d679e9cdbb90e7f23e2ea8f46f74`,
runtime-ELF SHA-256
`9a47427e29789fe8963c6f83f71e6be1e679f37b250c1412887bfe27a9503a90`
and runtime CRC-32 `b0b71004`.

| Policy/run cycle | Throughput | egress passes/token | Core0 radio | Core1 network + UDP TX | total task residence | total us/datagram |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| one packet, `1788168302477-003b5506` c0 | 118.296 Mbit/s | 1.0627 | 40.69% | 88.79% | 129.48% | 128.887 |
| one packet, `1788168302477-003b5506` c1 | 118.284 Mbit/s | 1.0626 | 40.72% | 88.94% | 129.67% | 129.073 |
| four packets, `1788168513976-003b5936` c0 | 118.392 Mbit/s | 0.3127 | 40.07% | 78.65% | 118.72% | 118.081 |
| four packets, `1788168513976-003b5936` c1 | 116.966 Mbit/s | 0.3126 | 39.62% | 77.70% | 117.31% | 118.093 |
| four packets, `1788168748311-003b5d61` c0 | 115.353 Mbit/s | 0.3128 | 39.19% | 76.70% | 115.88% | 118.295 |
| four packets, `1788168748311-003b5d61` c1 | 117.657 Mbit/s | 0.3126 | 39.95% | 78.33% | 118.28% | 118.378 |

The four-packet path reduces interface egress passes by 70.6% and total task
time per delivered datagram by 8.2--8.5%. Almost the entire saving is on
Core1: about 10.1 microseconds per datagram. Core0 work is not displaced
upward. All cycles retain two operational BA32 agreements, essentially only
full 32-frame runs, equal flows and no host-observed missing, reordered or
duplicate datagrams. The monitored OpenWrt link remains HT40 MCS7 SGI with
zero retries and failures.

The 115.35--118.39 Mbit/s spread in the four-packet runs must not be explained
as CPU cost: normalized total task time stays within 0.30 microseconds per
datagram across all four cycles. It is also not enough evidence to call the
spread radio interference, because only one of the two client links has an
external retry report. A later ceiling gate needs simultaneous evidence for
both clients. The causal conclusion here is narrower: repeated one-packet
stack dispatch was real removable CPU work, and removing it did not alter BA
geometry or transfer that work to Core0.

Observer-free production gate `1788170423975-003bb86e`, runtime CRC-32
`dad410e7`, then passed all six reset-isolated cycles at
118.514--120.540 Mbit/s. Both flows remained equal within one datagram and
reported zero missing, reordered and duplicate datagrams. The monitored link
remained HT40 MCS7 SGI with zero retries/failures. This proves that wake
suppression plus four-packet dispatch preserves the production ceiling; the
coarse observer's lower individual cycles are not a production regression.

A same-ELF four-versus-eight dispatch experiment then tested whether a larger
socket quantum could remove more Core1 work. Application SHA-256
`b8b2bf0efc1135391b4789211a223592290fc572699e23a06e4c3c0901483174`
and runtime-ELF SHA-256
`68d876c7b1f36204cfaf829529b49cf83df8d4534b5940e92f962d524a7c86bd`
were identical for both policies; runtime CRC-32 was `ef33161e`.

| Policy/run cycle | Throughput | egress passes/token | Core0 us/datagram | Core1 us/datagram | total us/datagram |
| --- | ---: | ---: | ---: | ---: | ---: |
| four packets, `1788170950081-003bd237` c0 | 117.935 Mbit/s | 0.3128 | 38.361 | 75.899 | 114.260 |
| four packets, `1788170950081-003bd237` c1 | 118.554 Mbit/s | 0.3126 | 38.278 | 75.900 | 114.178 |
| eight requested, `1788171159138-003bda88` c0 | 116.951 Mbit/s | 0.3126 | 38.243 | 77.285 | 115.528 |
| eight requested, `1788171159138-003bda88` c1 | 118.508 Mbit/s | 0.3128 | 38.340 | 77.298 | 115.638 |

The unchanged 0.3126--0.3128 passes/token proves that requesting eight at the
outer adapter did not change actual dispatch geometry: Xarxa's lower
cooperative egress quota stops the run at four. The eight-request control was
about 1.3--1.4 microseconds slower per datagram and did not improve
throughput. Its temporary protocol mode and scenario were removed. Production
therefore retains four as the effective cross-layer quantum. A future change
must alter and validate all participating budgets together, including sparse
peer latency and bidirectional ingress service; changing only the outer value
is neither an optimization nor a meaningful A/B.

### Saturated-plus-sparse service and diagnostic-image isolation

The first asymmetric two-client gate now exercises a saturated AP TX flow at
130 Mbit/s offered load while a second physical client receives two 1,472-byte
datagrams approximately every five seconds. The HIL protocol owns the pacing
group explicitly rather than approximating the workload with a high-rate
burst. The verdict requires at least eight secondary datagrams, no missing,
reordered or duplicate sequence numbers, and a host-observed maximum
inter-arrival of 5.5 seconds. Unequal offers deliberately disable the ordinary
throughput-skew verdict because equal delivered rates would be the wrong
fairness property for this workload.

Coarse run `1788172342431-003c5c6b`, runtime CRC-32 `6b15fabf`, passed both
22-second cycles:

| Cycle | Saturated flow | Sparse delivery | max sparse inter-arrival | Core0 radio | Core1 network + UDP TX | BA32 full / partial |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 118.526 Mbit/s | 10/10 | 5.002681 s | 38.55% | 70.22% | 6,915 / 11 |
| 1 | 118.553 Mbit/s | 10/10 | 5.003241 s | 38.59% | 70.32% | 6,921 / 6 |

The sparse peer therefore neither starves nor waits for a full BA32 under this
controlled load. Rare traffic is emitted in the small number of partial
aggregates while the saturated peer retains essentially full aggregate
geometry. This is a service/correctness result, not yet an airtime-fairness or
queue-latency proof: the current host timestamp bounds delivered inter-arrival,
not enqueue-to-radio residence, and only the sparse client's external link is
monitored independently.

The first task-residence run, `1788172754982-003c770a`, appeared to contradict
the coarse result: it reported 45.56--45.61% Core0 and 86.48--86.62% Core1 at
118.67--118.99 Mbit/s. The contradiction was in image composition. The
nominally minimal `task-residence-telemetry` feature also compiled the rejected
staging-copy, Core1-materializer, PSRAM-DMA and AXI-GDMA implementations. Their
runtime switches were disabled, but their branches, DMA owners and linked code
layout remained in the ELF. The image was 3,226,688 bytes and cannot be used as
a production CPU budget.

The feature graph is now fail-closed:

- `task-residence-telemetry` contains only connected-datapath poll timing;
- invasive backing/materialization controls live in the separate
  `tx-architecture-probes` feature and `diagnostic-tx-architecture` image;
- staging, Core1-materialization and PSRAM-DMA scenarios use that image;
- the image capability fingerprint distinguishes the two classes, so a
  scenario cannot silently reuse the wrong binary.

Clean task-residence run `1788173462651-003c8a58` used a 3,161,152-byte
application, exactly 65,536 bytes smaller than the contaminated image, with
application SHA-256
`37ef73f320af02993e488f22b695b90ebfce2c3a1f52e0982fbe367a5198a5ca`
and runtime-ELF SHA-256
`cb3acfd819c2ba9fefe1c3770efa1cabb74069ddb3169c25a82dc34fff8bc35a`.
It also passed both cycles:

| Cycle | Saturated flow | Sparse delivery | max sparse inter-arrival | Core0 radio | Core1 network + UDP TX | total two-core residence | total us/datagram |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 118.682 Mbit/s | 10/10 | 5.002813 s | 39.943% | 79.342% | 119.285% | 118.353 |
| 1 | 119.120 Mbit/s | 10/10 | 5.002981 s | 40.145% | 79.925% | 120.070% | 118.694 |

Relative to the contaminated image, clean composition removes
11.97--12.95 percentage points of summed two-core residence and about
12.3--12.7 microseconds per delivered datagram without lowering throughput or
changing sparse delivery. The remaining difference between coarse and clean
task-residence images is poll-timer instrumentation and code shape; it must not
be labelled production work. Observer-free throughput gates remain the ceiling
authority, while the clean residence class is the current closest bound on
where CPU time is spent.

Observer-free run `1788174309479-003cb51a`, application SHA-256
`3568c4524beefa8e5a09b0b8133426a9de26817f315f2762d9e922f2826e2393`,
then passed all six reset-isolated cycles at 117.862--119.448 Mbit/s for the
saturated flow. Every cycle delivered ten sparse datagrams with a maximum
host-observed inter-arrival of 5.002518--5.004932 seconds. The host report also
observed no missing, reordered or duplicate sequence numbers, although the
performance image deliberately does not promote exact delivery to a verdict
without driver observation. This is the ceiling authority for the asymmetric
workload; the two instrumented images remain the CPU and BA-geometry evidence.

The remaining instrumented Core1 cost is still substantial. At four-packet
dispatch it is approximately 78.3 microseconds per delivered datagram:

- about 41.1 microseconds are inside `TxToken::consume`, including about
  37.2 microseconds of frame emission and 3.9 microseconds of publication;
- about 30.2 microseconds remain in the network poll outside `consume`;
- about 6.9 microseconds belong to the UDP producer task.

These absolute values include diagnostic accounting and are not an
observer-free production budget. They do, however, identify the next bounded
work: split the remaining network-poll time into route/resolved-key selection,
socket/index bookkeeping and protocol emission; then measure checksum and
payload construction as same-ELF phases. Increasing the dispatch quantum is
only admissible after a 1/4/8 experiment includes sparse-peer latency and
bidirectional ingress service. It must not become a ceiling-only optimization.

### UDP TX checksum cost and rejected fused copy

The current Xarxa checksum is not the old byte-at-a-time implementation. Its
`checksum::data()` uses the maintained lwIP `LWIP_CHKSUM_ALGORITHM=2`-style
aligned native-word loop, four independent accumulators and end-around carry.
The remaining cost was measured instead of inferred.

Run `1788169347421-003b7c04` disabled only IPv4 UDP TX checksum generation in
the same `b0b71004` runtime used by the four-packet software-checksum runs.
IPv4 permits this zero-checksum wire representation, but it remains a
diagnostic policy, not silent production offload. It produced 117.266 and
119.077 Mbit/s, retained BA32 and exact/equal host delivery. Per datagram:

| Phase | software checksum | omitted checksum | measured difference |
| --- | ---: | ---: | ---: |
| `TxToken::consume` | about 13,164 cycles | 9,663--9,673 cycles | about 3,495 cycles |
| frame emission | about 11,920 cycles | 8,433--8,441 cycles | about 3,483 cycles |
| publication | about 1,240 cycles | 1,230--1,231 cycles | approximately unchanged |
| Core1 network + UDP TX | about 78.3 us | 64.85--64.95 us | about 13.4 us |
| total Core0 + Core1 task time | 118.08--118.38 us | 103.95--104.01 us | about 14.2 us |

The direct phase counter therefore places approximately 3.5 kcycles, or
10.9 microseconds at 320 MHz, inside UDP checksum generation for a 1,472-byte
payload. The larger whole-task difference includes secondary scheduler/code
path effects and must not all be labelled checksum. Core0 remains about
39.1 microseconds per packet; the work is removed rather than displaced to the
radio core. Throughput is not the discriminator because every run is already
near its variable air/service ceiling.

A follow-up attempted to accumulate checksum while copying the PSRAM payload
into the final SRAM frame. The generic implementation used explicit unaligned
32-bit source loads and destination stores so it remained correct for every
slice alignment. Exhaustive host tests proved byte and checksum equivalence,
but same-ELF HIL rejected its performance:

- fused software run `1788169810340-003b8e6f`, CRC `6fab7b7c`, required
  17,621--17,632 emit cycles and about 130.2--130.4 microseconds total task
  time per datagram;
- zero-checksum control `1788170019191-003b952c` in the identical binary
  required 8,325--8,326 emit cycles and about 99.1 microseconds total.

The fused implementation therefore made the checksum delta about 9.3
kcycles, versus 3.5 kcycles for the existing separate lwIP-style pass. It was
removed completely. The result rejects that implementation, not the abstract
possibility of a target-specific aligned fused primitive. Another attempt is
not justified without first proving actual source/destination alignment and
examining generated RV32 instructions. For now, the existing algorithm is the
best measured integrity-preserving path; zero UDP checksum remains only a
cost control.

## Evolutionary implementation plan

### Production pull boundary after the successful A/B

The historical `DestinationBurst` diagnostic selected queues by IP address.
It has now been removed. Production first resolves an `EgressRoute`, then the
device canonicalizes it into an opaque `EgressKey`, and Xarxa applies the
typed `EgressSchedule`. This distinction is required because an
infrastructure STA can reach many Ethernet destinations through one BSSID,
whereas a SoftAP normally maps unicast destinations to distinct radio peers.
Peer-generation safety remains owned by the Wi-Fi admission layer rather than
the generic stack.

The cross-core control plane consists of two bounded SPSC streams, not a
shared per-packet allocator:

```text
Core1 network                         Core0 radio/MAC

active key/backlog  ----------------> airtime scheduler
                                       BA/retry completion
local cached grant  <----------------  affine burst grant

selected packet handle
  -> final SRAM TxToken
  -> direct Ethernet/IP/UDP emit
  -> existing Core0 publication
```

The production key is:

```text
VIF + peer slot + peer generation + TID/AC + traffic kind
```

The stack resolves route/neighbour state before requesting final device
backing. The implemented classifier normalizes every infrastructure-STA route
to one radio domain. For AP it resolves a destination into the current
authorized peer slot and non-reusable association epoch through a bounded
atomic snapshot. The snapshot changes queue identity and invalidates Xarxa's
burst cursor, but does not authorize transmission. Final device admission now
revalidates the VIF, lifecycle epoch and live AP slot/generation before
claiming SRAM; stale or foreign keys receive `KeyDeferred`, while global pool
pressure remains `GlobalExhausted`.

The first shadow-grant slice is also present in intrusive telemetry builds.
Core0 publishes the identity and frame limit of each selected standby AP
aggregate; Core1 spends a local diagnostic credit copy and reports matches,
missing windows, mismatches and exhaustion without changing production
admission. Because this window is derived from frames already admitted into
SRAM, it validates identity and temporal overlap only. It cannot replace the
required active-backlog stream from Core1 to the radio scheduler.

Run `1788192866170-0003cece` measured that boundary after correcting credit
accounting to include successful SRAM claims only. The two cycles delivered
118.111/118.185 Mbit/s at HT40 MCS7 with BA32 and zero client-observed retries
or failures. The current Core0 key matched 97.53%/97.40% of successful
admissions; 2.45%/2.57% exhausted the local window, 37/45 admissions saw no
stable window, and key mismatch/unclassified remained zero. This establishes
the VIF/slot/generation/TID identity mapping for one peer, not scheduling
authority or multi-peer fairness. The observer still raised Core1 admission
from roughly 0.43 to 0.78 kcycles per successful packet and the diagnostic
Core0 residence was about 46.1--46.3%, so it must remain a temporary probe.

There is also a strict causality constraint: a standby-derived grant cannot
authorize the packets from which that standby was built. Active deferral at
this point would deadlock the empty pipeline. The candidate stream must be
published before SRAM admission, must wake Core0 without an existing radio
frame, and Core0 must return a grant from that early state. This is the next
evolutionary boundary; the present seqlock shadow is not promoted to policy.

The eventual production grant names exactly one key and
carries bounded frame credits plus an airtime quantum. Core1 spends those
credits locally, so ordinary packet emission does not bounce a shared atomic
cache line. An exhausted, cancelled or temporarily progressless grant is
returned once as a value message. Core0 never waits for Core1 while holding a
hardware TX obligation.

Packet payload remains in the bounded stack-owned PSRAM arena until selected.
Its removable handle, not a copied complete Ethernet frame, sits in an
intrusive per-key FIFO. Selection consumes a handle only after a matching grant
and SRAM credit both exist. Emission performs the stack's normal payload-to-
packet construction directly into that SRAM slot; there is no additional
terminal PSRAM-frame-to-SRAM copy. Failed route/admission/emission restores the
same handle at the head of its key queue.

The existing Core0 `RoundRobinTxQueues` supplies a reviewed indexed-FIFO
algorithm, but its owner type is too late for this boundary: it receives a
complete network frame after final backing was already chosen. Reuse its
intrusive index semantics and rollback tests, not that complete-frame arena.

The Core0 queue itself is now generation-correct. Its flow key contains the
portable `ApAssociationIdentity` (`MAC + AID + non-reusable association
epoch`) rather than a MAC alone. AP downlink admission returns this identity in
the same transaction as the power-save decision. Active leases, prepared
aggregate prefixes, sleeping-peer leases and affine power-save release tokens
all retain it. Before dequeue, Core0 validates the identity through the AID
slot in O(1) and drops the stale classified owner; it never reclassifies old
payload under a replacement association. An encoded but not yet published
standby A-MPDU is cancelled at the same validation boundary if its association
has ended. This closes the post-SRAM lifecycle hole without changing the
round-robin selection result for a live peer.

It does not close the pre-SRAM lifetime yet. Xarxa packet storage which has not
crossed egress classification carries no AP association generation, so a
packet retained entirely above that boundary may still be selected after a
later association and receive its new key. The grant/backlog protocol must
either bind generation at enqueue or explicitly revoke those candidates on
the peer-directory revision. This remains a prerequisite for active
policy-driven `KeyDeferred` admission. The final admission recheck does close
the narrower race where an already classified key becomes stale before token
allocation.

Control, neighbour discovery, authentication/EAPOL, BA/BAR and beacon work use
a separate bounded reserve and may bypass a data grant. Sleeping or stale-
generation peers retain only bounded PSRAM handles and are absent from the
active airtime set. No peer owns permanent data SRAM credits.

The required third-party API change is deliberately narrow and breaking in the
project fork: Xarxa exposes removable queued-packet candidates; route/neighbour
resolution produces a generic link egress key; `embassy-net-driver` requests a
TX token for that key rather than allocating one before selection. Middleware
must forward the keyed request. Wi-Fi peer generations and airtime policy do
not enter Xarxa or Embassy APIs.

### Phase 1: measure the existing promotion transaction

Add same-ELF counters around, separately:

- free-SRAM claim;
- payload copy;
- staged-owner release and producer wake;
- NETWORK-to-READY and READY-to-RADIO transitions;
- egress classification and active-queue operations;
- 802.11/CCMP encode and descriptor commit;
- batch depth, queue high-water marks and radio idle gaps.

Report cycles and retired instructions per frame and per aggregate. Do not
attribute the residual to memcpy without these counters.

### Phase 2: extract the common software TXQ

Create a role-neutral owner under the S31 Wi-Fi datapath. AP supplies peer
validation, generation and power-save disposition; STA supplies its single
peer binding; STA+AP shares one physical scheduler. Preserve the current
round-robin result first. Replace MAC-address lookup in the hot path with
validated peer slot plus generation, and use an active bitmap/intrusive list
instead of scanning every possible peer/TID.

The first behavior-preserving extraction is now present: the shared indexed
lease arena and round-robin per-flow FIFOs contain no AP policy, and AP supplies
only its current MAC/TID key. The active-flow scan is intentionally retained
until throughput and placement gates are closed; bitmap selection is a later
measured optimization. New ingress and failed speculative admission have
different queue operations so a later refactor cannot silently turn rollback
into tail insertion again.

Drain the network publication frontier into these queues as a bounded ingress
step. Queue storage holds indices and hot metadata only; payload remains in
the global PSRAM owner pool.

### Phase 3: introduce typed batch promotion

Replace per-frame `try_promote()` with a transaction which:

1. selects one queue and exact burst length;
2. reserves all available destination leases before moving any source owner;
3. owns source and destination arrays until completion or rollback;
4. publishes successful destinations together;
5. returns staged credits and wakes Core1 once per batch;
6. handles partial capacity by building a smaller valid burst, never by
   waiting while holding a terminal radio obligation.

First run a Core0 CPU-copy implementation. This isolates batching and owner
bookkeeping from copy placement even if it does not meet the final Core0 gate.

### Phase 4: materializer A/B

Compare behind the identical batch contract:

- Core0 CPU copy control;
- Core1 CPU copy through two affine SPSC rings;
- AXI-GDMA scatter/gather with IRQ completion.

Before the GDMA radio A/B, prove on HIL that one descriptor chain can copy 1,
2 and 32 discontiguous PSRAM frames into matching discontiguous SRAM slots,
including ring wrap, mixed lengths, guards, cancellation and cache writeback.
The AXI-GDMA group is shared hardware: initialize/reset it once before any
other AXI-GDMA user, then transfer a retained channel owner into the Wi-Fi
composition. Construction during active DMA is forbidden.

### Phase 5: airtime and AQL cutover

Run shadow decisions first, then enable weighted airtime DRR and pending
airtime limits. Reconcile estimates from actual BA/retry/PHY completion. Test
equal-rate, asymmetric PHY, lossy peer, sleeping peer, AP+STA and control
traffic under saturation.

### Phase 6: optional direct bypass and Xarxa A/B

Only after the queued path passes, compare:

- all-data staged baseline;
- role-level single-peer direct mode with a drain-safe transition;
- egress-hook affine direct grants;
- owned-UDP packet storage.

Delete any hook or fork delta which does not produce a measured CPU, memory or
latency benefit. No compatibility layer is required for rejected experiments.

## Acceptance gates

The queued production candidate is acceptable only when it demonstrates:

- at least 120 Mbit/s aggregate TX in the two equal-PHY client test;
- less than 40% Core0 residence at that rate;
- bounded Core1 residence with explicit headroom for bidirectional service;
- zero owner leak, duplication, reorder and unclassified loss;
- fixed internal-SRAM DMA footprint independent of associated peer count;
- bounded PSRAM bytes and packet age with control reserves;
- aggregate-depth and radio-idle histograms no worse than the 98-slot direct
  baseline within the agreed tolerance;
- fairness by delivered airtime, including asymmetric rates and retries;
- correct AP+STA VIF hierarchy and generation-safe teardown;
- the same functional result for CPU and GDMA materializers.

The required HIL matrix is single peer; 2, 4, 8 and 15 associated peers; 2, 4,
8 and 15 saturated peers where the lab supports them; asymmetric offered
load; asymmetric PHY/retry cost; sleeping peer; AP+STA; and TX-only plus
bidirectional traffic. Throughput equality alone is insufficient: both-core
cycles, instructions, queue residence, pending airtime, DMA-credit floor,
aggregate depth and radio idle time must be recorded.
