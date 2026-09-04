# Wi-Fi egress refactor status

Status: audited implementation and evidence snapshot, 2026-09-04. This file
describes what exists now. It is not the target architecture and should be
updated by replacement, not by appending an experiment diary.

## Executive status

The project has a safely pushed, host-tested and HIL-tested UDP scheduling
vertical slice across open-radio, Embassy and the old Xarxa line. It proved
important identity, queueing, grant, batching and measurement semantics. It is
not the production cutover candidate.

The decisive audit result is that the Xarxa feature branch and current Xarxa
`main` are unrelated histories. Current `main` has already replaced the
borrowed token/socket-ring model with owned `PacketBuf`. That model permits
the per-key software TXQ to live inside the Wi-Fi driver after packet ownership
transfer. Porting the existing public demand/grant protocol before testing the
simpler path would be unnecessary architecture and long-term fork cost.

No further TCP/raw provider work, fairness-policy expansion or micro-
optimization should be added to the old line. The old implementation remains
an executable semantic oracle while the owned-buffer cutover is built.

## Repository snapshot

After the architecture checkpoint and Phase 1 host integration:

```text
open-esp-radio-rs-wifi
  branch: refactor/wifi-interface-egress-scheduler
  architecture checkpoint: 2549d37ee8fe71c6397401127d82bd3e8611935a
  integrated origin/main: d66b310167b4a3eae6bee12afea07d2b5f1fd3c5
  merge commit: d4cbc012
  post-merge contract-test fix: 57486b85
  synchronized documentation: 8ec3f2c0
  main-only commits at audit: 0

Xarxa scheduling prototype
  branch: refactor/interface-egress-scheduler
  HEAD:   3ac0e58e2c37e052ef168fdec8c3cf69c39824a4
  base:   old line at 1f332ac32cc33d86aefc8e1c1a9749b93234a6de

Xarxa current main
  HEAD:   9d32976c3f3349235bab4f91922b81e5b04326b3
  merge base with prototype: none

Embassy scheduling prototype
  branch: refactor/interface-egress-scheduler
  HEAD:   ab8d91a5ddd2d9a4596a74f4ea89acda66cace1d

Embassy current main
  HEAD:   98d847be57f3ea022ce05fe9b95ab3639a1e0a93
  Xarxa pin: old 1f332ac token-based line
```

The merge was conflict-free. Workspace tests exposed two stale Blobray CLI
fixtures already present in the integrated main tree: one expected the former
function-selector help spelling and one constructed obsolete revision schema
4. Commit `57486b85` updates those tests to the current typed selector and
authenticated schema-5 occurrence model. Formatting, workspace check/tests,
workspace Clippy and the complete source-only/direct-target audit pass after
that fix.

The attempted Phase 1 correctness smoke then stopped before flashing. The
post-LTO stack audit reported three regressions:

```text
run_station_access_point_active  50,432 -> 53,488 bytes
supervisor::new                  17,792 -> 18,384 bytes
ConnectedStaPort::compose         8,336 ->  8,464 bytes
```

The comparison uses archived correctness ELF run
`1788272869730-00138fba` and the current clean build. Both use Rust 1.97.1,
LLVM 22.1.6, the same stack policy and equal-sized
`ProductionWifiFault`, `ConnectedStationFault` and
`ProductionStationReclaimFault` types. Reintroducing the Bluetooth dependency
removed by the main merge produced the same current frame sizes. The merge is
therefore not the cause.

The material build difference is the old scheduling prototype dependency
advance:

```text
Xarxa    56840265 -> 3ac0e58e
Embassy  3b91c620 -> ab8d91a5
```

Those revisions add the interface scheduling state whose public architecture
this audit has now rejected as the production base. Refactoring radio terminal
owner graphs or raising stack limits solely to accommodate that retired path
would spend risk on code which Phase 5 removes. The synchronized branch is
therefore frozen and pushed as the executable oracle. The production cutover
starts on a new branch directly from `origin/main`; its own clean STA/AP HIL is
the Phase 1 target baseline.

