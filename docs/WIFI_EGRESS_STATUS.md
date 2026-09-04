# Wi-Fi network integration status

Status date: 2026-09-04. Code checkpoint: `647bb112` on
`refactor/wifi-owned-egress`. This document records facts at that checkpoint;
it is not an experiment diary.

## Executive status

The product composition has cut over from the experimental public
Xarxa/Embassy egress scheduler to owned packet transfer. Core1 transfers a
general-memory `PacketBuf` owner to Core0. Core0 classifies and queues it,
selects radio work, reserves a complete SRAM batch, and copies each selected
frame exactly once into the fixed DMA-visible execution pool.

The rejected candidate/grant protocol, old pinned network token driver,
staging-copy modes and Core1 materializer experiment have been deleted from
the current code and HIL protocol. Git history retains them.

Host tests and cross-compilation pass. No post-cutover hardware performance
result exists yet, so current throughput and CPU targets are not claimed.

## Repository boundaries

Current dependencies:

- optimized Xarxa owned-packet fork: `122e97146fc0a174ef3310f4526defc37663bed4`;
- optimized Embassy fork: `244b4a3b80cb2f8a02f17b698f0ef4614e5fc01d`;
- compatibility `embassy-net-driver`: released crate `0.2.0`.

The `open-esp-radio-embassy-net` crate currently contains three logically
separate pieces:

- a small copied-frame adapter for the unchanged Embassy driver trait;
- the optimized owned-`PacketBuf` network boundary;
- the physical internal-SRAM TX execution pool.

This is compile-time clean but not the final crate topology. Separating the
compatibility and owned integrations into distinct crates remains planned so
Cargo feature unification cannot couple them.

## Current optimized TX path

```text
Core1 Xarxa
    -> constructs one owned general-memory PacketBuf
    -> OwnedNetworkDevice transfers the owner
    -> bounded owner queue
================ core boundary ================
Core0 DatapathRunner
    -> software TXQ classifies VIF/peer-generation/TID
    -> selects one flow/burst
    -> reserves all requested SRAM slots
    -> removes selected owners only after reservation succeeds
    -> one copy per frame into fixed SRAM
    -> AP or STA encodes and publishes aggregate
    -> terminal completion returns SRAM credits
```

Established invariants:

- a failed owned submission returns the exact `PacketBuf`;
- no partial source prefix is removed when an SRAM batch cannot be reserved;
- AP and STA aggregate extension reads the same owned source as the first
  frame; it no longer consults the obsolete pinned queue;
- association generation is preserved through the radio flow key;
- general packet backlog and SRAM execution credits are independent;
- SRAM completion does not wake Core1 because Core1 admission is governed by
  the general packet pool, while the physical pool is Core0-local.

The current Core0 flow scheduler is bounded and work-conserving. It preserves
per-flow FIFO order and round-robins non-empty flows. It is not yet an airtime
DRR/AQL implementation.

## Current RX path

The product uses `OwnedRxPublisher`. Core0 protocol processing publishes an
owned general-memory packet into the Xarxa RX pool. This is a clear ownership
boundary but currently includes a copy.

The earlier retained/pinned RX experiments remain historical evidence only.
Zero-copy or adopted DMA backing is not part of the current optimized Xarxa
contract. It requires a separate heterogeneous-pool design and measurements.

## Removed prototype

The following are deliberately gone from production sources:

- `EgressCandidate`, `EgressGrant`, `EgressShadowGrant` and `EgressGrantKey`;
- public `StackSelected`, `Shadow` and `Authoritative` scheduling modes;
- stack-visible radio grant/airtime state;
- per-destination UDP scheduling indexes;
- `PinnedNetworkRunner` and the mixed 4.7k-line `pinned.rs` module;
- `tx-egress-scheduling`, staging-copy and Core1-materializer features;
- no-op HIL selectors and scenarios for those removed paths.

This closes the reviewed liveness/capacity problems instead of patching them:
there is no 16-IP-destination authoritative catalog, no silent catalog
overflow, and TCP/raw cannot bypass an allegedly authoritative UDP-only
scheduler because that scheduler no longer exists.

## Verification completed

At checkpoint `647bb112`:

- `cargo check --workspace` passes;
- `open-esp-radio-embassy-net`: 9 owned unit tests and 3 compatibility tests
  pass;
- `open-esp-radio-esp32s31-wifi-embassy`: all 247 unit tests pass;
- product integration cross-checks for `riscv32imafc-unknown-none-elf` pass;
- the base HIL runtime cross-check passes;
- the HIL hardware/memory architecture feature set cross-check passes;
- the HIL scenario catalog validation tests pass.

Hardware was intentionally not touched during this cutover.

## Retained evidence, not current claims

Pre-cutover experiments established useful design inputs:

- driver-side per-flow queue geometry can recover multi-peer aggregation;
- increasing SRAM pool size can hide queue fragmentation but is not a scalable
  per-peer design;
- the tested Core1 materializer reduced throughput and increased total CPU;
- the tested dynamic RX replacement implementation increased Core0 work;
- SPSC handoff reduced measured per-packet transition cost;
- Wi-Fi DMA access to PSRAM is not established as a supported production path;
- the standalone GDMA memory-copy experiment did not establish an end-to-end
  product benefit.

Numbers attached to older ELFs remain valid only for those recorded artifacts.
They do not prove the performance of the current owned path.

## Known gaps

1. The post-cutover owned path has not been run on hardware.
2. Airtime DRR, pending-airtime limits and actual-airtime reconciliation are
   not implemented.
3. Multi-client, sparse-peer and AP+STA fairness need HIL evidence on the new
   path.
4. TCP and raw traffic share the owned API structurally but lack complete HIL
   qualification.
5. The compatibility integration is tested as a library driver but is not yet
   a separate product crate/composition.
6. The research integration is not implemented.
7. Core1 load after cutover is unknown. Moving work away from Core0 is not an
   accepted optimization without total-CPU evidence.
8. `DatapathTxConsumer` currently calls the owned source through a trait object;
   its cost must be measured before deciding whether Track B needs a generic
   static source type.
9. HIL module/document cleanup beyond removed TX selectors remains a separate
   audit item.

## Next decision gates

The immediate gate is correctness and resource qualification of the owned
one-copy path, not micro-optimization.

Only after clean single-peer and multi-peer measurements may the project decide
whether to:

- retain CPU promotion or revisit GDMA overlap;
- add a generic heterogeneous packet-owner API;
- add a direct-SRAM optimized API;
- move a research result into the production core;
- optimize Core1 or fuse the network and radio owners.
