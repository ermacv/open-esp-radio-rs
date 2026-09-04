# Wi-Fi network integration architecture

Status: accepted target architecture. This document defines boundaries and
decisions. Requirements are in
[`WIFI_FAIRNESS_REQUIREMENTS.md`](WIFI_FAIRNESS_REQUIREMENTS.md), current facts
in [`WIFI_EGRESS_STATUS.md`](WIFI_EGRESS_STATUS.md), and ordered work in
[`WIFI_EGRESS_CUTOVER_PLAN.md`](WIFI_EGRESS_CUTOVER_PLAN.md).

## Decision

The repository provides three independent network integrations around one
radio-native core:

1. a compatibility adapter for unmodified released Embassy/Xarxa crates;
2. an optimized owned-packet adapter with a small, generic library patch;
3. a research datapath designed from hardware constraints without upstream API
   compatibility.

They are separate composition boundaries, not runtime modes of one universal
driver. Compatibility must not constrain the radio core or the research path.
Successful research ideas move into the shared core or become separately
justified generic API changes.

```text
                         radio-native core
       VIF/peer lifecycle, BA, retries, rate, fairness policy
          physical scheduling, SRAM credits, DMA, completion
                                  |
                private selected-burst/materialization boundary
                 /                |                 \
        compatibility          owned             research
         token/copy          PacketBuf          specialized
        upstream-clean     minimal forks       hardware oracle
```

## Shared radio core

The shared core owns all facts whose truth comes from the radio:

- VIF and association-generation lifecycle;
- peer, group, power-save and traffic-class eligibility;
- BA sequence spaces, aggregation, retries and terminal acknowledgements;
- rate selection and estimated/actual airtime;
- hierarchical VIF/peer/class fairness;
- the global bounded internal-SRAM execution pool;
- Wi-Fi descriptors, DMA publication and terminal credit return;
- current and prepared radio work;
- correctness counters and HIL observations.

Network integrations must not own or mirror these facts. In particular,
Embassy and Xarxa APIs do not expose Wi-Fi peer slots, BA state, physical SRAM
credits, airtime units or a copy of the radio scheduler.

The radio may internally use a selected-burst protocol, but it is a private
open-radio interface:

```text
software queues report durable work
        -> radio selects VIF/peer-generation/TID
        -> selected owners are materialized into SRAM
        -> Core0 publishes descriptors
        -> terminal BA/retry result returns physical credits
```

The protocol is synchronous within an owner turn and batch-oriented across a
core boundary. It is never a per-packet request/reply RPC.

The code-level boundary separates complete-frame integration from physical
radio consumption:

- `SoftwareTxFrame`: affine software ownership plus interface and Ethernet
  view;
- `MaterializedTxFrame`: stable DMA ownership plus exact Ethernet geometry,
  maximum Ethernet length and guaranteed storage capacity for admission;
- `PhysicalTxSource`: synchronous transfer of final physical owners, whether
  already constructed in SRAM or materialized on demand;
- `SelectedBurstMaterializer`: queue observation, owner removal and
  reserve-before-remove single/batch materialization.

Those existing complete-frame contracts now live in the independent
`open-esp-radio-wifi-datapath` crate rather than the ESP32-S31 Embassy adapter.
The same crate also defines the research-side `EgressWorkProvider` and
`ReservedTxBatch` boundary: a deferred transport owner reports durable
`EgressFlowKey` demand and constructs only a radio-selected prefix directly in
an already reserved physical batch. `AdmissionClass` is orthogonal to the
radio peer/TID key, so bounded link-control service does not corrupt airtime or
aggregation identity.

The STA TX owner itself is parameterized by `MaterializedTxFrame`, not a
concrete Embassy pinned lease. Its aggregate builder consumes
`PhysicalTxSource`; the complete-frame materializer supplies that interface
automatically. Research reserved batches supply it directly without software
Ethernet objects or a compatibility materialization copy. The STA runtime is
still housed in the Embassy adapter. Ordinary TX (shared by STA/AP) and STA
aggregate `service` are synchronous; timeout-abort is retained state with an
explicit next deadline. Executor adapters wait outside those transitions.
AP aggregate service and the complete runner's IRQ/time integration still
need the corresponding extraction.

STA, AP and STA+AP policy are generic over those owners. Adapter mutexes,
queue dimensions, Xarxa packet types and Embassy runner lifetimes are not part
of the radio-service traits. This is the seam used by all three integrations;
it is not a public Xarxa/Embassy scheduling protocol.

