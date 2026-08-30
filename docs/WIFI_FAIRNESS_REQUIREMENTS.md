# Wi-Fi fairness initial requirements

Status: working requirements and measured architecture baseline
Initial revision: 2026-08-30
Current revision: 2026-08-30, direct-PSRAM Wi-Fi DMA proof and candidate-A rejection

This document is the starting contract for Wi-Fi fairness work. It is
intentionally evolvable: measurements and hardware limits may change the
policy, scenarios or numerical gates. Changes must preserve the reason for the
revision and distinguish a measured fact from a design hypothesis.

The document is not HIL evidence and does not qualify a capability. Dated,
sealed HIL runs and the machine-readable qualification manifests remain the
readiness authority.

## Implementation status

Protocol version 71 carries a bounded two-entry
flow table and requires one CRC-covered `FlowTransportEvidence` record for
every configured flow. The host independently verifies that the per-flow sum
equals the session-wide transport total.

The target accepts two-flow UDP only when every flow has a unique identity and
peer endpoint. RX classifies each datagram before accounting; TX owns an
independent offered-rate deadline per flow and selects ready flows in bounded
round-robin order. Single-flow ceiling sessions retain their dedicated path
without per-packet peer lookup.

The AP production path now classifies already-published network leases into
bounded intrusive per-peer/TID queues, selects those queues round-robin and
owns QoS sequence state per peer. This removed the original immutable-VIF-FIFO
ordering defect, but classification still happens after a frame has consumed
a physical DMA-visible SRAM credit. The distinction is central to the next
architecture: logical queueing has been separated, physical backing has not.

## Scope

The implementation and HIL must cover:

1. ESP operating as an access point with multiple external clients.
2. ESP operating as a same-channel access point plus station, connected to one
   external station and one external access point.
3. ESP operating as an HE20 station.
4. RX-only, TX-only and bidirectional traffic in every applicable topology.
5. Saturation initiated from each side and asymmetric competing loads.
6. Work-conserving, measurable fairness without losing the established
   single-peer throughput or Core0 efficiency.

## Current baseline

### Access point

The AP protocol owns up to 15 independent peer records, including security,
rate, power-save, per-peer QoS sequence and BlockAck state. Core0 now regroups
the one AP-VIF FIFO into per-peer/TID intrusive queues without copying payload
bytes. Each queued owner nevertheless remains one of the finite physical TX
DMA slots.

Consequences:

- peer association, protocol state and logical active queues are independent;
- all logical queues still compete for one DMA-backed producer pool;
- finding 32 frames for one selected peer may retain interleaved frames for
  every other peer and exhaust the credits needed to finish the aggregate;
- the required look-ahead is approximately `BA * (active_peers - 1)` for a
  deliberately alternating producer, so increasing the physical pool fixes a
  chosen peer count rather than the architecture;
- round-robin frames are not yet airtime fairness.

The new two-flow workload measures two simultaneously saturated peers and
proves exact per-flow host accounting. It is an architecture diagnostic, not
yet a fairness qualification: both peers currently use the same PHY and the
criteria deliberately do not yet impose an airtime verdict.

### Causal two-client TX credit experiment

All results below use HT40/MCS7, 1472-byte UDP payloads and the split
radio/network placement. The only production geometry changed in the A/B was
the number of DMA-visible TX slots.

| Geometry | Single client | Two clients | Interpretation |
| --- | ---: | ---: | --- |
| 67 slots | 122.39--123.01 Mbit/s | 96.99--98.47 Mbit/s | two complete BA32 arenas plus reserves cannot also classify an alternating second peer |
| 98 slots | 122.12--122.40 Mbit/s | 120.39--121.63 Mbit/s | 31 look-ahead credits restore two full peer-homogeneous aggregates |

Run `1788097700270-002ac14d` adds only executor-residence telemetry to the
98-slot image. Its two cycles delivered 119.66 and 119.78 Mbit/s, split within
one datagram between peers. Core0 radio residence was 33.24 and 32.17 percent;
Core1 network plus UDP-TX residence was approximately 64.9 and 63.4 percent.
This establishes that Core0 is below the 40-percent target in the measured
two-peer TX cell. It does not establish spare Core1 budget for an external
backlog copy.

