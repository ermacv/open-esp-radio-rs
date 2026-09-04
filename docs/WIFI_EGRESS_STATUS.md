# Wi-Fi network integration status

Status date: 2026-09-04. Active branch: `refactor/wifi-owned-egress`. This
document records facts after the integration-boundary cutover; it is not an
experiment diary.

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

The ESP32-S31 product now has two mutually exclusive compile-time network
compositions. `owned-network` remains the default optimized fork-backed path;
`compat-network` uses released `embassy-net 0.9.1` and
`embassy-net-driver 0.2.0`. Selecting both or neither is a compile error. Both
instantiate the same physical radio runner and fixed SRAM TX horizon.

## Repository boundaries

The network boundaries are now separate crates and dependency graphs:

- `open-esp-radio-network` contains only adapter-neutral interface/link/error
  values and has no network driver, queue, executor or allocator dependency;
- `open-esp-radio-embassy-net-compat` implements only the copied-frame
  released `embassy-net-driver 0.2.0` contract and has no Git or radio/DMA
  dependency;
- `open-esp-radio-esp32s31-wifi-embassy-compat` is the narrow bridge from that
  unchanged driver to the shared radio-selected burst and fixed SRAM
  materializer; it contains no Xarxa or owned-network dependency;
- `open-esp-radio-embassy-net` implements only owned `PacketBuf` transfer over
  Xarxa `122e97146fc0a174ef3310f4526defc37663bed4` and Embassy
  `244b4a3b80cb2f8a02f17b698f0ef4614e5fc01d`;
- `open-esp-radio-esp32s31-wifi-embassy` owns the product's fixed internal-SRAM
  promotion pool and telemetry; research has its own allocator composition
  over the same common pinned-DMA ownership primitive.

`tools/check-network-adapter-boundaries.sh` compiles each boundary independently
and rejects dependency leakage back into the compatibility or owned crates.
It also compiles the ESP32-S31 radio adapter with `--no-default-features` and
rejects Xarxa or optimized Embassy-network dependencies in that normal graph.

The ESP32-S31 radio adapter now exposes a private, static-dispatch boundary:

- `SoftwareTxFrame` is an independently owned, adapter-neutral Ethernet frame;
- `MaterializedTxFrame` is the final DMA-stable owner with explicit Ethernet
  geometry and conservative capacity bounds;
- `PhysicalTxSource` supplies final owners without requiring software Ethernet
  storage: it adapts the product materializer and research's prepared batch;
- `SelectedBurstMaterializer` owns synchronous, all-or-none movement between
  those domains.

The role-neutral runner, standalone and paired services, STA aggregate builder,
and AP multi-peer/power-save TX use these contracts. They no longer name
`OwnedNetworkTxFrame`, `DatapathTxConsumer`, an Embassy mutex, queue sizes or
the Xarxa resource lifetime. Those types remain only in the current owned
adapter implementation. A host regression test also materializes a non-Xarxa
software owner through the same physical pool. The shared `DatapathNetwork`
contract now uses only associated ownership/capability types; its former
adapter lifetime, mutex and queue/layout parameters have been deleted.
`SoftwareTxFrame` accepts affine pool leases whose storage lifetime is carried
by the concrete owner instead of demanding `'static` from every implementation.

The concrete STA TX implementation is now also generic over its physical frame,
including retained aggregate storage, retry and teardown. A host test transfers
the real research SRAM owners through STA encode, partial BA, retry and terminal
release; no software-frame materializer is implemented by the research batch.
This is a tested buffer seam, not yet an executor-neutral radio runner.

Shared ordinary TX and STA aggregate service now execute synchronously.
Timeout-abort returns `Pending` with retained state and a settle deadline;
late completion/repeated wakes cannot return credits before detach. Host tests
also prove that cancelling a polling wait preserves its in-progress ordinary
transaction. Existing Embassy adapters remain responsible for waiting. AP
aggregate service still has its own asynchronous settle path and is the next
state-machine cutover; the fused research runner is not yet implemented.

Each retained TX phase has one actionable deadline. An intermediate layout
with separate publication/settle deadlines failed the 50 KiB image-frame gate;
the compact phase representation passes at approximately 33 KiB maximum frame
in the current performance image. This is a build-time stack observation, not
runtime stack high-water or CPU evidence. The budget was not increased.

The compatibility endpoint separates payload storage from queue metadata.
STA/AP RX/TX payload arenas are placed in PSRAM by the product; channels carry
only unique mutable leases. A frame slot returns to its origin free queue on
ordinary consume, stale-link rejection, unused-token drop, radio
materialization and callback unwind. The adapter still performs the unavoidable
compatibility copy into final SRAM after radio selection.

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