The shared value vocabulary lives in `open-esp-radio-network`. It contains
only logical interface identity, link state and frame/admission errors. It has
no driver trait, executor, queue or packet allocator. The radio adapter builds
with `--no-default-features` without Xarxa, either Embassy network driver, or
the optimized owned adapter in its normal dependency graph. `DatapathNetwork`
uses associated ownership types only; external resource lifetimes, mutexes and
queue geometry remain properties of a concrete integration.

## Memory and ownership model

Long-lived backlog and physical execution storage are different resources.

```text
general memory / PSRAM                 internal SRAM
----------------------                 -------------
owned network packets                  current aggregate
transport/application backlog   --->   next prepared aggregate
sleeping-peer backlog        one copy  retries and control reserve
bounded software queues                DMA-visible fixed slots
```

The SRAM pool size depends on radio pipeline depth, not associated peer count.
No peer owns a permanent BA-window-sized SRAM reservation.

Every transfer obeys these rules:

- failure returns the original owner unchanged;
- selection happens before scarce SRAM admission;
- batch admission reserves all required SRAM slots before removing any source
  owner from its queue;
- packet construction completion is not radio completion;
- a physical slot returns only after the terminal hardware path releases it;
- descriptor ownership never crosses an executor or core boundary.

`SoftwareTxFrame` may borrow a statically rooted pool lease. The owner must
outlive the synchronous radio operation which holds it, but the trait itself
does not impose a blanket `'static` bound. The common core can therefore accept
affine leases without erasing ownership or copying a frame solely to satisfy a
type restriction.

## Integration A: upstream compatibility

This adapter compiles against the released `embassy-net-driver` contract and
requires no patch to Embassy or Xarxa.

TX is necessarily push-oriented:

```text
upstream TxToken
    -> stack builds a complete Ethernet frame
    -> adapter owns bounded staging
    -> classify VIF/peer/TID
    -> radio-owned queues and scheduling
    -> copy selected work into SRAM
```

Because the token is issued before the destination is known, it must not claim
a scarce final DMA slot. Fairness can still be implemented after the completed
frame reaches the driver, but per-key upstream backpressure and direct final
SRAM construction are unavailable.

The compatibility adapter is a maintained product option with its own resource
and performance envelope. It is not the baseline for the optimized hot path.

The released-driver endpoint and the ESP32-S31 radio bridge are deliberately
separate crates. The first knows only the official Embassy driver API and owns
bounded complete Ethernet frames. Its payload slots live in separately placed
general-memory arenas; hot channels transfer only unique mutable leases, so a
queue operation cannot copy a 1600-byte frame value or force payload and
waker/epoch metadata into one memory tier. The second narrows its radio-side
capabilities into the same `DatapathNetwork` and `SelectedBurstMaterializer`
contracts used by the optimized integration. Therefore STA, AP and concurrent
role policy is shared, while the extra compatibility copy and queue storage
remain visible in only the compatibility composition.

Acceptance requires building against pinned official releases, deterministic
bounded backpressure, UDP/TCP/raw/control correctness and the same radio-policy
semantics as other integrations.

## Integration B: optimized owned packets

This is the current product path. The only library-level semantic extension is
owned packet transfer:

```text
receive() -> Option<PacketBuf>
transmit(PacketBuf) -> Result<(), PacketBuf>
```

The patch also needs correct level-backed wake registration and bounded stack
polling. It must remain generic: no peer, BA, airtime, SoftAP cardinality,
grant, shadow or authoritative scheduling concepts belong in Xarxa or Embassy.

TX is:

```text
Xarxa builds one owned general-memory PacketBuf
    -> adapter transfers its owner to Core0
    -> driver classifies and queues by radio flow
    -> radio selects a contiguous burst
    -> selected owners are copied once into reserved SRAM slots
    -> Wi-Fi encoding/DMA owns those slots until terminal completion
```

All stack traffic crosses the same owned boundary. UDP-specific destination
indexes and a second PSRAM complete-frame tier are forbidden. TCP and raw need
qualification, not a parallel bypass API.

RX currently copies into a general-memory `PacketBuf`. Heterogeneous buffer
adoption is a separate possible generic extension and must not be coupled to
TX scheduling. It is accepted only if ownership, pool provenance, `Drop`,
exhaustion and measured value are all demonstrated.