The 31 extra slots cost 52,576 bytes of internal SRAM. A 99-slot trial did not
fit while preserving the reviewed 8-KiB CPU1 bootstrap handoff stack, and the
98-slot task-poll/correctness overlays also exceed internal SRAM. Therefore 98
is retained as the two-peer causal/performance baseline, not accepted as the
scalable production design.

### One-copy lower-bound experiment

The first staging discriminator was invalid: its per-packet stack array caused
the compiler to emit both a 1,600-byte ROM `memset` and a variable-length ROM
`memcpy`. Its numbers must not be used as the cost of one copy. The corrected
probe uses one persistent, pre-initialized 1,600-byte scratch allocation per
network endpoint. The HIL placement audit locates both scratch allocations in
PSRAM. Machine-code inspection shows no packet-sized `memset`, reduces the
network dispatch stack frame from approximately 1.7 KiB to 592 bytes, and
shows one mutually exclusive IPv4/IPv6 call of
`memcpy(dma, psram, packet_len)`.

Runs `1788100000961-002b5528` (direct) and
`1788100210071-002b5d17` (copy) used the same runtime CRC `cff670da`, two
simultaneously saturated AP clients, two 16-second cycles and exact per-flow
delivery. Results are:

| Path | Aggregate throughput | Core1 network+UDP TX | Core0 radio | Core1 time/datagram |
| --- | ---: | ---: | ---: | ---: |
| direct DMA | 120.78--121.39 Mbit/s | 66.00--66.04% | 33.54--33.59% | 64.06--64.35 us |
| persistent PSRAM, one copy | 114.60--118.32 Mbit/s | 71.84--74.44% | 33.67--34.82% | 73.83--74.09 us |

The measured lower-bound cost of materializing a 1,486-byte Ethernet frame is
therefore 9.74--9.77 microseconds of Core1 task residence per datagram. One
copy is not free and cannot be accepted merely because it is scalable. This
probe does not yet schedule before DMA admission, does not remove the
31-credit classification window and does not model a cache-cold queued packet.
It therefore neither accepts nor rejects the two-tier design. The decisive
test must combine the copy with peer-homogeneous software queues and reduce the
DMA-visible pool; the recovered aggregation and SRAM must be compared against
the measured CPU/throughput cost.

### Scheduled one-copy ownership experiment

The typed candidate separates the network packet owner from the physical DMA
owner. The AP retains CPU-only frames in its existing per-peer/TID arena and
claims a DMA slot only after selecting a flow. Promotion copies the selected
Ethernet frame once from PSRAM into DMA SRAM and immediately returns the PSRAM
credit. A staged frame cannot implement `StableDmaBacking`; the DMA encoder can
only receive the result of the promotion operation. Exhausted DMA credit
returns the exact staged owner without copying or dropping it.

The 98-slot same-ELF pair used runtime CRC `68e3ab8c`:

| Path | Run | Aggregate throughput | Core0 radio | Core1 network + UDP TX |
| --- | --- | ---: | ---: | ---: |
| direct DMA | `1788101941828-002ba4d0` | 120.77--121.39 Mbit/s | 33.91--34.15% | 61.16--61.22% |
| scheduled one-copy | `1788102053452-002bac1f` | 121.52--121.82 Mbit/s | 62.47--62.48% | 65.71--65.89% |

Both paths delivered the two flows within one datagram and reported no host
loss, reorder or duplication. The copy did not improve throughput because the
98-slot direct control already sustains full aggregates. It increased Core0
radio residence by approximately 28.4 percentage points, or 27.1--27.3
microseconds per delivered datagram relative to the direct control. Writing
the packet into the PSRAM tier also increased Core1 residence by approximately
4.6 percentage points.

The decisive reduced-resource pair used runtime CRC `f2e582c4` and 67 physical
DMA slots, saving 31 slots or 52,576 bytes of internal SRAM relative to the
98-slot control:

| Path | Run | Aggregate throughput | Core0 radio | Core1 network + UDP TX |
| --- | --- | ---: | ---: | ---: |
| direct DMA | `1788102341331-002bb7d4` | 96.61--97.20 Mbit/s | 39.41--39.56% | 49.55--50.14% |
| scheduled one-copy | `1788102448956-002bbb11` | 120.05--121.03 Mbit/s | 60.90--60.94% | 64.84--65.31% |

