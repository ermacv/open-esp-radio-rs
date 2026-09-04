# Wi-Fi driver and egress-refactor audit, 2026-09-04

Status: current decision record before further implementation. This audit
compares the original fairness requirements, the three-repository source
state and the available HIL evidence. It does not qualify the final
architecture.

## Executive verdict

The target architecture remains valid, but the current implementation is not
ready for a full cutover.

The accepted architectural boundary is still:

```text
stack-owned bounded payload/work backlog
    -> opaque device egress key
    -> interface-wide demand catalogue
    -> Core0 radio scheduling grant
    -> direct final construction in fixed internal SRAM
    -> MAC / CCMP / A-MPDU / DMA
    -> terminal BA/retry/rate receipt
```

This separates software queue capacity from the short physical radio working
set and avoids both a DMA-SRAM allocation per peer and a second complete-frame
PSRAM staging copy. It is the correct adaptation of mature per-station/per-TID
TXQ scheduling to the S31 memory constraint.

Three conclusions must not be overstated:

1. The topology is supported by causal experiments; the current control path
   is not yet efficient enough.
2. A task-residence percentage is not a calibrated physical CPU-utilization
   measurement.
3. Full BA32, zero retries and zero lifecycle errors exclude several failure
   classes, but do not prove that the radio environment or every scheduling
   boundary is innocent.

The immediate priority changes after the latest HIL. Before TCP coverage,
terminal airtime accounting or fairness policy is extended, the current
`118.5 -> 90--93 Mbit/s` asymmetric AP regression must be localized with a
three-arm same-image control. Continuing to add protocol functionality on a
Core1 path already near saturation would make causality worse and could bake
an avoidable cost into the provider API.

## Exact repository state

At the time of this audit:

```text
open-esp-radio-rs-wifi:
    branch  refactor/wifi-interface-egress-scheduler
    commit  efef18360c7ec47d55f32f47884dbbba6bcb8e69
    remote  origin/refactor/wifi-interface-egress-scheduler (pushed, clean)

Xarxa:
    branch  refactor/interface-egress-scheduler
    commit  0cdbfc427a062ba533781ee31fbf75a45dcb679a
    remote  origin/refactor/interface-egress-scheduler (pushed, clean)

Embassy:
    branch  refactor/interface-egress-scheduler
    commit  04dbb95a80145a92f959a3a39a8eb348fd919356
    remote  origin/refactor/interface-egress-scheduler (pushed, clean)
```

The open-radio feature branch is currently 55 commits ahead of and 52 commits
behind `origin/main`, with merge base `8905f4e8`. Most new main-side work is in
Bluetooth and Blobray, but merging it now would change code layout and the
source boundary while the Wi-Fi regression is still unlocalized. The branch
is safely pushed; first preserve the current causal measurement, then merge
main and repeat the clean baseline.

The current dependencies passed:

- Xarxa scheduling-feature tests: 220;
- Xarxa default workspace tests: 675;
- Embassy feature-off and feature-on checks plus bridge tests;
- open-radio scheduling adapter: 26 unit, 33 device and 3 stack tests;
- full open-radio workspace tests;
- formatting and diff checks;
- the complete source-only audit, including the RISC-V HIL target, placement,
  stack-frame, source-graph and direct-target checks.

These establish build and host-model correctness. They do not supersede HIL
performance or radio-behaviour gates.

## Current physical driver

### RX ownership

The production RX path uses 96 physical descriptors/buffers. At most 32
original buffers may be retained above the Core0 radio owner, leaving 64 in
the radio domain. This is a bounded retained-buffer design: those 64 are not
promised to be armed at every instant while a masked continuation is pending.

The driver masks the RX interrupt, performs a bounded synchronous Core0 drain,
reposts a level-like continuation when work remains, and unmasks only after
the completion frontier and recycle obligations are clear. This is the
intended NAPI-like ownership rule. The previous copy/immediate-release and
dynamic replacement experiments did not improve the measured ceiling and
added Core0 work; replacement/page-pool is therefore not the current
performance direction.

