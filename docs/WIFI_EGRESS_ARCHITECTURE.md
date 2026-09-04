# Wi-Fi egress architecture

Status: architecture decision record, 2026-09-04. This is the target design
for the next cutover. It supersedes the architectural conclusions in the
former append-only TXQ, API-audit and egress-checkpoint documents.

## Decision

The current token-based Xarxa/Embassy scheduling branch is an experimental
semantic oracle, not the production base.

Production development moves to the current Xarxa owned-`PacketBuf` model.
The first candidate transfers a completed packet owner to the Wi-Fi driver,
queues it by link egress domain, selects a burst inside the Wi-Fi adapter and
promotes that burst once from general/PSRAM storage into the fixed internal-
SRAM radio pool. The scheduling protocol between Core1 and Core0 remains an
implementation detail of the Wi-Fi driver.

We will not port the current public `EgressDemand`/`EgressBurstGrant` protocol
into new Xarxa or stabilize its optional methods in `embassy-net-driver` unless
the owned one-copy candidate fails an explicit resource gate and a direct-SRAM
pre-materialization path is then proved necessary.

The owned-buffer base required one deliberate extension: a `PacketBuf` must be
able to remember and return to its originating static pool. That extension is
now implemented on the owned Xarxa branch: one uniform pointer-sized owner can
originate in the general PSRAM pool or a driver RX SRAM pool and returns to the
correct pool on final drop. Pool-aware ownership was the required cross-stack
change. Radio scheduling remains private to the Wi-Fi driver.

## Why the decision changed

The scheduling work was built on Xarxa commit `1f332ac`, continued by the
project branch at `3ac0e58`. The audited owned line began at Xarxa `9d32976`,
which has no merge base with that branch, and now continues at `122e9714`.
The two lines are different architectures:

| Old line | Current `main` |
|---|---|
| borrowed `RxToken`/`TxToken` | owned `PacketBuf` |
| socket-owned packet rings | one shared packet ownership model |
| final backing chosen before `TxToken::consume` | completed buffer transferred by `Driver::transmit` |
| out-of-order UDP selection needs intrusive ring indexes | packet owner can be moved into a driver queue |
| current project adds scheduling through Xarxa and Embassy | driver can queue and schedule an accepted owner |

Completing TCP/raw coverage on the old line would therefore not prepare a
merge. It would reimplement scheduling inside an architecture that upstream
has already replaced, then require another implementation on the new line.

The original design solved a real problem: early allocation of scarce SRAM
before peer selection fragments aggregates and does not scale. Owned
`PacketBuf` changes where that problem can be solved. A completed general-
memory packet can now be handed to the driver without claiming a DMA slot, so
the per-peer queue no longer has to live in every transport socket.

## Constraints

### Physical memory

- Cached PSRAM is suitable for code, general packet ownership and bounded
  backlog.
- The reviewed Wi-Fi DMA path does not consume packet data directly through
  the cached PSRAM aperture. The same-image direct-address experiment reached
  zero delivered traffic while the SRAM control worked.
- DMA-visible TX storage remains internal SRAM.
- SRAM usage must be a function of radio pipeline depth, not associated peer
  count or software backlog.
- SRAM is uncached. A copy into it is real CPU or GDMA work, not a cache-line
  ownership operation.

### Execution

- Core0 owns the radio, DMA, BA/retry completion and physical policy.
- Core1 currently owns Xarxa and application/network work.
- The design must remain valid if the stack and driver later share one core.
- Work removed from Core0 must not simply appear as equal or greater Core1
  work.

### Protocol

- AP peer identity includes association generation.
- STA collapses multiple Ethernet destinations onto its one associated radio
  peer.
- TID/access category is part of Wi-Fi queue identity, but generic network
  code carries only priority/traffic-class intent.
- Power-save and terminal airtime facts remain link-layer state.

## Mature-stack model being adopted