This establishes two distinct facts. Scheduling before DMA admission removes
the peer-interleaving/HOL penalty and restores full aggregation with the
smaller SRAM pool. The measured PSRAM-to-DMA implementation nevertheless
violates the less-than-40-percent Core0 target by about 21 percentage points.
Candidate A is therefore retained as a causal reference, not accepted as the
production datapath. Candidate B must preserve the late DMA-admission queue
topology while transferring an existing packet owner instead of materializing
the payload on Core0.

### Direct-PSRAM Wi-Fi DMA address proof

The general ESP32-S31 capability flag `SOC_PSRAM_DMA_CAPABLE` is not a Wi-Fi
DMA contract. The same vendor headers define the ordinary DMA window as only
`0x2f00_0000..0x2f08_0000`; `esp_ptr_dma_capable()` accepts that internal
window and exposes a separate `esp_ptr_dma_ext_capable()` query for external
memory. The vendor Wi-Fi configuration is more specific: enabling PSRAM forces
static Wi-Fi TX buffers, each upper-layer frame is copied into one of those
buffers, and the static buffers are the hardware-layer DMA storage.

The repository now contains a diagnostic-only counterexample test rather than
inferring the result from those sources. Runtime CRC `7251c13e` retained the
A-MPDU descriptors in internal SRAM at `0x2f06c5c0`, retained the ordinary TX
pool at `0x2f050100`, and placed the alternate 115,904-byte pool in PSRAM at
`0x50324080`. Every PSRAM slot is 64-byte cache-line isolated. The final
software ownership edge performs the S31 two-pass L1 D-cache writeback before
publishing the full 32-bit packet-buffer address in an otherwise unchanged
internal-SRAM descriptor.

The exact-workload pair used one AP client, HT40, a 1,472-byte payload, a
10-Mbit/s offer, two 8-second cycles and differed only in `tx_buffer`:

| Backing | Run | Result | Publication evidence |
| --- | --- | --- | --- |
| PSRAM direct | `1788104889906-002c79de` | failed: zero UDP bursts reached the associated client | 62,464 backing preparations, first/last addresses `0x5032ba00`/`0x50338b40` |
| SRAM direct | `1788105231819-002c882e` | passed both cycles; 10,080,256 target bytes and 10,385,943/10,386,052 client bytes per cycle | zero PSRAM preparations, as required |

The PSRAM preparation count includes repeated aggregate publication/retry and
is not a unique-frame count. It proves that validated PSRAM owners repeatedly
crossed the final cache-maintenance and DMA-publication boundary; the failure
cannot be attributed to the socket, ARP lookup or software queue. The SRAM
control used the identical ELF and laboratory geometry and observed HT40 MCS7.
An additional same-ELF saturation control, run `1788105063894-002c8511`,
delivered 122.351 and 122.396 Mbit/s.

Therefore the documented/current Wi-Fi TX DMA path cannot consume packet
backings through the cached PSRAM aperture. A hypothetical undocumented address
translator would require a new vendor register proof before reconsideration;
passing a `0x50xx_xxxx` address and writing back cache lines is insufficient.
Production descriptors and packet backings remain internal-SRAM-only. PSRAM
may hold software backlog, but a selected packet must acquire SRAM backing
before Wi-Fi DMA ownership.

### Same-channel station plus access point

STA and AP have separate network endpoints and share one physical datapath.
The current TX selector compares admitted frame counts for the two VIFs. The
RX/TX cooperative fairness quantum is also expressed in completed frames.

This is useful protection against gross starvation for equal-rate traffic, but
it is not airtime fairness: MCS, bandwidth, aggregation, retries and failed
attempts are not represented by a frame count.

The current HIL can run STA RX/TX and AP RX/TX simultaneously. Its fairness
verdict compares per-interface throughput under equal offered rates and permits
a 50 percent skew. It does not test asymmetric saturation or per-AP-client
fairness.

### HE20 station

Separate UDP RX and TX ceiling scenarios exist. The bidirectional HE20 scenario
is currently TCP at a moderate 30+30 Mbit/s offer. There is no UDP
bidirectional ceiling, asymmetric RX/TX sweep or mixed-packet-size fairness
matrix.

### Physical BlockAck resources

ESP32-S31 has eight physical ordinary RX BlockAck banks. The same-channel
logical owner can identify one station peer plus up to 15 AP peers, but logical
peer capacity does not create additional hardware banks.

The AP currently declines an ADDBA request when no bank is free. The station
path can propagate the same resource failure. A production combined-role
policy must reserve or prioritize the upstream station agreement and must
degrade to non-aggregated RX rather than fault the role.

