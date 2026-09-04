# Wi-Fi network integration plan

This is the active implementation order for the architecture in
[`WIFI_EGRESS_ARCHITECTURE.md`](WIFI_EGRESS_ARCHITECTURE.md). Work is complete
only when ownership, liveness, resources and performance are demonstrated.
Micro-optimization does not block structural cutover unless a regression makes
the path unusable or obscures correctness.

## Phase 0: architecture and evidence checkpoint — complete

- [x] Audit Embassy driver APIs, Xarxa ownership and the public grant prototype.
- [x] Compare the proposed topology with NAPI, AF_XDP, DPDK and mac80211/mt76
  principles.
- [x] Select three independent integrations around one radio-native core.
- [x] Define shared ownership, memory, wake and fairness invariants.
- [x] Preserve the pre-cutover implementation and evidence in Git.

## Phase 1: owned network foundation — complete

- [x] Add owned `PacketBuf` transfer to the maintained Xarxa/Embassy forks.
- [x] Make failed TX submission return the exact owner.
- [x] Add level-backed pool/waker behavior and bounded cooperative polling.
- [x] Move product STA, AP and concurrent network composition to the owned API.
- [x] Make Core0 receive general packet owners instead of borrowed/token-backed
  final DMA slots.

## Phase 2: physical promotion cutover — complete in code

- [x] Separate the fixed internal-SRAM execution pool from network tokens.
- [x] Select software flow before SRAM admission.
- [x] Reserve an entire physical batch before removing source owners.
- [x] Use the same owned source for the first and subsequent aggregate frames.
- [x] Keep the source owner until the one promotion copy succeeds.
- [x] Return SRAM only at terminal radio completion.
- [x] Preserve STA/AP/AP+STA unit behavior.

This phase is not performance-qualified until Phase 5.

## Phase 3: remove the rejected prototype — complete

- [x] Delete public egress candidates, grants, shadow state and peer catalogs.
- [x] Delete the mixed pinned network driver and obsolete tests.
- [x] Delete staging-copy/Core1-materializer compile-time branches.
- [x] Remove no-op HIL selectors and scenarios.
- [x] Reduce TX telemetry to the current Core0 promotion boundary.
- [x] Compile the compatibility adapter against released
  `embassy-net-driver 0.2.0`.

## Phase 4: finish integration boundaries

### 4A. Separate crates

- [x] Define adapter-neutral network values in a no-std crate with no driver,
  queue, executor or allocator dependency.
- [x] Create an upstream-clean compatibility adapter crate.
- [x] Add a separate ESP32-S31 compatibility bridge from the unchanged
  released driver to the shared selected-burst/SRAM materializer contract.
- [x] Keep the owned Xarxa adapter in a distinct crate with only the two forked
  dependencies it requires.
- [x] Move physical SRAM execution types into the radio/ESP32-S31 adapter layer,
  not the generic Embassy compatibility crate.
- [x] Ensure Cargo feature unification cannot pull owned/research code into the
  compatibility binary.
- [x] Add compile and dependency checks for each adapter independently.
- [x] Compile the radio policy/materialization crate without the owned network
  feature and reject Xarxa/owned-Embassy dependencies in that normal graph.

### 4B. Shared radio-native ingress

- [x] Define one internal selected-burst/materialization trait owned by
  open-radio.
- [x] Make current owned software queues implement it without exposing Xarxa
  types to radio policy.
- [ ] Remove the remaining trait-object call from the measured hot path if HIL
  shows meaningful cost; otherwise retain the simpler boundary.
- [x] Make STA, AP and paired services consume the same physical materializer,
  with role-specific peer/lifecycle policy outside it.

The completed boundary consists of `SoftwareTxFrame`, `MaterializedTxFrame`
and `SelectedBurstMaterializer`. Radio services no longer carry an Embassy
mutex, queue depth, frame layout or resource lifetime in their trait contract.
The current owned adapter supplies those details only in its concrete
implementation. `DatapathNetwork` likewise contains only associated
capability/owner types; the old adapter geometry parameters have been removed.

## Phase 5: owned-path HIL qualification

Do not tune scheduler policy before the baseline matrix is captured.

1. Build clean observer-free and narrowly instrumented artifacts from one
   commit.
2. Record channel, route, PHY/MCS/GI/BA, ELF/BIN, dependency SHAs and dirty
   patch state.
3. Run STA and one-peer AP RX-only, TX-only and bidirectional ceilings.
4. Run AP with two clients: equal saturation, unequal offered rates and one
   saturated plus sparse traffic.
