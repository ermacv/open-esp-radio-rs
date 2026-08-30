# Wi-Fi TXQ and Core0-load architecture audit

Status: working architecture decision, 2026-08-30. This document separates
measured facts from design conclusions. It is not qualification evidence.

## Decision

The target is achievable: the measured 98-slot direct path already delivered
about 121 Mbit/s with about 34% Core0 residence. The 61% Core0 result is not an
intrinsic cost of the radio, CCMP or BlockAck path. It belongs to the current
scheduled staging implementation, which promotes one PSRAM frame at a time on
Core0 and performs a complete ownership transaction around every copy.

The production direction is therefore:

1. keep complete, independently owned software frames in a bounded PSRAM tier;
2. classify those owners into role-neutral `VIF + peer generation + TID/AC`
   queues before internal-SRAM/DMA admission;
3. select one queue by hierarchical airtime policy and dequeue a burst;
4. materialize the selected burst into a fixed global internal-SRAM working
   set as one typed batch;
5. overlap preparation of the standby batch with the active radio exchange;
6. keep the current direct path as the measured single-owner control and add a
   direct-bypass policy only after the queued path meets its gates.

This is the topology of mac80211 software TXQs adapted to the S31 memory
constraint. The Linux implementation itself is not reusable, but its
`classify -> software TXQ -> airtime selection -> burst dequeue -> short
hardware queue` boundary is applicable.

A deep Xarxa packet-ownership rewrite is not required to establish this
architecture or to remove the measured Core0 staging overhead. It remains a
later A/B candidate for reducing Core1 work and one upper-layer payload copy.
The experimental pre-emission egress hook is useful for lazy admission and a
direct bypass, but it does not implement TXQ pull scheduling and must not be
the foundation of fairness.

## Measured state

All percentages below are task residence, not an assumption derived from
throughput. The relevant two-client HT40 runs are recorded in
[`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md).

| Path | DMA slots | Aggregate throughput | Core0 radio | Core1 network + UDP TX | Established result |
| --- | ---: | ---: | ---: | ---: | --- |
| direct | 98 | 120.77--121.39 Mbit/s | 33.91--34.15% | 61.16--61.22% | the radio path can sustain the target below 40% Core0 |
| direct | 67 | 96.61--97.20 Mbit/s | 39.41--39.56% | 49.55--50.14% | early DMA admission plus interleaved peers fragments aggregates |
| scheduled PSRAM staging | 67 | 120.05--121.03 Mbit/s | 60.90--60.94% | 64.84--65.31% | late DMA admission fixes queue geometry but the current promotion is too expensive |

The 98-slot direct result is a causal baseline, not a scalable design. It uses
31 additional 1,696-byte internal-SRAM slots, or 52,576 bytes, to provide
look-ahead for two interleaved peers. Extending that rule by one BA window per
peer cannot support the AP client limit.

The staged path proves the complementary fact: a peer-homogeneous software
queue can recover full A-MPDU throughput with the 67-slot physical pool. Its
failure is CPU placement and transaction granularity, not its queue topology.

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
- an exhausted DMA credit returns the exact staged owner without loss.

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

### Level 3: pull-selected Xarxa egress — largest change, not justified yet

A true stack-level pull API would let the radio grant a peer/TID and ask Xarxa
to materialize a matching queued transport packet directly into SRAM. That
could avoid the complete PSRAM frame for direct hits, but it changes socket
polling, route/neighbour handling, fragmentation, the driver trait and every
middleware adapter. It also couples a generic IP stack to link-scheduler
selection. Consider it only if bounded PSRAM TXQs plus batch materialization
meet Core0 but fail Core1 or memory/latency gates at 8--15 clients.

## Evolutionary implementation plan

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