## Fairness definition

Fairness is not defined as equal throughput in the general case. Peers using
different PHY rates can have equal airtime and different useful throughput.

The required properties are:

1. **TX airtime fairness.** ESP-originated transmissions are selected by
   weighted airtime, not frame count or byte count.
2. **RX service fairness.** One external transmitter must not monopolize Core0
   protocol work, reorder publication or network handoff after the physical DMA
   ring has been drained.
3. **Buffer fairness.** One producer must not permanently retain the complete
   shared pinned-frame or deferred-work capacity.
4. **No starvation.** Every healthy, backlogged eligible entity receives
   service within a bounded number of scheduler rounds.
5. **Work conservation.** Idle entities do not prevent active entities from
   using the channel or Core0 budget.
6. **Deadline correctness.** Beacon, BA, authentication, key and power-save
   control deadlines remain bounded, but control work does not receive an
   unlimited priority budget.

Ordinary external uplink EDCA is not scheduled by ESP. AP RX airtime fairness
is therefore observational unless trigger-based uplink scheduling is available.
ESP can measure received airtime, protect its software resources and compensate
subsequent ESP TX service, but cannot force independent stations to contend
equally.

## Architecture target

### Classification and queue ownership

Network TX must be classified before peer aggregation into a stable key such
as:

```text
TxQueueKey = interface + peer binding/generation + TID/AC + traffic kind
```

The peer generation prevents a queued frame from crossing peer-slot reuse.
Ordering is required within one peer/TID. No global ordering requirement exists
between independent peers, and retaining it creates avoidable head-of-line
blocking.

Logical backlog and physical DMA transaction storage must be separate tiers.
Per-peer/TID queues own indices into one shared, bounded software-packet pool;
they do not reserve DMA slots. Only frames chosen by the airtime scheduler are
materialized into the common active/standby DMA aggregate windows. Retried
MPDUs retain their DMA backing until terminal completion, while an unselected
peer consumes no DMA credit.

The software pool is global rather than `peer_count * BA`. Admission combines
a minimum active-entity share, an elastic remainder, per-flow caps and a
control reserve. Its capacity and watermark are measurements, not values
inferred from the maximum peer count.

The direct zero-copy path remains valid when there is only one schedulable data
entity. When multiple VIF/peer/TID entities contend, scheduling must precede
DMA ownership, so their data enters the software tier. A mode transition must
drain the older frontier before publishing the newer backing kind; it may not
reorder frames within a peer/TID or reuse a peer generation.

The intended ownership graph is:

```text
embassy-net -> shared software packet pool -> VIF/peer/TID queues
                                              |
                                      airtime scheduler
                                              |
                       common DMA materialization credits
                                              |
                                active/standby A-MPDU
                                              |
                                      BA/retry completion
```

This adds one PSRAM-to-DMA payload copy for contended data. A path which first
writes DMA SRAM, spills to PSRAM and later copies back performs two additional
copies and is not the production candidate. Direct Wi-Fi DMA from PSRAM is not
assumed: the ESP32-S31 vendor contract explicitly describes the TX buffer as a
DMA buffer and, when PSRAM is enabled, uses static TX buffers plus a copy from
the upper layer.

### Rejected and retained candidates

| Candidate | Status | Reason |
| --- | --- | --- |
| 67-slot DMA-only queues | fallback baseline | bounded SRAM and zero-copy, but measured at only 97--98 Mbit/s for two alternating peers |
| 98-slot DMA-only queues | causal/two-peer baseline | measured fast and efficient, but consumes 52,576 extra internal bytes and needs approximately `BA * (N-1)` look-ahead |
| one BA-sized DMA reservation per peer | rejected | internal memory grows with logical peer count and idle peers strand radio credits |
| DMA-to-PSRAM overflow spill | rejected as normal path | classification occurs too late and spilled frames pay two extra payload copies |
| direct PSRAM Wi-Fi DMA | rejected by hardware A/B | 62,464 prepared PSRAM-backed publications delivered no UDP data while the identical SRAM workload passed |
| shared PSRAM backlog, always copied | rejected as production path | restores 120--121 Mbit/s with 67 DMA slots, but measured Core0 radio residence is 60.9% |
| transferable software packet ownership with late DMA admission | next candidate | must retain the proven peer-homogeneous queue geometry without a Core0 payload copy |
| direct uncontended path plus transferable contended backlog | conditional target | preserves the proven single-entity path; acceptance depends on candidate-B resource and CPU evidence |