## Integration C: hardware research datapath

The research integration is an engineering oracle, not a compatibility layer.
It reuses production PAC/DMA/MAC/CCMP/BA/rate code while allowing a specialized
synchronous packet engine.

Permitted experiments include:

- fused one-core and batched split-core execution;
- radio-pull selected-burst materialization;
- affine batches of final SRAM leases;
- transport-owned handles rather than ready Ethernet packets;
- direct final-SRAM packet construction;
- CPU copy versus GDMA copy and overlap;
- heterogeneous pools and bounded PSRAM spill.

The first oracle implements Ethernet, ARP, IPv4, ICMP and UDP. TCP is added only
after the radio/DMA/copy ceiling is established; otherwise TCP complexity would
hide the hardware cost being measured.

The protocol engine is allocation-free and synchronous. Its general-memory
queue owns canonical UDP/control work rather than complete Ethernet frames;
the final Ethernet/IP/L4 representation is emitted through a
`ReservedTxBatch`. This domain code contains no executor, PAC, Xarxa or Embassy
dependency. Fused and split-core composition are transports around the same
state machine, not separate network implementations.

The current research enqueue API copies caller bytes into its inline canonical
work storage, then packet emission copies those payload bytes into final SRAM.
Thus it removes complete-frame staging, but does not yet provide an
application-to-radio one-copy or zero-copy API. Independently owned payload
handles and caller-filled storage remain research alternatives, not claims
about the current implementation.

The same synchronous state machines must run in fused and split modes. Only the
transport boundary changes, giving a direct measurement of cross-core cost.

## Queue and scheduler topology

The radio scheduling key is equivalent to:

```text
VIF + peer generation/group + traffic class/TID
```

It is derived inside open-radio from the completed frame or specialized work
metadata. Ethernet destination alone is not a universal radio peer: station
traffic for many destinations still reaches one BSSID.

The target scheduler is work-conserving and hierarchical:

```text
physical radio
    -> weighted VIF DRR
    -> peer airtime DRR
    -> class/TID latency selection
    -> bounded pending-airtime/SRAM admission
```

Sparse traffic is sent promptly and never waits for BA32. A saturated selected
flow may fill the available BA/airtime/physical horizon. Actual completion and
retry airtime correct estimates.

## Core and wake model

Core0 is the physical radio owner. The optimized integration currently runs the
network stack on Core1. This placement is not part of packet semantics and may
later be fused.

Cross-core queues are bounded, ownership-transferring and preferably SPSC.
Wakeups are hints backed by durable level state:

```text
producer publishes state -> wake
consumer checks -> arms waiter -> rechecks -> may sleep
```

Software budget exhaustion self-wakes. Real resource exhaustion waits for the
specific credit-return edge. RX readiness, general packet-pool availability and
Core0 SRAM completion are distinct wake domains.

Timeout abort is a two-turn operation: request abort, retain DMA owners until
the hardware settle deadline, then detach and release (or quarantine on failed
detach). The interval starts after the abort request. Repeated/early wakes and
late completion cannot bypass it. Cancellation of a polling wait must leave
the transaction in its owner rather than dropping state stored in the future.

## Rejected designs

The following are not part of the target architecture:

- public Xarxa/Embassy demand, candidate, grant or authoritative modes;
- per-packet cross-core scheduling round trips;
- a UDP `[destination; 16]` index justified by SoftAP peer count;
- stack-visible radio airtime reservations;
- one physical or BA-sized SRAM queue per peer;
- PSRAM direct Wi-Fi DMA without hardware evidence;
- PSRAM blamed for performance without an A/B measurement;
- Core0 reduction counted as success when equal or greater work moves to Core1;
- a feature matrix that compiles several obsolete datapaths into one product
  binary.

## Evidence gates

Each integration is measured against the same radio revision, channel, PHY,
BA policy, SRAM ceiling and scenario. Reports record source/dependency commits,
dirty patches, exact artifacts and host topology.

The required metrics are throughput, Core0 cycles/utilization, Core1
cycles/utilization, total cycles per delivered unit, SRAM/PSRAM use, copy cost,
aggregate geometry, inter-aggregate gaps, retries and correctness counters.

No pre-cutover throughput or CPU number is automatically a property of the new
owned path. Hardware claims resume only after clean HIL qualification.
