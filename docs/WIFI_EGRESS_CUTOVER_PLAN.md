# Wi-Fi egress cutover plan

Status: ordered implementation plan, 2026-09-04. The plan follows the decision
in [`WIFI_EGRESS_ARCHITECTURE.md`](WIFI_EGRESS_ARCHITECTURE.md). It deliberately
defers micro-optimization until one ownership model has replaced the parallel
prototype paths.

## Working rule

Every phase must name:

- the owner before and after each boundary;
- the old mechanism it removes or leaves isolated as an oracle;
- its correctness and resource gate;
- the evidence needed before the next phase.

A phase which only adds another parallel scheduler, queue, completion meaning
or runtime mode is not acceptable.

Performance sentinels run during cutover to detect catastrophic regression,
but we do not tune instruction layout, checksums, batch constants or individual
hot functions until the semantic cutover is complete. Diagnostic observers
are compiled only into named evidence images.

## Phase 0: architecture checkpoint

Status: complete at `2549d37e`; the document set is committed, audited and
pushed.

- Replace the overlapping append-only design journals with requirements,
  architecture, current status and this plan.
- Preserve the old Xarxa, Embassy and open-radio feature branches as pushed
  executable oracles.
- Record exact dependency SHAs and the absence of a Xarxa merge base.
- Stop feature expansion on the old Xarxa scheduling line.
- Refresh remote refs before starting code work.

Gate:

- documentation has one canonical source per purpose;
- repository link and source-only audits pass;
- no production behavior changes in the checkpoint commit.

## Phase 1: synchronize the production base

Status: the experimental integration branch is synchronized and all
host/source gates are complete through `8ec3f2c0`. Its target build exposed a
stack regression caused by the subsequently enlarged old Xarxa/Embassy
scheduling prototype. That branch is retained as the oracle rather than made
the production base.

The open-radio experimental branch contains `origin/main` at `d66b3101` and
preserves the pushed oracle history. Production work starts on a new sibling
branch directly from that `origin/main` commit, then carries only the canonical
architecture documents and unrelated main test correction. It does not carry
the 62 public demand/grant experiment commits merely to remove them later.

- Merge `origin/main` into the pushed feature branch without rewriting the
  oracle commits.
- Resolve only real conflicts; do not carry obsolete compatibility modes into
  main-side code.
- Run the workspace, formatting, Clippy, source-only and direct target checks.
- Build the experimental branch once to classify any pre-existing target
  regression. Do not raise resource limits or structurally optimize a rejected
  path to make this diagnostic build pass.
- Create the production sibling branch directly from current `origin/main`.
- Run clean STA and AP smoke HIL there before the Xarxa migration.

Gate:

- clean pushed oracle branch and clean production sibling based directly on
  current open-radio main;
- no lifecycle, placement or role regression;
- baseline report records exact source/dependency state.

## Phase 2: owned Xarxa/Embassy foundation

Create new sibling feature branches from Xarxa current `main` and Embassy
current `main`. Do not merge or cherry-pick the old scheduling implementation
as a block.

### Xarxa work

- Retain the current owned `Driver::{receive, can_transmit, transmit}` model.
- Design a safe, static, origin-aware packet-pool API.
- Make `PacketBuf` return to its creating pool on drop.
- Permit application-controlled placement and alignment of pool storage.
- Thread an explicit general allocator/pool through stack allocation sites;
  avoid a hidden global allocator as the only production composition.
- Permit the Wi-Fi driver to allocate RX packet owners from its dedicated DMA
  pool and pass them to the stack.
- Keep protocols independent of pool memory class.

The preferred implementation keeps one uniform `PacketBuf` type and stores
private pool origin in the slot/owner. An arbitrary external-slice callback API
is not the starting design because it makes lifetime and release safety hard to
audit.

### Embassy work

- Port the Embassy wrapper/runner to Xarxa's owned driver API.
- Keep the base Embassy driver API minimal; add no public radio scheduling
  methods in this phase.
- Make runner work budgets explicit immutable policy rather than S31 constants
  hidden in generic code.
- Bound RX drain and TCP/control work.
- Self-wake after cooperative budget exhaustion; sleep only on a real driver
  or timer block.
- Preserve register-before-recheck driver-waker semantics.

Host gates:

- pool exhaustion and exact reuse;
- two independent pools cannot free one another's slots;
- drop on another thread/core returns exactly one slot;
- stale/double-return is impossible in safe code;
- headroom, metadata and payload operations preserve origin;
- driver RX ownership survives queueing through a socket and returns to the
  RX pool;
- feature matrix for minimal, UDP, TCP, raw, async and all relevant mediums;
- plain drivers and wrappers still use only the plain driver contract.

Target gates:

- linker map proves general pool placement in PSRAM and RX DMA pool placement
  in internal SRAM;
- no generated binary embeds a large zero-initialized PSRAM pool as loadable
  flash data;
- RX buffer address/alignment and return path pass the existing placement and
  descriptor ownership audits.

## Phase 3: minimal owned Wi-Fi adapter

Implement the simplest complete one-copy TX owner graph.

```text
Xarxa general PacketBuf
  -> successful Driver::transmit ownership transfer
  -> bounded Core1-to-Core0 submission
  -> Core0 per-key software TXQ
  -> selected burst CPU-promoted into fixed SRAM slots
  -> existing MAC/CCMP/A-MPDU/DMA path
```

Requirements:

- `can_transmit == true` guarantees that the next transfer is accepted;
- the submission path transfers a handle, not packet bytes;
- classification creates a driver-private VIF/peer-generation/TID key;
- STA canonicalizes all ordinary egress to its associated radio peer;
- AP resolves unicast, group and stale/unassociated destinations explicitly;
- per-key and global software queue limits are bounded;
- queue overflow, stale generation and teardown have explicit drop counters;
- partial burst selection preserves FIFO order;
- copy failure returns or retains both owners without reordering;
- sparse work is submitted immediately without waiting for BA32;
- management and essential network control have a typed small reserve.

The initial scheduler may be round-robin, because this phase proves ownership,
not final airtime policy. There must be only one ordinary-data queue selector.
The old Xarxa interface selector, old shadow grant and current AP
`RoundRobinTxQueues` cannot all remain active in the new path.

Host gates:

- 1/2/4/15-key interleaving produces peer-homogeneous FIFO bursts;
- per-key rollback reconstructs the exact prefix;
- one saturated key cannot consume a reserved control slot;
- teardown drops only the old generation;
- all accepted UDP, TCP and raw packets traverse the same driver queueing
  boundary;
- queue wake is level-backed and survives the enqueue/arm race.

HIL correctness gate:

- STA, AP and AP+STA association/lifecycle smoke;
- one-peer and two-peer UDP TX;
- TCP smoke in each network role;
- saturated plus truly sparse AP peer;
- no loss, duplicate, unexpected reorder, ownership, DMA or lifecycle errors;
- RX smoke using the new pool-return path.

Performance sentinel:

- record throughput, both-core task residence, normalized cycles/instructions,
  copy cost, aggregate width/gap and queue occupancy;
- do not optimize failures yet; attribute them to a named phase first.

## Phase 4: complete physical scheduling semantics

Replace the temporary selector with the one Core0 radio policy.

- active queues are organized by VIF, peer generation/group and TID/AC;
- VIF-weighted and peer-weighted deficit scheduling is work-conserving;
- grant/opportunity size is bounded by queue depth, BA state, estimated airtime,
  AQL and free SRAM;
- power-save peers leave the eligible set without losing bounded backlog;
- group traffic obeys DTIM policy;
- control/management reserves remain bounded and typed;
- actual terminal BA/retry/rate evidence reconciles pending airtime and
  deficit;
- unused selected capacity is returned when the queue drains;
- association and role epoch changes invalidate queued and physical work
  without ABA.

Cross-core placement is selected here:

1. Measure Core0-owned queues and promotion.
2. If Core0 fails its gate, compare a Core1-owned queue/promotion variant using
   a private coalesced active/burst protocol.
3. Choose by total cycles and both-core headroom, not by moving work.

The private protocol, if needed, must make update acceptance durable, retain a
level snapshot across SPSC fullness and use queue/burst granularity.

Gates:

- deterministic state-machine/property tests for queue activation,
  reassociation, PS transitions, current/next opportunity, exhaustion,
  terminal completion and wake races;