The HIL wire protocol is currently 79. A historical firmware using an older
protocol cannot be replayed by the current runner merely because its binary
was archived; the matching host tool contract must also be reconstructable.

## Current physical Wi-Fi datapath

### RX

The shipping experimental branch uses 96 physical RX descriptors/buffers and
permits at most 32 retained packet owners above the Core0 radio path. The
remaining radio capacity is bounded but is not claimed to be 64 descriptors
armed at every instant.

RX service masks the interrupt, drains bounded work synchronously on Core0,
publishes a durable continuation if work remains and unmasks after the observed
completion/recycle frontier is clear. Copy/immediate-release and dynamic
replacement experiments did not raise throughput and added Core0 work. This
does not prove retained ownership has no backpressure effect in every workload;
it establishes that replacement is not the measured performance solution.

Migration to new Xarxa cannot preserve this RX behavior with its current
`PacketBuf`: the new buffer can only return to one private global pool. A
pool-aware/adoptable owner is required to let a driver-owned RX DMA buffer
cross the stack and return to its radio pool without a copy.

### TX

The current direct path owns 67 internal-SRAM slots:

```text
32 aggregate-scale current capacity
+ 32 aggregate-scale next/elastic capacity
+ 2 endpoint response reserves
+ 1 bounded control reserve
= 67
```

This decomposition is a sizing description, not a proven occupancy theorem.
The permanent property is a fixed shared pool independent of peer count.

AP and STA now share physical datapath mechanisms while retaining separate
role/peer/security policy. AP bulk TX is presently best-effort TID 0 and can
form BA32 aggregates. The generic RX reorder agreement remains BA16; these are
different directions and resources.

## Old Xarxa/Embassy scheduling prototype

### What is implemented and worth retaining

- Xarxa asks the device to canonicalize a resolved route into an opaque
  `EgressKey`.
- Multiple IP destinations mapped by the device to one link domain share one
  UDP queue key.
- Global physical exhaustion differs from deferral of one key.
- Demand identity uses schedule epoch plus activation to reject stale ABA
  reuse.
- The interface catalogue aggregates a key across providers.
- Demand and progress cross to Core0 as coalesced bounded state.
- Core0 returns current plus standby burst grants.
- `transmit_granted(serial)` performs final synchronous validation at the
  physical backing owner.
- Cooperative packet and state-only budgets prevent an executor turn from
  being captured indefinitely.
- Register-before-recheck wake logic exists in the concrete cross-core path.

### Review findings which are already fixed

The following findings from early external reviews do not describe current
prototype HEAD:

- the missing Embassy feature guard is fixed;
- UDP queues are keyed by device domain rather than IP destination;
- scheduling state and packet-index metadata are feature-gated;
- driver-declared `max_active_keys` is checked against compiled capacity;
- authoritative catalogue overflow fails closed;
- the feature-off/on checks exist;
- a blocked current completion no longer prevents local standby progress.

They remain useful regression subjects, not open blockers.

### Unresolved structural problems

The prototype is authoritative only for catalogued UDP:

```text
UDP             demand/grant path
DHCP/DNS/ICMP   bounded special control admission
TCP/raw         uncatalogued ordinary transmit bypass
other internal replies/fragments have additional direct paths
```

Other structural debt includes:

- scheduling is eight Cargo-gated default methods in the base Xarxa/Embassy
  driver traits;
- middleware can compile while silently dropping the scheduling capability;
- `EgressSchedule` mixes lifecycle, physical capacity, runner policy and
  diagnostic rollout mode;
- demand and grant-completion publication do not expose durable acceptance or
  backpressure in the generic API;
- feature-on UDP state contains an `O(sockets * max_keys)` fixed index;
- old ring storage needs `IndexedSlots` complexity for out-of-order removal;
- traffic class is effectively zero in the current production AP path;
- stack materialization completion is named like grant completion but is not
  terminal BA/retry completion;
- radio airtime crosses Xarxa even though Xarxa does not use it;
- `transmit_control` embeds a hidden policy instead of a typed reserve class;
- Xarxa and Embassy manually duplicate all protocol types;
- old shadow-grant telemetry and the new authoritative grant coexist;
- Xarxa selection is followed by another AP `RoundRobinTxQueues` selector.