Linux mac80211 keeps intermediate queues per station/TID so hardware queues
can remain short and fairness can run before hardware admission. A driver is
woken when a queue becomes active, asks for the next TXQ, and dequeues frames
from that selected queue. The packet already exists as an `skb`; mac80211 does
not pull bytes directly from a UDP or TCP socket. See the
[mac80211 TXQ contract](https://github.com/torvalds/linux/blob/master/include/net/mac80211.h)
and its [airtime scheduler](https://github.com/torvalds/linux/blob/master/net/mac80211/tx.c).

The mt76 driver then dequeues a burst from one selected TXQ until the software
queue or hardware capacity stops it, rather than selecting a new peer for each
packet. See
[`mt76_txq_send_burst`](https://github.com/openwrt/mt76/blob/master/tx.c).

The ownership rule also matches ordinary network drivers: accepting a packet
transfers ownership; rejecting it does not. See the
[Linux softnet driver contract](https://docs.kernel.org/networking/driver.html).
DPDK expresses the same idea for batches: only the accepted prefix of a TX
burst changes owner. See the
[DPDK ethdev guide](https://doc.dpdk.org/guides-25.11/prog_guide/ethdev/ethdev.html).

We adopt these topology and ownership rules, not Linux machinery. There will
be no `skb`, RCU, dynamic qdisc graph, busy polling or hardware queue reserved
per peer.

Delayed transport materialization directly into SRAM is not established by
these references. It is a possible S31 optimization and must be justified by
our own measurements.

## Target owner graph

```text
application / transport
  owns UDP input until send, TCP stream and retransmission state
                    |
                    v
Xarxa current owned-packet path
  builds one complete Ethernet PacketBuf in the general pool
  hands it to Driver::transmit(PacketBuf)
                    |
                    | successful call transfers ownership
                    v
Wi-Fi software egress (driver-owned)
  classifies VIF + peer generation/group + traffic class/TID
  queues PacketBuf owners in bounded per-key FIFO queues
                    |
                    | active-set state, not one request per packet
                    v
Core0 radio policy
  applies VIF/peer airtime DRR, PS/BA/rate eligibility and AQL horizon
  selects one queue and a bounded burst
                    |
                    v
promotion
  copies selected owners once into reserved internal-SRAM slots
  returns general PacketBuf owners after successful copy
                    |
                    v
MAC / CCMP / A-MPDU / Wi-Fi DMA
  keeps generation, key and accounting identity to terminal completion
                    |
                    v
BA / retry / drop receipt
  releases SRAM and charges actual airtime
```

The exact location of software TX queues is an implementation choice:

- Core0-owned queues allow the radio owner to select and promote without a
  return grant, at the cost of moving general `PacketBuf` owners across the
  core boundary and reading PSRAM on Core0.
- Core1-owned queues allow Core1 to perform promotion, but require a private
  queue-level active/grant protocol so Core0 remains the policy owner.

Both are valid under this architecture. The first prototype should choose the
smaller correct owner graph, instrument both cores, then retain the placement
that minimizes total work while meeting the Core0 gate. The protocol must be
per activation/burst, never a request/reply for every packet.

## Packet buffer ownership

### Required abstraction

`PacketBuf` remains the uniform packet type seen by Xarxa. It must carry enough
private origin information for `Drop` to return storage to the pool that
created it.

Required pool properties:

- caller-provided static storage; no heap requirement;
- compile-time capacity and alignment;
- safe allocation and unique ownership;
- `PacketBuf: Send` when the pool's return path is thread-safe;
- pool origin retained through header push/pull and metadata changes;
- buffer returned exactly once, from any valid owner/core;
- no arbitrary borrowed slice whose lifetime is erased unsafely;
- explicit memory class/capability visible to the driver, not to protocols.

At minimum the composition needs:

```text
General pool       PSRAM-capable, stack/backlog ownership
RX DMA pool        internal SRAM, descriptor/recycle ownership
TX execution pool  existing internal-SRAM radio slots
```

The first TX candidate consumes `General` owners and copies into the existing
TX execution pool. RX should eventually return an adopted RX-DMA owner to
Xarxa and recycle it to the radio pool on drop. Until that exists, a migration
RX copy is diagnostic scaffolding and may not be declared the final zero-copy
path.

Pool placement must be application/linker controlled. A generic crate must not
hard-code an ESP section name. A custom section or a caller-supplied static
pool may implement placement, but its initialization and `NOLOAD` behavior
must be verified on target.

### Why one global pool is insufficient

Putting the new Xarxa global pool in SRAM permits DMA but makes socket,
neighbor-resolution and multi-peer backlog compete with radio descriptors.
Putting it in PSRAM scales the backlog but forces RX and TX copies. The choice
cannot represent the required hierarchy. Increasing one homogeneous pool is
therefore not a solution.

## TX queue and admission contract

The base owned driver contract is sufficient for the first candidate:

```rust
fn can_transmit(&mut self) -> bool;
fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf>;
```

For the Wi-Fi driver, `can_transmit` means that the bounded software ingress
queue can accept one more general packet. If it returns `true`, the next
`transmit` must succeed. It does not promise that a DMA descriptor is currently
free.

On success, the driver owns the packet through queueing and promotion. On
failure, it returns the unchanged owner. Queue limits include a global bound,
per-key bounds and reserved capacity for required control work. A progressless
peer cannot consume the complete general pool.

Classification happens after ownership transfer and before insertion:

```text
adapter instance / VIF
+ current association generation
+ destination peer or group domain
+ trusted traffic class -> TID/AC
```

The final physical path revalidates lifecycle identity. Stale data is dropped
with an explicit reason and owner release; it is never retargeted to a new
association.

## Radio scheduling and completion

The radio policy uses three independent credits:

1. `QueueCredit`: bounded general-memory backlog.
2. `AirtimeCredit`: VIF/peer/class deficit and estimated pending airtime.
3. `DmaCredit`: fixed internal-SRAM execution slots.

Admission charges estimated airtime when a burst enters the physical horizon.
Terminal completion releases physical credit and reconciles actual airtime,
including retry cost where the hardware exposes it. Stack or copy completion
only says that a source owner may be released; it is not an airtime receipt.

The scheduler selects up to:

```text
min(queue depth,
    BA window availability,
    aggregate limit,
    free SRAM slots,
    remaining estimated-airtime quantum,
    AQL pending limit)
```

It may select fewer immediately. There is no timer whose purpose is to wait
for BA32. An optional very short aggregation deadline may be introduced only
with a latency/throughput measurement and must expire independently of another
enqueue.

## Cross-core and wake protocol

The present split-core composition uses bounded SPSC ownership transfer. The
source of truth is durable state, not the edge which announces it.

Typical state changes are:

- a queue becomes active or inactive;
- software queue capacity becomes available;
- SRAM/DMA credit returns;
- a terminal TX receipt arrives;
- power-save or association generation changes;
- a cooperative runner budget expires.

The required waiting pattern is:

```text
check durable pending state
arm waiter
recheck durable pending state
sleep only if still clear
```

Notifications may be coalesced. Full transport cannot silently discard the
only representation of `Active`, `Inactive`, reset or returned credit. Either
the latest level state remains visible and is retried, or publication returns
backpressure to the owner.

## Embassy integration

The first owned-packet candidate does not need public radio-scheduling methods
in `embassy-net-driver`. Embassy must provide a bounded runner around Xarxa's
ordinary owned `Driver`:

- a configurable ingress packet budget;
- a bounded TCP/control egress budget;
- fair alternation of ingress, timers and stack-generated egress;
- self-wake when only a cooperative budget stopped progress;
- sleep only after real driver or timer blocking;
- driver wake when Wi-Fi software queue capacity returns.

Runner budgets are constructor/configuration policy, not fields reported by a
radio device.

### Audit of the current prototype interface

The old-lineage prototype added these operations to the base driver trait:

```text
transmit_control
egress_key
transmit_for
transmit_granted
egress_schedule
update_egress_demand
poll_egress_grant
finish_egress_grant
```

They are feature-gated default methods. That has four architectural problems:

1. A concrete type can advertise an authoritative schedule while inheriting a
   default implementation of an operation required for progress.
2. Middleware such as a VLAN splitter or driver-channel wrapper can preserve
   the base trait while silently losing scheduling behavior.
3. The base device API mixes packet ownership, scheduling policy, rollout mode
   and executor budget.
4. Xarxa and Embassy duplicate the protocol schema and manually translate it.

The current implementation fixed its earlier missing feature guard and has
feature-off/on checks, but those tests cannot prove that an arbitrary wrapper
preserves a semantic capability. The problem is the shape of the type
contract, not a missing `cfg`.

If the optional direct-SRAM extension is ever needed, the capability must be a
required subtrait or a distinct device type accepted by a distinct runner.
VLAN/channel middleware must then either implement deliberate multi-consumer
arbitration or fail to compose. Silent fallback to plain `transmit` is not
allowed.

### Feature policy

Cargo features select code and memory that a product requires. They do not
select correctness semantics at runtime.

- The ordinary owned driver remains the default and pays no scheduling-
  protocol memory cost.
- Wi-Fi queueing/fairness is production behavior inside the Wi-Fi adapter, not
  an optional `tx-egress-metadata` mode.
- Shadow observers, FIFO controls, copy probes and identity tracing are named
  diagnostic image features and never production alternatives.
- A future public selectable-egress capability, if justified, uses a name such
  as `egress-scheduling`; it has a feature-off RAM/code-size test.
- Capacities are explicit composition parameters or checked compile-time
  constants. A feature must not encode the assumption that IP destinations,
  radio peers and active TIDs have the same cardinality.

## Xarxa capability and limitation audit

The new owned architecture provides useful guarantees:

- `Driver` is object-safe and usable in a multi-interface stack;
- received and transmitted packet ownership is explicit;
- UDP/raw queues no longer need a private MTU-sized TX ring per socket;
- a pending neighbor-resolution packet remains one movable owner;
- UDP routes and checks device capacity before invoking the payload closure;
- TCP retains canonical stream/retransmission bytes and materializes only a
  currently eligible segment;
- failed TCP device/pool admission leaves the socket state as if that segment
  had not been sent.

Its present limitations for this project are equally explicit:

- one hidden global packet pool and one global size/alignment policy;
- no external/adopted driver buffer and no origin-aware return;
- no memory-class or per-interface allocator selection;
- `can_transmit` is global, so it cannot provide per-key backpressure before a
  completed packet exists;
- `Stack::poll` drains driver RX and TCP work in loops, requiring an Embassy
  cooperative boundary under sustained load;
- current Embassy main has not yet integrated this Xarxa line.

None of these limitations requires Wi-Fi peer state in Xarxa. Pool ownership,
bounded poll and an optional generic traffic-class field are appropriate
generic changes. BA, PS, VIF fairness, AQL and radio grants are not.

### Traffic metadata

The driver can classify AP/STA destination from the completed Ethernet frame.
Priority should be preserved as generic packet metadata so the driver does not
need to reinterpret every protocol header and applications/system policy can
set it explicitly. The generic value is an intent, not a trusted request for
unlimited voice priority. Wi-Fi validates/maps it to UP/TID/AC and applies its
own admission policy.

Adding compact feature-gated traffic class to `PacketMeta` is compatible with
the owned one-copy design. It is independent of public grant scheduling.

If a later direct-SRAM A/B proves that the stack must choose backing before
packet construction, the extension must be an explicit capability and an
explicit scheduled runner. It must not be optional default methods on the base
`Driver`, because middleware can otherwise compile while silently dropping
the capability.

## Optional direct-SRAM extension gate

A public stack/link scheduling API may be designed only if the owned one-copy
candidate fails one or more agreed gates and phase measurements attribute the
failure to unavoidable materialization work.

Such an API must be smaller than the current experiment and satisfy all of:

- static capability at the type/composition boundary;
- complete bulk coverage, including TCP and raw;
- no per-socket `O(max_keys)` arrays;
- queue-level readiness and burst permission, not per-packet RPC;
- a transactional TCP prepare/commit boundary;
- typed admission/reserve classes;
- opaque egress identity and lifecycle generation;
- final synchronous, infallible admission after backing reservation;
- no Wi-Fi airtime units in the generic stack API unless the stack consumes
  them;
- wrappers such as VLAN and driver-channel either deliberately arbitrate and
  preserve the capability or fail to type-check in that composition.

The current `StackSelected`/`Shadow`/`Authoritative` runtime modes and
`tx-egress-metadata` feature are experimental rollout mechanisms, not this
future interface.

## Rejected architecture directions

- SRAM buffers or BA-window-sized reserves per peer.
- A global FIFO followed by scanning for a contiguous peer run.
- Direct Wi-Fi DMA from cached PSRAM without a new hardware proof.
- A second complete Ethernet-frame copy in a Wi-Fi-specific PSRAM queue when
  the transferred `PacketBuf` can be the canonical queued object.
- Calling UDP/TCP sockets from the driver.
- Per-packet cross-core candidate/grant round trips.
- Treating stack materialization completion as terminal airtime completion.
- Moving copy cost from Core0 to an already saturated Core1 and calling it an
  optimization.
- Stabilizing the old no-common-ancestor Xarxa branch because it has more
  implemented features.

## Permanent concepts retained from the prototype

The prototype remains valuable. These concepts survive, whether public or
private:

- opaque driver-owned egress identity;
- explicit global exhaustion versus key-specific deferral;
- generation/epoch protection against stale work;
- coalesced active-set publication;
- burst-sized opportunity;
- exact ownership transfer at final admission;
- fixed shared SRAM horizon;
- Core0-owned BA/PS/rate/AQL/terminal policy;
- separate stack/materialization and physical completion meanings;
- bounded cooperative progress.

The concrete old-lineage socket index, duplicated protocol types, optional
base-trait methods and diagnostic modes do not survive automatically.

## Open questions resolved by implementation evidence

The architecture deliberately leaves these quantitative choices open:

- Core0-owned versus Core1-owned software TXQ placement;
- CPU versus AXI-GDMA promotion;
- size of the general queue and per-key limits;
- number of current/standby aggregates in SRAM;
- exact VIF and peer airtime quanta;
- whether direct-SRAM materialization is justified;
- compact representation and release mechanism of multi-origin `PacketBuf`.

They are not correctness-policy decisions. The cutover plan defines the tests
which choose them.