The RX design is not part of the present TX regression, but it still lacks a
new full AP/STA/STA+AP bidirectional regression after the latest Xarxa pins.
Zero `BUFFER_FULL`/`FIFO_OVERFLOW` counters show absence of catastrophic DMA
exhaustion in a run; they do not by themselves measure retained-credit
pressure.

### TX ownership and memory

The production TX pool is fixed at 67 DMA-visible internal-SRAM slots:

```text
32 current aggregate
+ 32 next/prepared aggregate
+ 2 permanent network-endpoint reserves
+ 1 bounded control reserve
= 67 slots
```

The interpretation as exact physical banks is descriptive, not a proved
occupancy invariant. The important invariant is that the pool is global and
does not grow by one BA window per associated peer.

Queued UDP payload remains in the Xarxa packet arena in PSRAM. After selection,
Xarxa constructs the final Ethernet/IP/UDP frame directly in one SRAM token.
There is no accepted complete Ethernet frame in PSRAM followed by a second
terminal copy. Direct Wi-Fi DMA from cached PSRAM was tested and rejected by
the reviewed address-window experiment. CPU and GDMA one-copy variants remain
useful controls but failed the combined throughput/Core0/Core1 gate.

Core0 owns physical radio facts: VIF, peer generation, power-save state, BA,
rate, retries and hardware completion. The shared datapath owns RX/TX service
cadence, while STA and AP retain role-specific protocol and frame-building
logic. This is an appropriate split: duplicating association semantics would
be wrong, while duplicating queue/admission mechanics would also be wrong.

Current AP bulk aggregation is effectively best-effort TID 0 with BA32 TX
aggregation. The generic RX reorder resource remains BA16. Those are different
directions and resources; neither number should be generalized into a global
"Wi-Fi BA size".

## Current Xarxa and Embassy refactor

### What is implemented correctly

- `EgressKey` is opaque to Xarxa; the device maps a resolved route to a
  physical scheduling domain. Multiple Internet/IP destinations behind one
  STA gateway therefore share one radio key.
- UDP packet owners enter an unclassified intrusive list, are resolved at the
  interface boundary and move without payload copy into per-key FIFO lists.
- Final SRAM admission distinguishes `GlobalExhausted` from `KeyDeferred`, so
  global credit pressure does not rotate and fragment the selected burst.
- The demand identity contains schedule epoch plus activation, preventing an
  old grant from authorizing a new lifetime of the same key.
- Core1 publishes coalesced bounded demand/progress records. Core0 returns a
  current plus standby affine grant. This is a burst protocol, not one
  inter-core round trip per packet.
- `transmit_granted(serial)` leaves the pinned driver as the final SRAM
  authority and validates the exact live grant rather than accepting a racy
  current-peer hint.
- The UDP IP-destination cardinality bug, interface catalogue silent overflow,
  feature-off RAM regression and Embassy missing feature guard found by the
  external reviews are fixed and covered by tests.
- A terminal current grant no longer blocks a spendable standby merely because
  the Core1-to-Core0 progress stream is temporarily full.

### What is only a vertical slice

`Authoritative` currently means authoritative for catalogued UDP, not for the
entire interface:

```text
UDP            catalogued, grant-authoritative
DHCP/DNS/ICMP  bounded control reserve
TCP/raw        UncataloguedBulk, ordinary transmit bypass
```

This is acceptable only as an experiment. A final mode may not silently allow
bulk protocols to bypass the physical scheduler. TCP demand must describe
currently emit-able transport work, not all queued or unacknowledged bytes;
cwnd-, peer-window- or timer-blocked bytes must not retain a radio grant.

The present control policy is also not final fairness:

- the actual VIF selector still has historical frame-count behaviour;
- the portable DRR/AQL model and role-derived opportunities are partial;
- traffic class is currently zero and AP is TID0-only;
- grant completion means stack materialization completion, not terminal
  BA/retry/rate completion;
- grant serial is not yet retained through every physical aggregate path;
- it is not proved whether an aggregate is always serial-homogeneous;
- 16 active keys cover the current TID0 domain, not 15 peers multiplied by
  several active WMM classes.

### Architectural debt which should be removed at cutover

- `tx-egress-metadata` now names a scheduling protocol, not merely metadata;
- the optional demand/grant protocol is embedded in the base driver trait
  instead of an explicit typed capability;