These issues are not a to-do list for the old branch. Most disappear when the
software TXQ moves below the new owned driver boundary.

## Reconciliation with the original plan

The audit changes mechanism, not the problem statement.

| Original direction | Audit decision |
|---|---|
| Separate long software backlog from short SRAM/DMA horizon | retained as a hard invariant |
| Select peer/TID before scarce physical admission | retained inside the Wi-Fi driver |
| Per-peer/TID queues instead of a global FIFO | retained |
| Fixed SRAM independent of client count | retained |
| Core0 owns BA/PS/rate/airtime and terminal completion | retained |
| Xarxa sockets publish demand and consume radio grants | replaced for the first candidate by owned `PacketBuf` transfer and driver queues |
| Extend every protocol into an interface provider model | removed from the first candidate; all completed owners enter one driver boundary |
| Construct selected packets directly in SRAM | moved behind an evidence gate as an optional fast path |
| PSRAM contains a second Wi-Fi complete-frame staging copy | rejected; the general `PacketBuf` itself is the queued owner |
| TCP scheduler observes emit-able transmission opportunities | needed only if a future pre-materialization API is justified |
| Public Embassy scheduled-egress protocol | not needed for the one-copy candidate |
| Airtime DRR/AQL and terminal reconciliation | retained, but private to the link/radio owner |

This is why the earlier work was not pointless. It established the queue key,
lifecycle and physical-credit semantics and exposed the cost of per-packet
plumbing. What changes is the layer in which those semantics live.

## New Xarxa `main` audit

Current `main` makes the fundamental ownership transition:

```rust
fn receive(&mut self) -> Option<PacketBuf>;
fn can_transmit(&mut self) -> bool;
fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf>;
```

UDP resolves the route, checks device room, allocates one packet, builds UDP,
IP and Ethernet headers in place and transfers the completed owner. TCP keeps
canonical stream/retransmission bytes, checks device room, materializes the
currently eligible segment into a `PacketBuf`, and commits its state only on
the accepted path. Raw and internally generated traffic use the same owned
packet model.

This solves several prototype problems:

- the Wi-Fi driver can enqueue all accepted protocols uniformly;
- packet ownership can cross a core without copying the handle or payload;
- no per-protocol demand observer is needed for the first one-copy design;
- TCP eligibility remains in the TCP implementation rather than being
  duplicated by a scheduling observer;
- one driver-side per-key queue topology covers UDP, TCP, raw and control.

### Current new-main blocker

`PacketBuf` is not yet heterogeneous:

- it allocates from one private static pool;
- the handle stores only a pointer to that pool's private slot;
- `Drop` derives an index relative to that same global pool;
- the capacity and alignment are global build settings;
- an external driver-owned DMA buffer cannot be adopted;
- a caller cannot select PSRAM versus SRAM backing.

Consequently, placing the pool in SRAM makes all software backlog consume
scarce DMA memory. Placing it in PSRAM forces copies on both TX and RX. The
owned abstraction is the right foundation, but it needs origin-aware static
pools before it is a complete S31 solution.

## Measured facts retained from the experiment history

Only results which constrain the architecture are summarized here. Exact logs
and older intermediate implementations remain in Git and HIL artifacts.

### Multi-peer queue geometry

With early physical admission, one peer sustained roughly 124 Mbit/s while two
interleaved peers with 67 slots fell to roughly 97 Mbit/s. A 98-slot direct
control restored roughly 121 Mbit/s but consumed another 52,576 bytes of SRAM.
Scheduling a peer before physical admission restored 120--121 Mbit/s with 67
slots. This proves the queue-geometry/HOL problem and fixed-SRAM requirement.

### One-copy cost

The corrected persistent one-copy lower-bound added about 9.7 microseconds of
Core1 task residence for a 1,486-byte frame. In the driver-side scheduled
copy, one-copy with 67 physical slots restored 120--121 Mbit/s but raised
Core0 radio residence to about 61%, versus about 39.5% for the fragmented
direct control. The complete promotion transaction measured approximately
8.1--8.5 thousand cycles/frame; payload copy was the largest measured phase.

