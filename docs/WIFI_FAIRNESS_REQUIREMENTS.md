# Wi-Fi egress and fairness requirements

Status: normative, intentionally evolvable requirements. This document says
what the completed datapath must do. It does not prescribe the current
implementation and does not contain an experiment diary.

The target is one ESP32-S31 radio serving station, access-point and concurrent
station-plus-access-point roles without making internal SRAM usage grow with
the number of clients. Correctness, bounded progress and total CPU work are
requirements; peak throughput alone is not sufficient.

## Scope

The completed design must cover:

- an AP with one through fifteen associated clients;
- simultaneous AP and STA virtual interfaces on one same-channel radio;
- HT20, HT40 and the implemented HE20 station subset;
- UDP and TCP bulk traffic, raw traffic and stack-generated control traffic;
- TX-only, RX-only and bidirectional traffic;
- equal saturation, unequal offered loads, sparse traffic, sleeping peers,
  rate asymmetry, retry pressure and association churn;
- one-core and current split-core compositions without changing packet or
  radio semantics.

Monitor and scanner roles share the physical lifecycle and interrupt rules,
but have no ordinary network egress queues. Their regression gates remain part
of the role matrix, not the airtime-fairness policy.

## Normative ownership requirements

1. A packet has exactly one owner. A failed admission leaves ownership with
   the caller; a successful admission transfers it to the callee.
2. Long-lived software backlog and short-lived DMA/radio execution storage are
   distinct resources.
3. The internal-SRAM TX working set is global and bounded independently of
   associated peer count.
4. A sleeping, slow or stalled peer cannot retain an unbounded share of SRAM,
   DMA descriptors or pending airtime.
5. Core0 is the sole mutable owner of physical radio facts: active VIFs, peer
   generations, BA sessions, power-save eligibility, rate/retry state,
   physical credits and terminal TX receipts.
6. Network and transport code does not know Wi-Fi peer slots, BA banks, rate
   tables or descriptor identities.
7. Association generation is part of queued-work identity. Reassociation of
   the same MAC address never authorizes stale work.
8. Stack-side completion of packet construction is not a terminal radio
   completion. SRAM/DMA credits and actual airtime are reconciled only by the
   physical completion path.

## Queue and scheduling requirements

The link scheduler operates on a domain equivalent to:

```text
physical radio
  -> VIF
     -> peer generation or group domain
        -> traffic class / TID
           -> FIFO packet owners
```

The exact key is driver-owned and opaque outside the link layer. An Ethernet
destination is not universally a radio peer: station traffic for many remote
IP or MAC destinations still uses one associated BSSID.

Scheduling is work-conserving:

- if an eligible queue has work and physical resources permit progress, the
  radio does not wait for a full BA window;
- a sparse packet is sent within a bounded service interval;
- a saturated selected queue may produce a contiguous burst up to its BA,
  airtime and physical-credit limits;
- an empty or ineligible queue returns unused opportunity immediately;
- no global FIFO allows one peer to create head-of-line blocking for another.

BA32 is an upper bound for the current AP TX aggregate, not a minimum batch
size. RX BA16, TX BA32 and the number of DMA descriptors are separate
resources and must not be conflated.

## Fairness policy

Fairness is defined in airtime, not packet count or bytes. The production
policy is hierarchical:

```text
radio airtime
  -> weighted VIF service
     -> weighted peer service
        -> traffic-class latency policy
```

The VIF tier prevents an AP with many clients from receiving a multiple of the
STA VIF's weight merely because it owns more active queues. The peer tier uses
estimated airtime for admission and terminal actual airtime, including retry
cost where observable, to correct the deficit.

Traffic class and fairness are orthogonal. A latency-sensitive class may be
served sooner, but it does not receive unlimited airtime. Generic network
metadata carries priority intent; the Wi-Fi layer maps that intent to TID,
access category, EDCA and BA state. The stack must not infer application type
or contain WMM policy.

The scheduler must separately account for:

- queued software work;
- estimated airtime admitted below the scheduler;
- physical SRAM/DMA occupancy;
- terminal completed, retried, dropped and acknowledged work;
- actual airtime charged to the VIF/peer/class.

## Backpressure and bounded latency

When instantaneous producers exceed radio capacity, the system must choose a
bounded combination of queueing, upstream backpressure and drop. It must not
create unbounded PSRAM queues.

Required limits are expressed in bytes and/or estimated queueing time at both
per-domain and global levels. Admission policy must prevent bufferbloat while
preserving a small burst horizon for aggregation. A later AQM policy may refine
the drop choice; it cannot change the ownership contract.

Required progress cases include:

- one saturated peer plus one peer sending a small burst every five seconds;
- two or more peers with sub-ceiling offered loads;
- all peers sending only occasional packets;
- a peer entering and leaving power save with queued traffic;
- peer teardown while software, granted and physical work exists;
- loss of a VIF while the other VIF remains active;
- physical-credit exhaustion followed by exactly one wake-worthy return.

No case may wait indefinitely for a full aggregate or a new enqueue event.

## Power-save, multicast and reserved work

Sleeping-peer data remains in bounded software storage and is ineligible for
ordinary physical admission until the role policy permits it. It cannot hold
ordinary SRAM execution credits.

Group/multicast traffic is a per-VIF scheduling domain and obeys AP DTIM and
power-save rules. Authentication, association, EAPOL, BA/BAR, beacon-related
work and essential network-control replies use explicitly typed, bounded
reserve classes. A bulk stream cannot consume their complete reserve, and
reserved work cannot grow into an unbounded parallel datapath.

## Core and executor requirements

The current implementation may split the network stack and radio owner across
cores. The protocol must also permit a later single-core merge.

- Cross-core communication is bounded and non-blocking.
- Control traffic is proportional to active queues and radio quanta, not to
  packets.
- Wakeups are backed by durable level state. A wake is a hint, not the sole
  record that work exists.
- A consumer arms its waiter and rechecks durable state before sleeping.
- A bounded runner yields after a configured work quantum and self-wakes when
  only its cooperative budget, rather than a real resource, stopped progress.
- Reducing Core0 work by moving the same or greater work to Core1 is not an
  optimization. Both cores and total cycles per delivered unit are measured.

## Resource and performance gates

The exact numerical gates are scenario properties in HIL. The architecture
must support these project-level acceptance rules:

- no packet loss, duplication, unexpected reordering, ownership fault, DMA
  overflow or lifecycle mismatch in a passing correctness run;
- no internal-SRAM growth proportional to peer count, queue depth or BA window
  count;
- at the agreed HT40 MCS7 ceiling, Core0 radio work remains below 40% of one
  core in a low-intrusion measurement image;
- Core1 and total CPU work do not regress merely to satisfy the Core0 gate;
- a one-peer AP and station retain the established ceiling band after cutover;
- two-peer and multi-peer aggregate throughput approaches the same physical
  radio ceiling, subject to airtime and protocol overhead;
- sparse-service latency is measured independently of saturation throughput;
- AP+STA reports per-VIF throughput and airtime shares, not only their sum;
- HE20 station behavior is measured separately from HT40 expectations.

Task residence is useful diagnostic evidence but is not, by itself, calibrated
CPU utilization. Claims must also use normalized cycles/instructions, packet
counts and physical radio evidence where available.

## Required HIL matrix

Every production cutover must include clean, observer-free runs and narrowly
instrumented diagnostic companions.

| Role | Traffic | Required load shapes |
|---|---|---|
| STA HT40/HT20 | RX, TX, bidirectional | ceiling, stepped load, sparse |
| AP one peer | RX, TX, bidirectional | ceiling, stepped load, sparse |
| AP 2/4/8/15 associated | RX, TX, bidirectional | equal saturation, unequal load, one saturated plus sparse |
| STA+AP | RX, TX, bidirectional on both VIFs | symmetric and asymmetric saturation |
| HE20 STA | RX, TX, bidirectional | moderate and ceiling-seeking load |
| AP power save | TX and bidirectional | sleep, wake, PS-Poll/trigger, DTIM/group |
| lifecycle | mixed | reassociation, disconnect, role stop/restart |

For large associated-peer counts, not every peer must be saturated in every
run. The matrix must distinguish associated count from active and saturated
queue count.

Each report records the exact source revisions and dirty patches, dependency
revisions, feature/configuration set, firmware and runner artifacts, channel,
width, MCS/GI evidence, offered load, topology and both endpoints' routes.

## Evidence rules

- A same-image A/B establishes the effect of its runtime selector only.
- A feature-on/off rebuild is not a same-image A/B and can be affected by code
  layout.
- Zero error counters exclude only the errors they count.
- A full BA32 does not prove absence of inter-aggregate idle gaps.
- A throughput ceiling can hide large differences in CPU cost.
- A diagnostic observer's cost is measured before its results are used for a
  performance claim.
- Historical runs remain evidence only with a reconstructable matching host
  runner and configuration.

## Revision policy

This document changes only when desired externally observable behavior or a
hard ownership/resource invariant changes. Implementation status belongs in
[`WIFI_EGRESS_STATUS.md`](WIFI_EGRESS_STATUS.md), the design rationale in
[`WIFI_EGRESS_ARCHITECTURE.md`](WIFI_EGRESS_ARCHITECTURE.md), and ordered work
in [`WIFI_EGRESS_CUTOVER_PLAN.md`](WIFI_EGRESS_CUTOVER_PLAN.md). Git history
and HIL reports, not append-only prose, are the experiment archive.