Candidate A has completed the same-image copy-cost gate and failed the Core0
budget. Candidate B must now separate packet construction/ownership from DMA
admission without depending on direct PSRAM DMA. Its A/B must account for
SRAM, PSRAM, Core0, Core1, throughput and queue residence; moving the same copy
to an unmeasured boundary is not an architectural win.

### Candidate-B API audit and ownership contract

The current `embassy-net-driver`/Xarxa transmit contract cannot express the
required decision point. `Driver::transmit` returns a token before Xarxa has
resolved the destination hardware address. `TxToken::consume` then asks that
token for exactly one mutable output buffer and emits the complete Ethernet
frame into it. On the ordinary IPv4 path `set_meta` is called only after
neighbor lookup and immediately before `consume`, but its metadata carries no
egress destination or traffic class.

Consequently, changing only the Rust owner type does not remove a payload
copy. The existing direct path already transfers unique ownership of the
completed internal-SRAM frame from Core1 to Core0 and then to Wi-Fi DMA without
a post-stack copy. If a completed frame instead resides in PSRAM, the negative
DMA address proof requires one PSRAM-to-SRAM materialization before radio
publication. No ownership API can make that physical transfer disappear.

Candidate B therefore has two explicit mechanisms rather than one universal
packet representation:

1. A logical TX token is reserved without irrevocably choosing PSRAM or DMA
   SRAM. Before `consume`, Xarxa supplies an `EgressIntent` containing the
   resolved destination hardware address and traffic class. A bounded
   scheduler grant for the matching VIF/peer-generation/TID permits the token
   to emit directly into a granted SRAM owner. That completed owner follows the
   existing zero-copy Core1-to-Core0 path.
2. A token without a matching grant emits into the shared PSRAM software pool.
   Core0 queues only its small packet identifier and classification. After the
   airtime scheduler selects it and assigns a DMA credit, a Core1
   materialization worker performs the unavoidable single copy into that
   exact SRAM owner and returns a prepared owner to Core0. Core0 must not touch
   the payload during this path.

The grant is a consumable credit, not a racy `current_peer` hint. It is keyed by
interface, peer identity/generation and TID/AC, and it is bounded by aggregate,
airtime and DMA capacity. A stale generation cannot consume it. Reserving a
logical token must also reserve a valid fallback backing so that the existing
infallible `TxToken::consume` contract remains true.

```text
Core0 scheduler                         Core1 network/materializer
---------------                         --------------------------
grant(key, credits)  -----------------> set_egress_intent(key)
                                         | matching grant
                                         `-> emit directly in SRAM owner
                                             ---------------------> Core0 ready

PSRAM packet id      <------------------ no matching grant: emit in PSRAM
select(id) + SRAM owner ----------------> copy once on Core1
prepared SRAM owner  <------------------ return ownership, not payload
publish to Wi-Fi DMA
```

The first implementation gate is API/ownership correctness, not throughput:
host tests must prove that metadata is visible before backing selection, every
token has exactly one fallback owner, grant consumption is affine, stale grants
fall back to PSRAM, and a materialization request either returns the exact
prepared DMA owner or preserves both original owners. The subsequent same-ELF
HIL comparison must report direct/staged frame counts and copied bytes in
addition to both-core residence and memory footprint. Candidate B is accepted
only if two-peer throughput remains at least 120 Mbit/s and Core0 returns below
40 percent without moving an unbounded bottleneck to Core1.

### Hierarchical TX scheduling

The scheduler hierarchy is:

```text
control deadlines
       |
       v
VIF airtime scheduler
       |
       +-- STA peer -> AC/TID
       |
       `-- AP VIF -> AP peer -> AC/TID
```

VIF scheduling must precede AP-peer scheduling. A flat list of all peers would
give an AP VIF more aggregate weight merely because it has more associated
clients and could starve the station VIF in STA+AP mode.

The initial production policy is equal weight among simultaneously backlogged
entities at each level. Weights must remain explicit so later product policy can
change them without replacing the ownership model.

Unused credit is bounded by an active interval and must not accumulate during a
long idle period. A sleeping, stopped or otherwise progressless peer is removed
from the active data scheduler and rearmed by its wake/progress event.

### Airtime accounting