- `airtime_hundred_nanoseconds` crosses Xarxa although Xarxa does not use it;
- `transmit_control()` encodes a hidden policy where a generic typed admission
  class would be clearer;
- Xarxa and Embassy mirror protocol types manually and need full conversion
  conformance tests;
- serial/epoch/activation wrap requires an explicit no-ABA contract;
- the current same-image selector, StackSelected fallback, old shadow
  telemetry and `UncataloguedBulk` are diagnostic/transitional code, not
  permanent architecture.

These are real cleanup tasks, but none currently explains the measured speed
regression. The proposed wake-suppression liveness bug also remains unproved;
it needs adversarial state-machine tests rather than a speculative fix.

## Comparison with the original requirements

| Original requirement | Current state | Verdict |
|---|---|---|
| AP with multiple clients | two-client equal-flow and saturated-plus-sparse HIL exist; latest asymmetric ceiling regressed | incomplete and currently failing performance |
| simultaneous STA+AP | shared physical owner and typed VIF evidence exist; authoritative airtime fairness does not | partial |
| HE20 STA | moderate historical coverage only; no complete new ceiling/asymmetry matrix | incomplete |
| RX-only, TX-only, bidirectional | many historical scenarios exist, but the current dependency set has not passed the full matrix | incomplete |
| saturation from both sides | supported by HIL machinery, not requalified for the current refactor | incomplete |
| airtime rather than packet fairness | host model/shadow estimation exists; terminal accounting and authoritative policy do not | not implemented in production |
| fixed SRAM independent of peers | 67-slot global TX pool and 96-entry RX ring retained | achieved for current UDP topology |
| no sparse-peer starvation | historical true sparse run passed; current low-rate grant run used coarse BA-sized application bursts | requires a new true sparse gate |
| Core0 below 40% at ceiling | shown for earlier and current single-peer diagnostic images; not shown for latest asymmetric AP run | not a general current claim |
| no hidden transfer to Core1 | latest Core1 task residence is near saturation | failing/no-regression blocker |

The old requirements document contains historical wording which calls the
IP-destination selector a production cutover and describes a complete-frame
PSRAM spill for contended traffic. The canonical checkpoint supersedes those
parts: current queues are device-key based and normal emission constructs
directly in SRAM. The original behavioural scope and fairness definitions
remain valid.

## What the current HIL actually proves

### Clean single-peer same-image gate

Clean commit `4385769b` used one exact application image for enabled and
disabled task-residence runs:

| Metric | enabled | disabled | delta |
|---|---:|---:|---:|
| host throughput | 115.943 Mbit/s | 116.884 Mbit/s | -0.805% |
| Core0 radio-task residence | 32.691% | 31.268% | +1.423 pp |
| Core1 network + UDP-TX residence | 79.201% | 85.705% | -6.504 pp |

The throughput gate passes; the strict Core0 `+1 pp` overhead gate narrowly
fails. The apparent Core1 residence reduction is not a CPU saving. Intrusive
phase counters instead show approximately +294 measured Core0 and +211
measured Core1 cycles per frame in enabled mode. Task residence includes time
inside a poll while interrupts preempt it.

The serial observer saw 3,253--3,277 complete grant lifecycles with no
collision, mismatch or incomplete close. Core0 issued a successor about 50 us
after receiving progress. The roughly 6.2 ms grant lifetime overlaps the
standby grant and must not be labelled radio idle time.

### Latest asymmetric AP regression

The historical clean-enough diagnostic image `1788172342431-003c5c6b`
delivered 118.526 and 118.553 Mbit/s to the saturated primary peer while ten
sparse packets reached the second peer. The latest exact application image
produced:

| Current mode | Run | Saturated primary | Sparse peer | BA/lifecycle result |
|---|---|---:|---:|---|
| authoritative | `1788511506961-0025611c` | 89.996 Mbit/s | 10 packets | almost all BA32; all grants complete; no identity errors |
| control disabled | `1788511722390-00256ba1` | 92.480, 93.142 Mbit/s | 10 packets/cycle | almost all BA32; no host loss/reorder/duplicate |

