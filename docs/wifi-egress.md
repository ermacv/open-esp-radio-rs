# Wi-Fi network integration

This reference describes the implemented network/radio boundary. The radio
owns peer and physical execution state; network adapters own their packet
storage and stack-facing contract. Source support limits are listed in the
[ESP32-S31 IEEE 802.11 feature reference](../driver/chips/esp32s31/ieee80211/FEATURES.md).
Hardware readiness is decided by [qualification](../qualification/README.md).

## Compositions and owners

| Component | Owns | Does not own |
| --- | --- | --- |
| `driver/network/interface` | Logical interfaces, link state and frame/admission errors | Queues, allocator, executor or hardware |
| `driver/network/adapters/embassy/compat` | Released `embassy-net-driver` tokens and bounded complete-frame staging | Radio peer/BA state or final SRAM slots |
| `driver/network/adapters/embassy/owned` | Owned `PacketBuf` handoff and stack wake registration | Radio scheduling or DMA descriptors |
| `driver/ieee80211/datapath` | Software/physical ownership traits and selected-burst contracts | Concrete allocator, stack or executor |
| `driver/runtime/embassy/esp32s31/ieee80211` | Physical radio runner, SRAM promotion, completion and executor waits | Application sockets or a second network stack |
| `driver/network/research` | Synchronous bounded protocol engine and selected-work construction | Production network integration or hardware qualification |

The ESP32-S31 product selects exactly one of `owned-network` and
`compat-network` at compile time. The default owned path uses the pinned
network forks; compatibility uses the released Embassy network contract.
Both compose the same physical radio runner and fixed SRAM TX horizon.
Exact dependency revisions belong in Cargo manifests and lockfiles.

The radio owns VIF/peer-generation state, power-save eligibility, rate and
retry state, BA sessions, DMA descriptors, physical credits and terminal TX
receipts. The application retains board startup, credentials, network stack,
DHCP and sockets. Stack APIs do not expose radio peer slots or airtime grants.

## Owned TX path

```text
Core1 network stack
    constructs a general-memory PacketBuf
    transfers its unique owner through a bounded queue
                         |
Core0 radio runner       v
    classifies VIF / peer generation / TID
    selects a flow and burst
    reserves the complete SRAM batch
    removes selected software owners
    copies each selected frame once into fixed DMA-visible storage
    encodes and publishes radio work
    returns SRAM credits after terminal completion
```

General-memory backlog and scarce SRAM execution credits are separate
resources. Selection occurs before SRAM admission. Failed owned submission
returns the original `PacketBuf`; a failed batch reservation removes no source
prefix. Association generation remains part of queued-work identity, so reuse
of the same MAC address does not authorize stale work.

The implemented scheduler is bounded and work-conserving. It preserves
per-flow FIFO order and round-robins non-empty flows. It does not implement
hierarchical airtime DRR or AQL. Full aggregates, low error counters or source
support for multiple peers do not establish a measured fairness guarantee.

## Physical storage contracts

- `SoftwareTxFrame` carries an affine software owner and Ethernet view.
- `MaterializedTxFrame` carries stable DMA ownership, Ethernet geometry and
  conservative capacity bounds.
- `PhysicalTxSource` transfers final physical owners synchronously.
- `SelectedBurstMaterializer` observes queued work and implements
  reserve-before-remove single/batch promotion.
- `EgressWorkProvider` and `ReservedTxBatch` let deferred research work emit a
  selected prefix directly into an already reserved physical batch.

The STA aggregate builder consumes `PhysicalTxSource`; retained frames,
retries and teardown use the concrete physical owner. A software frame may
hold a statically rooted pool lease without the trait imposing a blanket
`'static` bound. Concrete ownership carries the necessary storage lifetime.
Descriptor ownership stays with the radio executor/core.

Ordinary TX and STA/AP aggregate `service` transitions are synchronous.
Timeout abort retains the transaction and an actionable settle deadline.
Storage is released only after hardware detach, or quarantined if detach
fails. Early/repeated wakes and late completion cannot bypass the settle
interval. Cancelling the polling wait does not discard the retained owner.

Embassy waits outside these transitions. The complete runner's IRQ/time
binding remains executor-specific. Event priority also remains role-specific:
AP checks timeout/collision before completion; STA can prefer an observed
completion. A shared service signature does not make those policies identical.

## Compatibility and RX

A released `TxToken` is issued before its destination is known. Compatibility
therefore owns bounded complete-frame staging, classifies the finished frame
and copies selected work into final SRAM. Payload arenas are separate from
hot channel metadata; channels transfer unique mutable leases. Consumption,
stale-link rejection, unused-token drop, materialization and callback unwind
return a slot to its originating pool.

The owned RX path uses `OwnedRxPublisher`: Core0 protocol processing copies
into a general-memory packet and transfers it to the stack's RX pool. DMA
buffer adoption and zero-copy RX are not provided by this contract.

Cross-core wakes are hints backed by durable state. Consumers check, arm and
recheck before sleeping. Exhausting a software work budget self-wakes; actual
resource exhaustion waits for its credit-return edge. RX availability, general
packet-pool availability and Core0-local SRAM completion are distinct domains.
SRAM completion does not govern Core1 packet-pool admission.

## Research boundary and limits

The research engine implements Ethernet, ARP, IPv4, ICMP and UDP with bounded
canonical work storage. Its domain code is allocation-free and synchronous,
without PAC, executor or network-stack dependencies. Enqueue copies caller
payload into canonical storage; emission copies it into final SRAM. It is not
an application-to-radio zero-copy path. TCP and a complete fused hardware
runner are not implemented there.

Research physical batches use the production DMA ownership primitives and
STA frame interface. That shared interface does not make the research engine
a selectable production network adapter or establish its on-air performance.

Check dependency and compilation boundaries with `cargo xtask check network`.
Host tests cover ownership, admission and wake behavior. Hardware comparisons
must name the exact firmware, role, PHY/channel, traffic shape, topology and
instrumentation; resource/correctness budgets belong to the selected HIL
scenario, and readiness belongs to qualification.
