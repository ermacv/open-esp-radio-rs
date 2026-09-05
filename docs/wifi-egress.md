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
`compat-network` at compile time. Both compose the same physical radio runner
and the product's finite SRAM TX horizon. The research engine is a separate
library, not a third selectable product feature.

The radio owns VIF/peer-generation state, power-save eligibility, rate and
retry state, BA sessions, DMA descriptors, physical credits and terminal TX
receipts. The application retains board startup, credentials, network stack,
DHCP and sockets. Stack APIs do not expose radio peer slots or airtime grants.

## Network dependency contracts

| Integration | Stack-facing contract | Dependency guarantee |
| --- | --- | --- |
| `compat-network` | Released `embassy-net-driver` 0.2.0 RX/TX tokens; `embassy-net` 0.9.1 uses smoltcp | The selected production network graph uses unmodified registry packages |
| `owned-network` (default) | Unique Xarxa `PacketBuf` owners, explicit packet allocators and bounded stack polling | Pinned Embassy/Xarxa Git sources with a maintained patchset |
| Research library | Bounded synchronous IPv4 work and physical batch emission | No Embassy or Xarxa dependency |

The owned integration uses an upstream Git API baseline with maintained
changes for explicit packet pools, credit-return wakes, bounded polling and
construction of protocol state in resource storage. Its driver and socket
APIs differ from the released smoltcp-based Embassy API. Maintaining a small
patchset against that Git baseline does not imply drop-in compatibility with
the registry release. Package name and version alone do not identify either
contract: Cargo source identity and the pinned revision also matter.

The [product manifest](../driver/integration/esp32s31/embassy/ieee80211/Cargo.toml)
and its lockfile define the exact selected versions and revisions. Its
lockfile includes both optional network alternatives; an inactive dependency
can still participate in Cargo resolution. The released-network guarantee
concerns the reachable production graph for `compat-network`, not the absence
of fork entries in a shared lockfile. The ESP32-S31 hardware dependencies on
the pinned `esp-hal` and `esp-pacs` forks remain in both product profiles.

Released Embassy and Git Embassy documentation describe different interfaces.
Use the [released driver reference](https://docs.embassy.dev/embassy-net-driver/0.2.0/default/trait.Driver.html)
and [released UDP reference](https://docs.embassy.dev/embassy-net/0.9.1/default/udp/struct.UdpSocket.html)
for compat. The owned adapter must match the sources pinned by its manifest,
rather than a moving upstream `main` API.

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
- `SelectedTxSource` constructs one deferred frame per physical take, keeping
  unrequested work with its provider and returning unused reservations when
  the selection ends.
- `TxRequestSource` admits a selected request without requiring Ethernet bytes.
  The STA `start_request` entry returns the original request when busy or unable
  to materialize it. Packet-backed sources implement this through the existing
  materializer; the shared scheduler still selects complete software frames.

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

The adapter reserves a TX slot before issuing the RX/TX token pair, so the
stack can construct a reply without a fallible allocation inside
`TxToken::consume`. An unused TX token returns that reservation. The adapter
also limits each continuous ingress drain to its queue depth: a synthetic
`None` and self-wake let socket consumers run before another drain. This is
local scheduling behavior; the external token API and released stack remain
unchanged.

Released UDP `send_to_with` and `recv_from_with` already allow an application
to construct or consume a datagram in socket storage without an additional
application-buffer copy. They do not transfer a packet owner to the caller or
remove copies between socket, adapter and physical storage. Owned UDP receive
can transfer the stack's packet owner to the application; slice-based socket
APIs still copy. Socket-level ownership alone does not establish an end-to-end
DMA zero-copy path.

The owned RX path uses `OwnedRxPublisher`: Core0 protocol processing copies
into a packet allocated from the endpoint's RX pool, then transfers its owner
through the stack-facing queue. DMA buffer adoption and zero-copy RX are not
provided by this contract. Its staged RX admission is `AwaitCapacity`: the
protocol retains the original DMA staging owner until the output queue and
packet pool have capacity. `Immediate` is reserved for sinks whose retained
staging owner is itself sufficient publication credit; diagnostics cannot
upgrade an independently bounded output pool to that capability.

Cross-core wakes are hints backed by durable state. Consumers check, arm and
recheck before sleeping. Exhausting a software work budget self-wakes; actual
resource exhaustion waits for its credit-return edge. RX availability, general
packet-pool availability and Core0-local SRAM completion are distinct domains.
SRAM completion does not govern Core1 packet-pool admission.

## Research boundary and limits

The research engine implements resolved-route IPv4 UDP transmission,
synchronous UDP reception, ARP requests/replies and ICMP echo replies with
bounded canonical work storage. Its domain code is allocation-free and
synchronous, without PAC, executor or network-stack dependencies. The
[research component reference](../driver/network/research/README.md) defines
its payload ownership APIs and copying boundaries. `receive_parts` accepts
decoded Ethernet addresses, EtherType and borrowed payload without assembling
an Ethernet frame. UDP callbacks borrow the caller's receive storage for the
synchronous call; queued ARP/ICMP replies own independent work. EAPOL remains
with the radio security owner. ARP caching and unresolved datagram retention,
fragments, IPv6, DHCP and TCP are not implemented there.

For deferred TX, `SelectedTxSource` connects bounded network work to physical
takes after the radio reserves a batch. It consumes only the requested prefix
under the selected frame and byte budgets. Construction reports and physical
credit return are separate from transmission receipts. Selection matches the
complete flow identity, but current epoch and peer eligibility validation
remain the caller's radio responsibility.

Research physical batches use the production DMA ownership primitives and
STA frame interface. That shared interface does not make the research engine
a selectable production network adapter or establish its on-air performance.
There is no product supervisor connecting this engine to the fused hardware
runner, no native HIL composition and no split-core batch transport. The shared
physical interface currently exposes Ethernet geometry; it is not a general
native-MSDU or scatter-gather contract.

Check dependency and compilation boundaries with `cargo xtask check network`.
Host tests cover ownership, admission and wake behavior. Hardware comparisons
must name the exact firmware, role, PHY/channel, traffic shape, topology and
instrumentation; resource/correctness budgets belong to the selected HIL
scenario, and readiness belongs to qualification.