Admission charges estimated airtime before frames enter a hardware transaction,
providing an AQL-like bound on outstanding work. Completion reconciles the
estimate using every proven source available on ESP32-S31:

- selected PHY vector;
- aggregate composition;
- BA bitmap and failed MPDUs;
- retries and fallback attempts;
- hardware PPDU duration, only if a reviewed hardware path proves it.

Until hardware duration is proven, a conservative PHY estimator plus
completion correction is compared against independent monitor and external
station duration counters. Estimator accuracy is a measurement gate, not an
assumption.

An A-MPDU contains only the selected peer/TID and is bounded by the negotiated
BA window, hardware limit, available frames and remaining airtime quantum.

### RX service

The physical RX ring keeps the current NAPI-like ownership rule: mask IRQ,
drain with a bounded budget, continue polling while work remains, and unmask
only after the hardware frontier is proven empty.

Peer fairness begins only after the descriptor/lease has crossed the physical
drain boundary. Any future per-peer protocol budget must not delay DMA-credit
recovery. RX service restructuring is introduced only after HIL counters prove
peer starvation or resource capture.

### BlockAck allocation

The shared eight-bank allocator requires an explicit policy:

- reserve or prioritize at least one upstream STA/TID0 agreement in STA+AP;
- allocate the remaining banks among active AP peers/TIDs;
- decline excess agreements without faulting the role;
- reclaim only through ordered DELBA/reorder/hardware teardown;
- expose allocation, decline, pending and reclaim counters per VIF/peer/TID.

## Required HIL matrix

### AP with multiple clients

The first physical implementation targets two controlled clients:

| Cell | Traffic |
| --- | --- |
| RX-only | client 1 -> ESP and client 2 -> ESP |
| TX-only | ESP -> client 1 and ESP -> client 2 |
| Bidirectional | all four flows |
| Sparse isolation | one saturated peer plus one low-rate/latency flow |
| Slow peer | equal offered load with deliberately different MCS |
| Lossy peer | one retry-heavy peer plus one healthy peer |
| Power save | one sleeping/waking peer plus one active peer |
| Lifecycle | join, leave and reassociation under load |

Client identities and start order are swapped between repetitions. Host/model
tests cover all 15 logical peer slots even while the physical lab supports only
two simultaneous clients.

### Same-channel STA+AP

The four independent flows are:

- `S_RX`: external AP -> ESP station;
- `S_TX`: ESP station -> external AP;
- `A_RX`: external station -> ESP AP;
- `A_TX`: ESP AP -> external station.

The full periodic matrix contains all 15 non-empty activation masks. The
minimum per-change suite contains:

- each flow independently;
- `S_RX + A_RX`;
- `S_TX + A_TX`;
- `S_RX + A_TX`;
- `S_TX + A_RX`;
- all four flows;
- 90/10 and 10/90 dominance for each competing pair.

HT40 is the initial controlled cell. A second cell uses an HE20 upstream
station association and an HT20 SoftAP on the same 20-MHz channel context.

### HE20 station

Required UDP cells are:

- RX-only and TX-only rate sweeps;
- bidirectional 50/50, 75/25 and 25/75;
- 64, 512 and 1472-byte payloads;
- a below-ceiling exact-delivery cell;
- a deliberate overload cell with complete drop accounting.

The runner records PHY, width, MCS, coding, GI/LTF and BA state. Exact GI is a
verdict only for a fixture which explicitly fixes it; otherwise results are
bucketed by the observed GI rather than silently mixed.

## Evidence and verdicts

Required per-VIF/peer/TID evidence:

- offered, admitted and delivered frames/bytes;
- queue depth, residence p50/p99/max and drop reason;
- scheduler selections, weight/debt and maximum service gap;
- estimated and completed TX airtime;
- rate, retries, failed MPDUs and BA partial completion;
- aggregate MPDU/byte population;
- RX airtime, reorder occupancy and protocol/network blocking.

Required global evidence:

- Core0 task residence;
- RX IRQ, repost and poll counters;
- minimum free RX descriptors and TX leases;
- beacon lateness/loss;
- DMA, FIFO and ownership errors;
- independent monitor and external station duration/retry counters where
  available.

The principal fairness metric is normalized weighted airtime. A Jain index is
supplemental and cannot replace maximum service-gap and queue-latency checks.
Throughput skew is a fairness proxy only in a controlled same-PHY cell.