At the current source checkpoint:

- `cargo check --workspace` passes;
- `open-esp-radio-embassy-net`: all 6 owned unit tests pass;
- `open-esp-radio-embassy-net-compat`: all 8 compatibility device/lifecycle
  tests pass;
- `open-esp-radio-esp32s31-wifi-embassy-compat`: all 3 shared-radio bridge
  tests pass;
- `open-esp-radio-esp32s31-wifi-embassy`: all 249 unit tests and 5 physical
  ownership/materialization boundary tests pass;
- `open-esp-radio-wifi-datapath`: all 4 queue/ownership tests pass;
- `open-esp-radio-research-datapath`: all 10 protocol/physical-source tests pass;
- `open-esp-radio-esp32s31-wifi-sta`: all 73 tests pass, including retained
  timeout service and polling-wait cancellation;
- `open-esp-radio-esp32s31-wifi-ap`: all 27 tests pass;
- the same radio crate without the optimized network feature passes 226 host
  unit tests and warning-free all-target clippy;
- product integration cross-checks for both mutually exclusive network
  compositions on `riscv32imafc-unknown-none-elf` pass;
- the base HIL runtime cross-check passes;
- the HIL hardware/memory architecture feature set cross-check passes;
- the HIL scenario catalog validation tests pass;
- `tools/audit-source-only.sh` passes, including the final 4 MiB HIL image,
  placement, stack-frame and forbidden-ROM audits.

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
5. The compatibility product composition has no HIL correctness, resource or
   performance qualification yet.
6. The research pinned-SRAM batch is host-tested with the production STA TX
   owner, but no complete research radio runner or HIL target exists yet.
7. Core1 load after cutover is unknown. Moving work away from Core0 is not an
   accepted optimization without total-CPU evidence.
8. `DatapathTxConsumer` currently calls the owned source through a trait object;
   its cost must be measured before deciding whether Track B needs a generic
   static source type.
9. HIL module/document cleanup beyond removed TX selectors remains a separate
   audit item.
10. The one-time `embassy_net::new` path still materializes Xarxa's complete
    stack value before writing it into static resources. Its 18 KiB HIL
    composition frame is explicitly bounded today; an in-place constructor is
    the correct future fork API if initialization stack use must be reduced.
11. The standalone station example source composes with both network leaves,
    but its ordinary ESP-HAL linker profile does not yet implement the HIL
    product's initialized-PSRAM placement contract for the high-throughput
    resource graph. Source cross-checking is not a flashable-image claim.

## Research datapath foundation

The research path now has an independent code foundation, but no hardware
composition or performance claim. `open-esp-radio-wifi-datapath` owns the
stack/executor-neutral radio key, demand, admission and materialization
contracts. The previous complete-frame materializer contract has moved out of
the Embassy adapter. STA physical consumption now also accepts already-built
SRAM owners directly; existing product BA/retry/teardown tests still pass.

`open-esp-radio-research-datapath` implements a synchronous, allocation-free
Ethernet/ARP/IPv4/ICMP/UDP engine. It stores canonical UDP/control work in a
bounded general-memory queue, publishes durable per-radio-key demand and emits
the final frame only after selection through `ReservedTxBatch`. Host tests
cover direct UDP construction and checksum validation, synchronous UDP RX,
ARP reply admission and ICMP echo replies. The crate has no Embassy, Xarxa,
PAC or executor dependency.

The research crate binds a whole reserved batch transactionally to the real
pinned-SRAM ownership primitive. Its direct physical-source implementation is
covered by partial-build/drop tests and by a STA partial-BA/retry test. The
remaining composition gate is a fused Core0 runner with synchronous radio
service and external executor waits. Ordinary/STA TX service is now synchronous;
AP aggregate service and runner composition remain. Until those exist and HIL
runs, this is architectural code, not evidence of lower CPU use.

Current UDP enqueue copies caller payload into inline canonical work storage;
final emission then copies that payload into SRAM. There is no intermediate
complete Ethernet frame, but application-to-radio one-copy is not implemented.

## Next decision gates

The immediate code gate is extracting synchronous radio service and composing
the fused research runner. This does not require completing product fairness
first. Correctness/resource HIL qualification of the owned one-copy path and
the research composition follows; micro-optimization is not the current gate.

Only after clean single-peer and multi-peer measurements may the project decide
whether to:

- retain CPU promotion or revisit GDMA overlap;
- add a generic heterogeneous packet-owner API;
- add a direct-SRAM optimized API;
- move a research result into the production core;
- optimize Core1 or promote a research fused owner into a product integration.