Both current rows use application SHA-256
`8c8c76e0a134a566dcfaf2e0f6ffc7112808788127bf8f8f0cceb982bdadb04c`
and differ only by the runtime control selector. Disabling the Core0 grant
protocol therefore recovers only about 2--3 Mbit/s. It does not recover the
historical 118.5 Mbit/s.

This disabled control is narrower than its name suggests. It leaves keyed
Xarxa scheduling enabled and advertises `StackSelected`; only demand/grant
authority is disabled. It cannot exonerate or convict the new UDP key queues,
catalogue reconciliation or interface dispatch machinery.

The prepared-successor gap is especially informative. It is elapsed time from
one completed TX transaction to publication entry for an already prepared
successor, not CPU work:

```text
historical fast image:  about 49 us mean
current disabled:       about 93--99 us mean
current authoritative: about 170 us mean
```

Almost every successor is still a full BA32. The current failure is therefore
not explained by a reduced negotiated BA window, fragmented aggregates,
hardware overflow, retries or grant lifecycle corruption. It is a cadence gap
between full aggregates. The first roughly 44--50 us of regression exists even
without grant authority; the grant path adds another roughly 70--77 us in the
enabled diagnostic image.

Core1 evidence points in the same direction but is not yet a causal profile.
The historical fast cycle spent about 69.8 us of network-plus-producer task
residence per delivered datagram. Current disabled spends about 113 us and
enabled about 126 us. This is a material implementation regression or
environment-sensitive scheduling interaction, not an acceptable cost to hide
until after TCP is added.

The primary saturated laptop link has no independent retry/station dump in
these reports. Only the sparse OpenWrt client reports HT40 MCS7 SGI and zero
retries/failures. Radio environment remains a possible contributor, but it is
not the leading causal conclusion because the same current image changes
cadence and both-core work by software mode. It must still be measured rather
than dismissed.

### Replay limitation

Attempting to replay the old application image with the current runner failed
before the scenario because the firmware publishes HIL protocol 76 and the
current runner expects 78. This exposes a real evidence-system gap: archiving
only firmware is insufficient when the matching host decoder/runner is not
also executable or reconstructable.

This does not justify legacy compatibility in the production driver. The HIL
solution is to archive or reconstruct the matching runner/tool contract, or
to provide a protocol-specific replay adapter in host tooling.

## Current blockers, in order

1. **Unlocalized AP TX cadence regression.** The current asymmetric path is
   25--28 Mbit/s below its historical result. Grant authority explains only a
   small part.
2. **Core1 headroom.** Current asymmetric task residence is roughly 89% in
   disabled and about 96% in enabled mode. This is not calibrated CPU
   utilization, but it is a hard no-regression alarm and correlates with the
   lower cadence.
3. **Incomplete authoritative coverage.** TCP/raw bypass the scheduler.
4. **No terminal receipt.** Fair airtime/AQL cannot be reconciled from stack
   grant close.
5. **No production airtime policy.** Current policy does not yet implement the
   required VIF/peer/AC DRR, AQL and power-save semantics.
6. **Evidence gaps.** True isolated sparse latency, current bidirectional/RX,
   primary-client radio observation and old-protocol replay are incomplete.
7. **Branch divergence.** `origin/main` must be integrated, but only after the
   current regression boundary is captured.

## Revised execution plan

### Stage 0: freeze semantics and localize the regression

Use the current diagnostic ELF for three modes of the same asymmetric AP
workload:

```text
A: authoritative grants          existing result
B: keyed StackSelected           existing disabled result
C: ordinary FIFO / schedule None required next result
```

Mode C already has a diagnostic runtime selector
(`DirectDmaFifoDiagnostic`); only a matching saturated-plus-sparse scenario is
needed. This separates:

```text
A - B = demand/grant authority cost
B - C = keyed Xarxa queue/catalogue/dispatch cost
C versus historical = driver/layout/fixture drift
```

Record for every arm: exact image hash, both-core normalized phase cycles,
network polls, egress passes/token, prepared-successor gap, aggregate size,
BA/retry/errors, route/channel/width and primary plus secondary air/link
evidence.