- actual airtime is never completed by stack/copy completion;
- no software or DMA resource grows with associated-peer count;
- sparse-service deadline passes while another peer is saturated.

## Phase 5: remove the prototype architecture

After the owned path passes correctness gates:

- remove open-radio dependencies on the old Xarxa and Embassy scheduling SHAs;
- remove public `tx-egress-metadata`/grant-mode integration;
- remove old `EgressShadowGrant` per-frame telemetry;
- remove `StackSelected`, `Shadow` and `Authoritative` production selectors;
- remove `UncataloguedBulk` and special `transmit_control` integration;
- remove duplicate normal-data selectors and obsolete copy probes;
- retain only narrowly named diagnostic features that still answer a current
  question;
- update driver and feature documentation to the single owner graph.

The pushed old branches remain available for source and HIL comparison. No
runtime compatibility layer is retained.

Gate:

- source search finds no production reference to the removed protocol;
- all scenarios use the same production queueing topology;
- default builds do not pay RAM or code-size cost for retired diagnostics.

## Phase 6: full qualification

Run the matrix in
[`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md).

Minimum sequence:

1. single-peer STA and AP TX/RX/bidirectional;
2. two peers equal saturation;
3. one saturated plus one sparse peer;
4. two sub-ceiling peers and all-sparse traffic;
5. 4, 8 and 15 associated clients with active-count separated from associated
   count;
6. rate/retry asymmetry and power save;
7. simultaneous STA+AP symmetric and asymmetric load;
8. HE20 station;
9. TCP sustained and bidirectional runs;
10. teardown/reassociation during backlog and physical TX.

Acceptance requires the fixed-SRAM, correctness, latency, throughput and
both-core gates. Observer-free reports are the performance record; diagnostic
reports explain a failure but do not qualify it.

## Phase 7: decide whether a direct-SRAM API is needed

Only after the one-copy architecture is complete, use phase attribution to
answer:

- Is copy itself the dominant remaining total CPU cost?
- Does it prevent the Core0 gate, Core1 headroom or radio ceiling?
- Can AXI-GDMA overlap useful work without increasing inter-aggregate gaps?
- Is the cost present for TCP and bidirectional load, not only UDP TX?

First compare CPU and AXI-GDMA promotion in the same owned architecture. If a
copy remains the proved blocker, write a new ADR for the smallest
pre-materialization capability.

Possible capability scope:

- driver publishes bounded availability for an opaque egress domain;
- stack obtains an affine burst/backing permit;
- eligible UDP/raw owners and transactional TCP opportunities can consume it;
- final backing allocation and transfer are synchronous and infallible;
- all bulk protocols participate;
- middleware must preserve the capability explicitly or fail to compose.

Do not resurrect the old eight-default-method API automatically. The new API
must be justified against the owned one-copy baseline and compile-time prove
its capability.

## Risk register

| Risk | Control |
|---|---|
| New Xarxa main is not yet integrated by Embassy main | isolate sibling branches and land owned wrapper before radio cutover |
| Pool-aware `PacketBuf` grows handle or hot-path cost | measure layout and cycles; prefer origin in pool slot/control block |
| RX migration reintroduces a copy | make adopted DMA-pool owner a Phase 2 gate |
| General PSRAM pool causes cache pressure | measure cache/cycles and bound queue depth; do not infer |
| Core0 copy exceeds 40% | compare placement and GDMA only after correct owner graph exists |
| Core1 promotion saturates network work | measure total work; reject simple workload migration |
| Global `can_transmit` lets one peer fill software storage | per-key limits, AQM/drop and reserved capacity; add keyed pre-admission only if needed |
| Power-save queue blocks active peers | eligibility removal and no SRAM retention |
| Lost wake under bounded channels | durable level snapshots and arm/recheck tests |
| TCP state changes before failed admission | preserve new Xarxa transactional build/commit semantics |
| Old and new schedulers both remain | deletion ledger and source-search gate in Phase 5 |
| HIL observer changes result | paired observer-cost measurement and clean qualification image |

## Definition of complete

The refactor is complete when there is one production owner graph, all bulk
protocols use it, radio fairness closes on terminal evidence, SRAM is fixed,
the full role/load matrix passes and neither core has hidden unbounded work.
At that point optimization may change implementation cost, but not ownership,
queue identity or completion semantics.