Initial numerical gates, subject to revision after the baseline matrix, are:

- at most 2 percent single-peer throughput regression;
- no more than 2 percentage points additional Core0 residence and retention of
  the established less-than-40-percent target at the existing ceiling;
- no more than 15 percent normalized airtime skew in a controlled equal-weight
  two-peer cell;
- no starvation of an eligible backlogged peer;
- zero beacon loss and zero hardware overflow/ownership errors;
- at least three reset-separated repetitions with swapped client identity and
  start order.

Below-ceiling correctness requires exact delivery where the fixture can prove
it. Deliberate overload does not require exact delivery; it requires that all
drops are bounded, classified and compatible with the fairness policy.

## Implementation sequence

1. Preserve the completed 98-slot and 67-slot same-ELF direct/one-copy HIL
   records and the direct-PSRAM negative proof as causal evidence; do not
   optimize candidate A or direct PSRAM into production.
2. Prototype transferable network packet ownership and compare its SRAM,
   PSRAM, Core0, Core1 and throughput costs against both recorded candidates.
3. Keep the physical DMA pool at the smallest geometry that sustains the two
   active/standby BA windows plus endpoint/pipeline reserves; prove that owner
   transfer retains the peer-count-independent late-admission behavior.
4. Remove the losing ownership implementation and its runtime discriminator.
5. Add airtime accounting in temporary shadow mode and validate the estimator.
6. Cut over to hierarchical VIF -> peer -> TID scheduling and add AQL-like
   outstanding-airtime and buffer bounds.
7. Sweep two through fifteen modeled peers and every physically available HIL
   peer count; tune the shared software-pool capacity from queue residence and
   aggregate-population evidence rather than `N * BA` allocation.
8. Introduce the explicit shared RX BA-bank policy.
9. Add post-DMA RX peer scheduling only if measurements prove it necessary.
10. Seal qualification runs and remove the DMA look-ahead workaround and all
   temporary probes.

## External design references

- [Linux mac80211 software TX queueing](https://github.com/torvalds/linux/blob/master/include/net/mac80211.h): per-station/per-TID queues and delegated airtime scheduling.
- [Linux mac80211 TX queue implementation](https://github.com/torvalds/linux/blob/master/net/mac80211/ieee80211_i.h): per-TID flow queue, CoDel state and scheduler round.
- [Linux mac80211 station airtime state](https://github.com/torvalds/linux/blob/master/net/mac80211/sta_info.c): per-AC weight, deficit and pending airtime initialization.
- [OpenWrt mt76 TX path](https://github.com/openwrt/mt76/blob/master/tx.c): selected-station bursts and removal of progressless power-save peers from the scheduler.
- [ESP32-S31 Wi-Fi buffer model](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-guides/wifi-driver/overview.html): separate upper-layer packets and static DMA TX buffers with an explicit copy when PSRAM is enabled.
- [ESP32-S31 Wi-Fi performance configuration](https://docs.espressif.com/projects/esp-idf/en/latest/esp32s31/api-guides/wifi-driver/wifi-performance-and-power-save.html): physical static TX-buffer depth and upper-layer TX-buffer depth are independent tuning dimensions.
- [ESP-IDF Wi-Fi TX-buffer Kconfig](https://github.com/espressif/esp-idf/blob/master/components/esp_wifi/Kconfig): PSRAM selects static buffers and every upper-layer frame is copied into a static Wi-Fi TX buffer.
- [ESP32-S31 memory windows](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32s31/include/soc/soc.h): the generic DMA window is internal `0x2f00_0000..0x2f08_0000`, distinct from external RAM at `0x5000_0000..0x5400_0000`.
- [ESP-IDF DMA pointer predicates](https://github.com/espressif/esp-idf/blob/master/components/esp_hw_support/include/esp_memory_utils.h): ordinary and external DMA-capable pointer checks are separate contracts.
- [Linux NAPI contract](https://docs.kernel.org/networking/napi.html): bounded RX polling and IRQ masking ownership.
- [MediaTek mt7925 airtime reporting](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/mediatek/mt76/mt7925/mac.c): hardware per-station TX/RX airtime reporting.

## Revision policy

When this contract changes, update the status/revision notes and explain which
new measurement or hardware fact caused the change. Never rewrite an
unconfirmed hypothesis as historical fact. Dated HIL records remain immutable;
this living document records the current intended behavior.