5. Run AP+STA with external AP and external station.
6. Measure both cores and total cycles per datagram/byte, not Core0 alone.
7. Capture aggregate sizes, inter-aggregate gaps, retries, queue occupancy and
   promotion phases.
8. Compare with the last reconstructable pre-cutover artifacts; do not compare
   isolated headline numbers from different channels or host routes.

Pass conditions:

- no ownership/lifecycle/DMA errors;
- sparse traffic does not wait for a full BA window;
- no SRAM growth with peer count;
- single-peer throughput remains in the established physical ceiling band;
- Core0 remains below the project gate in the low-intrusion image;
- Core1 and total CPU do not regress without an explained new service.

If the baseline fails, first locate the exact stage using counters and same-ELF
controls. Do not change memory size, radio policy or PHY and call the result a
fix without isolating the cause.

## Phase 6: fairness policy

- [ ] Add durable active flow state by VIF/peer-generation/TID.
- [ ] Implement hierarchical VIF then peer airtime DRR.
- [ ] Add estimated pending-airtime limits before physical admission.
- [ ] Reconcile terminal actual airtime, retry and drop outcomes.
- [ ] Add bounded latency policy for sparse and latency-sensitive work.
- [ ] Keep control/management reserve typed and bounded.
- [ ] Make sleeping peers ineligible without consuming ordinary SRAM.
- [ ] Add group/DTIM handling and association teardown invalidation.
- [ ] Validate 1/2/4/8/15 associated clients while keeping one fixed SRAM
  ceiling.

The first fairness implementation may use simple conservative airtime
estimates. Correct ownership and work conservation precede estimator tuning.

## Phase 7: compatibility product integration

- [ ] Compose the upstream-clean adapter into an example/product target without
  patched Xarxa or Embassy.
- [ ] Route completed frames through the same radio scheduler and fixed SRAM
  execution pool.
- [ ] Qualify UDP, TCP, raw and control traffic under bounded queue exhaustion.
- [ ] Publish its explicit extra-copy/RAM/CPU envelope.

This adapter is allowed to be slower. It is not allowed to have different peer,
BA, retry or fairness semantics.

## Phase 8: research datapath

- [ ] Create a separate research crate/composition that reuses production
  hardware leaves.
- [ ] Start with a synchronous fused Core0 Ethernet/ARP/IPv4/ICMP/UDP engine.
- [ ] Add the same engine in split-core mode using ownership-transferring batch
  SPSC queues.
- [ ] Sweep SRAM caps, batch sizes and payload sizes in one runtime-selectable
  artifact where possible.
- [ ] Compare CPU copy, GDMA copy/overlap and direct final-SRAM materialization.
- [ ] Add per-key scheduling, then airtime policy.
- [ ] Add TCP only after the hardware/radio/copy lower bound is known.

The research path has no Xarxa/Embassy compatibility requirement, but all
memory, generation, control-liveness and radio correctness requirements still
apply.

## Phase 9: promotion decisions and optimization

For each successful research result choose exactly one:

- radio-generic and beneficial: move it into the shared core;
- generic owned-driver capability: propose a small independently valuable
  Xarxa/Embassy change;
- hardware-specific: keep it in the ESP32-S31 layer;
- unhelpful or too costly: record evidence and remove the path.

Only now optimize remaining hotspots. Required attribution separates fixed
runner-entry, per-frontier, per-frame, per-byte, copy, crypto, publication and
completion costs.

## Phase 10: documentation and HIL maintenance

- [ ] Audit the complete HIL module after the network cutover.
- [ ] Split oversized protocol/runner/runtime modules along ownership domains.
- [ ] Remove stale scenario fields, report parsers and terminology.
- [ ] Keep current design/status/plan documents concise; use Git and
  qualification records as the history.
- [ ] Ensure replay manifests retain source/dependency commits, patches,
  artifacts, features and host network topology.

## Stop conditions

Stop a phase and diagnose before proceeding if:

- an owner can be lost, duplicated or retargeted after generation change;
- a bounded queue can deadlock because a wake is edge-only;
- a partial batch consumes source owners without complete destination credit;
- control traffic can be starved by bulk work;
- an adapter-specific type enters the shared radio policy;
- SRAM grows with active or associated peer count;
- a performance claim lacks both-core and physical-radio evidence.

## Definition of complete

The refactor is complete when all three adapters have explicit independent
builds, the optimized product meets the full HIL matrix, fairness is based on
bounded airtime rather than frame count, the research oracle quantifies the
hardware lower bound, and current documentation contains no alternate legacy
architecture presented as live code.