If C restores roughly 118 Mbit/s, profile only permanent Xarxa boundaries:
demand reconstruction frequency, socket/provider scans, route/key lookup,
dispatch quantum and wake/credit exits. The likely expensive shapes are
visible in source but are not yet proven: catalogue reconstruction occurs at
each egress entry and again at grant completion, while a 32-frame grant is
consumed through four-frame socket-dispatch quanta.

If C remains near 93 Mbit/s, run the archived old image with its historical
runner from an isolated worktree. If the old pair is fast in the current lab,
bisect open-radio checkpoints; if it is also slow, investigate fixture/radio
state with independent monitoring before changing code.

No production API or fairness change is accepted during this stage.

### Stage 1: restore the permanent egress baseline

Optimize or simplify only the boundary proven by Stage 0. Acceptance requires:

- asymmetric saturated flow back near the established 117--119 Mbit/s band;
- all sparse packets with bounded measured latency;
- full aggregate efficiency except at real sparse/terminal boundaries;
- zero lifecycle, identity, sequence, DMA and radio errors;
- Core0 below 40% in the agreed diagnostic class;
- Core1 normalized work no worse than the pre-refactor permanent path.

After this checkpoint is clean and pushed, merge/rebase `origin/main`, resolve
without carrying obsolete compatibility, and repeat the exact gate.

### Stage 2: complete the provider model

Define one interface-owned provider contract and implement:

- UDP removable keyed owners (existing basis);
- TCP currently emit-able connection/segment work;
- raw keyed packet owners;
- typed bounded control admission.

Then remove `UncataloguedBulk`. Add TCP sparse, saturated TX and bidirectional
tests before calling the mode authoritative for the interface.

### Stage 3: bind admission to terminal radio completion

Carry grant serial in the final SRAM sidecar into the physical aggregate. First
measure whether aggregates are serial-homogeneous. If not, either end an
aggregate at a serial boundary and measure the cost, or retain a bounded list
of serial segments. Do not silently charge one aggregate to the wrong grant.

Core0 retains estimated airtime at issue and reconciles it from terminal
BA/retry/rate information. Remove the unused airtime value from the Xarxa
contract unless a real stack consumer appears.

### Stage 4: stabilize the no-compatibility API

After providers and receipts prove the semantics:

- split the optional scheduling capability from the minimal driver API;
- rename the feature to `egress-scheduling`;
- replace the special control method with a typed admission class;
- formalize wrap/no-ABA behaviour;
- complete cross-crate schema and adversarial wake tests;
- remove shadow, StackSelected and runtime selectors after the final same-image
  oracle has served its purpose.

### Stage 5: implement and qualify radio fairness

Implement bounded physical-radio-wide VIF -> AC -> peer/TID weighted airtime
DRR, AQL-like pending limits, power-save eligibility and control deadlines.
Choose an explicit active peer/AC capacity policy rather than multiplying 15
peers by every TID in fixed hot storage.

Run the original matrix: STA and AP RX/TX/bidirectional; one, two and several
AP peers; saturated plus sparse; mixed PHY rates; simultaneous STA+AP;
reassociation; power-save; multicast/control; and HE20 STA. Report throughput,
latency, aggregate efficiency, queue residence, drops, per-peer airtime and
normalized work on both cores.

### Stage 6: broader Core1 optimization

The broad stack optimization remains a separate project, but Stage 0/1 must
remove refactor-induced Core1 overhead now. After semantic cutover, calibrate
idle/busy accounting and profile Xarxa route, protocol, checksum, copy,
emission and queue costs. A future one-core merged stack/driver design remains
possible, but the present split-core design must first have correct ownership
and bounded work; moving work between cores is not optimization by itself.

## Final decision

Continue with the architecture, not with the current feature sequence.

The original design goal was correct: select a physical egress before scarce
DMA admission, keep a fixed shared SRAM horizon, preserve canonical upper-layer
ownership and let Core0 own radio policy. The external reviews found genuine
correctness holes around that design, and those holes were fixed.

The latest evidence changes the next action. The project must now stop adding
provider and fairness functionality long enough to explain the asymmetric AP
cadence and Core1 regression. The next code change should be diagnostic-only:
add the same-image FIFO arm, run the three-arm matrix and let that result choose
the implementation boundary to refactor.