Moving the materializer to Core1 reduced Core0 below 40% but delivered only
90.62 Mbit/s and raised Core1 network-plus-UDP residence to 72.18%. It moved
and increased work; it was not an accepted design.

### AXI-GDMA

The custom AXI-GDMA proof copied PSRAM to SRAM correctly. A 32-frame
scatter/gather copy used fewer active CPU cycles/instructions than CPU copy,
but took 2.27 times as much wall time. Its measured active saving was about
417 cycles/frame, only around 5% of the complete promotion cost. GDMA remains
an overlap candidate, not a proven production improvement.

### Direct PSRAM DMA

A same-image run published validated `0x50xx_xxxx` PSRAM packet addresses with
cache writeback and delivered no UDP traffic. The internal-SRAM control in the
same image worked and reached 122.35--122.40 Mbit/s. Current Wi-Fi DMA backing
therefore remains internal-SRAM-only.

### Current refactor regression and observer cost

An older clean checkpoint reached 104.731 Mbit/s in the current lab with about
70.9 microseconds Core1 and 40.0 microseconds Core0 task residence per frame.
After removing duplicate route classification, the current StackSelected path
recovered to about 100 Mbit/s but remains slower and more expensive.

The latest exact-image identity-observer pair used firmware SHA-256
`b135fa38e3cafe548676d5a6c0fce65b2eaa6dcae6700a37a249b5f4d6fe2599`:

| Mode | HIL run | Primary throughput | Result |
|---|---|---:|---|
| observer off | `1788517419528-0026a31d` | 100.340 / 100.080 Mbit/s | sparse delivered, full BA32, no loss/reorder |
| observer on | `1788517866195-0026b96a` | 96.637 / 96.242 Mbit/s | sparse delivered, full BA32, no loss/reorder |

The observer alone costs roughly 3.7--4.1% throughput, about 1.5% Core0
instructions/frame and about 3% Core1 residence/frame. It is diagnostic-only.
Even with it off, current Core0 instructions/frame remain about 27% above the
old checkpoint. A full BA32 and zero counted errors do not explain the
inter-aggregate cadence regression.

These measurements justify preserving the old branch as an oracle. They do
not justify micro-optimizing it before the ownership-lineage cutover.

## Current claims and non-claims

Established:

- early SRAM admission causes multi-peer HOL/aggregate fragmentation;
- a fixed shared SRAM horizon is feasible;
- direct cached-PSRAM Wi-Fi DMA does not work in the tested path;
- one CPU copy can recover aggregation but is too expensive in measured old
  implementations;
- the current per-frame identity observer is materially intrusive;
- the old grant protocol preserves identity in its tested UDP vertical slice;
- no current evidence supports scaling SRAM by BA window per peer.

Not established:

- that the new owned-`PacketBuf` one-copy implementation has the same cost as
  the old staged implementations;
- that Core0 or Core1 percentages are calibrated total CPU utilization;
- that the radio environment explains the current regression;
- that current airtime DRR/AQL is production-correct;
- that 67 slots are the optimal or permanent count;
- that two in-flight grants are the correct permanent pipeline depth;
- that a public Xarxa scheduled-egress API is required;
- that AP, STA, AP+STA and HE20 currently pass the full required matrix.

## Document audit result

The former documents totalled more than six thousand lines and each mixed
current contracts, superseded plans and chronological measurements. Several
simultaneously called themselves canonical. That contradicted the repository
policy that Git history is the archive.

They are replaced by:

- [`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md): normative
  behavior;
- [`WIFI_EGRESS_ARCHITECTURE.md`](WIFI_EGRESS_ARCHITECTURE.md): target design
  and rationale;
- this file: current implementation and evidence;
- [`WIFI_EGRESS_CUTOVER_PLAN.md`](WIFI_EGRESS_CUTOVER_PLAN.md): ordered work and
  gates.

The HIL architecture and reproducibility documents remain separate because
they define tooling/evidence systems, not the Wi-Fi egress design.
