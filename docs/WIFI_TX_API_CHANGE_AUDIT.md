# Wi-Fi TX network-driver API change audit

Status: route/key separation and generation-bound AP peer publication complete,
2026-08-31.

## Current implementation status

The earlier lazy-`TxToken` hook proposal below is superseded. The first
vertical refactor instead moves the scheduling boundary before final TX-token
allocation and deliberately removes the compatibility enum and its parallel
UDP dispatch implementations.

The maintained cross-repository contract is now:

```text
Xarxa resolves route + neighbour
    -> EgressRoute { link destination, traffic class }
    -> Driver::egress_key(EgressRoute)
    -> opaque EgressKey
    -> interface-wide EgressSchedule
       { non-zero max run, non-zero socket quantum, lifecycle epoch }
    -> Driver::transmit_for(EgressKey)
    -> EgressAdmission::{Granted, GlobalExhausted, KeyDeferred}
    -> final SRAM TxToken::consume()
```

Ownership remains deliberately split:

- Xarxa owns socket packet storage, per-IP intrusive queues, route/neighbour
  resolution and the interface-wide scan. It groups queues by the opaque key
  returned by the device, while retaining the original route to emit the
  Ethernet frame.
- The network driver owns route-to-egress classification, final backing and
  admission. `KeyDeferred` is a per-key scheduling decision;
  `GlobalExhausted` is physical/global pressure.
- Wi-Fi still owns VIF/peer generation, TID/AC, BA, retries, rate state and
  future airtime grants. None of those identities is accepted from Xarxa.

The removed surface is intentional: `EgressQueuePolicy`, `DestinationBurst`,
`ResolvedEgressBurst`, zero-valued quanta, `EgressMeta`, `KeyedTxToken`, and the
two old UDP dispatch entry points no longer exist. Ordinary drivers return no
schedule and retain FIFO behavior through the single production dispatch
path. Scheduling-aware configurations must construct non-zero quanta.

The cutover is implemented and published at:

- Xarxa `56840265711547e3ad8e24d602fc7a6e89258642`;
- Embassy `3b91c62054669719af1cfc97110ae6ddb5498272`;
- the Wi-Fi branch pins both revisions atomically in every standalone
  lockfile.

One concrete defect was found during the refactor. Xarxa's tracer, pcap,
fault-injection and fuzz wrappers forwarded keyed token requests but did not
forward the device scheduling configuration. Wrapping a device could
therefore silently change it back to FIFO. All four wrappers now forward the
schedule and route classifier explicitly. Focused tests cover classifier
forwarding. The missing schedule is a plausible cause of prior
trace/non-trace code path differences; it is not yet claimed as the cause of
any historical HIL throughput delta without a same-image measurement.

At this checkpoint the ESP32-S31 pinned adapter has an explicit immutable
`NetworkEndpointConfig`. Infrastructure STA uses `SingleRadioPeer`, so
distinct Ethernet destinations behind the same BSSID share one queue key.
SoftAP uses `AssociatedPeer`: Core0 publishes the current authorized peer
slot, MAC address and non-zero association epoch into a fixed 15-entry atomic
directory. Core1 resolves that snapshot only at Xarxa's burst/key boundary.
Known peers use slot+association epoch; unknown and group destinations retain
their complete Ethernet identity. The link lifecycle epoch,
peer-directory revision and network-interface ID jointly advance the local
scheduling epoch, so reconnect, association and removal invalidate Xarxa's
burst cursor. The route itself is not changed and is still used to construct
the correct Ethernet frame.

The peer directory is not a cross-core lock. Its complete snapshot uses an
atomic publication sequence; a Core1 read concurrent with mutation returns
unknown rather than waiting. Re-publishing an identity-equivalent status does
no writes, so buffered-frame counters and other unrelated AP status changes
do not bounce the directory. The table is cleared at the terminal AP service
edge.

This endpoint topology is queue geometry, not radio authorization. The
published association epoch prevents unrelated peer generations from sharing
a queue identity, but it does not make queued payload valid for a later
association. Final peer validation, backlog purge, TID/AC grants and airtime
debt still belong to the Core0 radio owner. The adapter requests a BA32-sized
keyed run and a four-packet socket quantum while the link is up, but its
current `transmit_for` still grants from the same global direct-SRAM pool and
does not yet return `KeyDeferred`. Therefore this refactor fixes the
classification boundary but does not yet implement airtime fairness, stale
backlog revocation or PSRAM spill.

No payload-memory architecture changed in this step. Frames are still built
directly in SRAM slots and transferred to Core0 without a complete-frame
copy. The rejected serial PSRAM promotion, GDMA single-frame path and owned
packet API remain rejected until new measurements justify them.

Validated so far:

- Xarxa: 698 production-feature tests plus the default workspace suite and a
  focused driver-key encoding test;
- Embassy: driver and adapter suites with scheduling enabled and disabled;
- main repository: workspace `check`, `test`, `clippy`, `fmt`, focused adapter
  suites with scheduling enabled and disabled, and the complete source-only
  audit (including the HIL performance-image and Blobray target audits);
- standalone manifests resolve with the updated revisions and lockfiles.

Hardware throughput and both-core residence are deliberately still pending.
API cleanup is not performance evidence. The next production step is a
same-ELF shadow admission experiment which records whether a Core0-owned
peer/TID grant would accept or defer each resolved key without changing the
current SRAM path. Only after owner conservation, sparse-peer latency and
single-peer throughput remain unchanged should `KeyDeferred` become active.

## Historical design audit (superseded where it conflicts above)

The remainder records alternatives and measurements considered before the
breaking cutover. It is retained as engineering evidence, not as the current
implementation plan.

## Decision summary

Do not replace `embassy-net-driver::Driver` with a new owned-packet interface.
The current direct path already has the useful ownership property: Xarxa emits
the complete frame into a uniquely owned internal-SRAM slot, and that owner is
transferred from Core1 to Core0 and then Wi-Fi DMA without a post-stack payload
copy.

The missing primitive is earlier egress classification. Today
`Driver::transmit` must reserve a concrete infallible TX token before Xarxa has
resolved the destination hardware address. Only after neighbor lookup does
Xarxa call `TxToken::set_meta`, immediately followed by `consume`. The selected
token therefore cannot distinguish a currently granted peer/TID from
contended backlog when it chooses its backing.

The lowest-cost experimental extension is now a feature-gated default
`TxToken::set_egress_meta()` hook. It must be treated as a hint for lazy backing
selection, not as packet identity or authorization. A shadow-only revision
must prove it first; it must not change backing or scheduling. This hook is not
equivalent to a mature stack's `select TXQ, then dequeue`: Xarxa has already
selected the next socket packet before the hook runs. It can support a hybrid
direct/spill policy, but it cannot by itself make Xarxa produce a burst for the
peer selected by the radio scheduler.

An initial prototype put the value in the existing non-exhaustive
`PacketMeta`. Host layout measurement rejected that representation: with
Xarxa default features disabled, `PacketMeta` grew from 0 to 10 bytes,
`UdpMetadata` from 12 to 22 bytes and each UDP packet-ring metadata element
from 24 to 32 bytes on the host ABI. The HIL queues contain two 64-entry data
rings. The absolute memory increase would be bounded, but copying and storing
an egress-only transient in every socket queue is the wrong lifetime and
working set. The hook keeps socket metadata unchanged and moves the maintenance
burden to explicit middleware forwarding, which is independently testable.

## Established constraints

1. Direct internal-SRAM ownership is already zero-copy after network-frame
   construction. Replacing its owner type cannot make it faster by removing a
   copy which does not exist.
2. The same-ELF hardware experiment in
   [Wi-Fi fairness requirements](WIFI_FAIRNESS_REQUIREMENTS.md) proves that the
   current S31 Wi-Fi TX DMA path cannot consume a cached PSRAM address. A frame
   whose canonical complete representation is in PSRAM therefore needs one
   physical PSRAM-to-SRAM materialization before radio publication.
3. The current one-copy candidate performs that copy on Core0 and reaches
   120--121 Mbit/s with 67 DMA slots, but raises Core0 radio residence to about
   60.9 percent. Moving the exact operation to Core1 is a candidate only; it is
   not yet an optimization until both-core time and throughput are measured.
4. Xarxa socket queues own transport payload, not a transferable complete
   Ethernet packet. `TxToken::consume` constructs Ethernet, IP and transport
   headers into a driver-provided mutable slice. Changing only
   `embassy-net-driver` cannot transfer the existing UDP/TCP socket-ring owner
   as a complete radio-ready frame.

## Current call order

For ordinary socket IPv4 egress with a resolved neighbor, current Xarxa does:

```text
socket.dispatch() exposes IP representation and borrowed payload
    -> device.transmit() reserves an infallible TxToken
    -> dispatch_ip() resolves route and destination MAC
    -> tx_token.set_meta(user_packet_meta)
    -> tx_token.consume(total_len, emit Ethernet + IP + payload)
```

This order has two important consequences:

- the destination is known early enough for a lazy token to choose a backing
  in `set_meta`, but not early enough for the current eager
  `Driver::transmit` implementation;
- `consume` is infallible, so every issued logical token must already own a
  valid fallback credit even if its preferred SRAM grant disappears.

The order is not uniform across all traffic. The current IPv6 path does not
call `set_meta`; the IPv4 fragmentation path does not attach metadata to all
fragments; ARP and other direct Ethernet control replies use
`dispatch_ethernet`; and a token paired with RX is created before the incoming
frame is interpreted. These paths must retain a bounded control/fallback
backing and cannot be silently made dependent on an egress hint.

## API alternatives and impact

| Alternative | Compatibility | Architectural result | Decision |
| --- | --- | --- | --- |
| `Driver::transmit(cx, intent)` | breaks every driver and adapter; cannot naturally provide the TX token paired with an as-yet unparsed RX frame | exposes classification at allocation, but forces control/neighbor resolution into a second API | reject |
| new required owned-packet `Driver` | breaks driver, Xarxa PHY and socket/fragmentation/VLAN contracts | duplicates ownership already present on direct SRAM and still cannot DMA a PSRAM owner | reject |
| feature-gated default `TxToken::set_egress_meta()` method | source-compatible; middleware must forward explicitly | keeps socket metadata unchanged and can make a token lazy | preferred experiment, with forwarding tests |
| feature-gated `PacketMeta::egress` | source-compatible because `PacketMeta` is non-exhaustive and fields already use Cargo features | reuses existing forwarding, but stores a transient link decision in every socket metadata ring | rejected by layout audit |
| parse destination only after `consume` | no third-party API change | classification is correct but backing has already been chosen | retain only as the shadow oracle |

The egress field should carry generic link information, not Wi-Fi peer state:

```text
EgressIntent {
    destination: Ethernet / IEEE 802.15.4 / native-IP,
    traffic_class: u8,
}
```

The open-radio adapter maps destination plus its immutable interface identity
to a local `VIF + peer generation + TID/AC` key. Association generation, BA
state, keys and scheduler grants must not be put into a generic network-driver
crate.

## Third-party maintenance cost

The workspace currently pins an Embassy fork and a Xarxa fork for the
cooperative runner and measured checksum implementation. The proposed feature
does not introduce the first fork, but it broadens the carried delta and must
be judged independently.

The change spans four contracts:

1. `xarxa-driver::TxToken` defines the optional pre-emission hook and generic
   egress value.
2. Xarxa invokes it only after route/neighbor resolution and before
   `TxToken::consume`.
3. `embassy-net-driver::TxToken` mirrors the hook and value.
4. Embassy's Xarxa adapter converts and forwards it without loss.

The hook must be forwarded by Embassy's Xarxa adapter and VLAN token, and by
Xarxa's tracer, fault-injection, fuzzing and pcap middleware. Terminal drivers
may keep the default no-op. A missing forwarding implementation does not break
packet correctness but silently disables the optimization, so regression tests
and a source audit of every middleware `TxToken` implementation are mandatory.

With the feature disabled, `PacketMeta` and the token trait retain their current
compiled surface. With it enabled, the egress value exists only across the
short route-to-emission call edge; socket ring layouts do not change. Feature
unification still enables the method for every crate resolving the same driver
package, so every middleware wrapper in that graph must compile under the
feature. It is a source-compatible additive change due to the default method,
but the two git revisions and the HIL target lockfile must advance atomically;
the main repository must never point at unpublished sibling working trees.

Upstreamability is uncertain. The generic API can describe destination and
traffic class, but its immediate consumer is a Wi-Fi-specific admission policy.
The extension must therefore remain optional, documented as observational, and
useful to other queued multi-destination links before an upstream proposal is
reasonable.

## Correctness hazards

### Metadata is not authority

The network stack must overwrite the egress field after its own neighbor
lookup. Application-supplied packet metadata cannot select another associated
peer or reuse an old peer slot. The radio adapter resolves the MAC against the
current association table and binds the current peer generation.

### Grants are affine credits

A shared `current_peer` atomic is insufficient: Core1 can observe it while
Core0 changes rounds, and multiple tokens can oversubscribe one advertised
window. Core0 must issue consumable grants keyed by interface, peer generation
and TID/AC. Each successful direct allocation consumes exactly one grant and
one DMA credit. Stale or exhausted grants choose the already-reserved PSRAM
fallback.

### Infallible token reservation

`Driver::transmit` may return `Some` only after reserving enough capacity for
`consume` to succeed. A lazy token may reserve a PSRAM fallback and later
upgrade to a granted SRAM slot. The fallback is returned before direct
publication. Cancellation before or after `set_meta` must return whichever
owners are live exactly once.

The RX-paired TX token remains backed by a dedicated control reserve. It must
not wait for a data-peer grant, because ARP, ICMP replies and other ingress
responses are part of the existing receive contract.

### Ordering and generation

Changing backing must not reorder frames within one peer/TID. A direct grant
cannot overtake older staged frames with the same key unless the scheduler has
explicitly drained or incorporated that frontier. Disconnect/reassociation
invalidates unconsumed grants and queued packet keys from the old generation.

### Core crossing and cache visibility

For staged frames, Core0 should queue only metadata and a packet index. It must
not read the PSRAM payload. After selection, Core0 transfers an exact DMA-slot
owner to a Core1 materialization worker; Core1 performs the one unavoidable
copy and publishes a prepared owner back through an affine SPSC boundary.
Release/acquire ordering and the existing final DMA-publication preparation
remain part of the owner transition. Partial copy, cancellation and reset must
return both owners or quarantine the DMA owner; they cannot publish a partially
initialized slot.

## Comparison with mature stack boundaries

Linux mac80211 exposes per-station/per-TID `ieee80211_txq` objects to the
driver and keeps pending airtime separately from hardware queue ownership. Its
driver-visible scheduling boundary is after packet classification, which is
the property missing from the current raw-frame token API. This supports the
queue/grant split, not a claim that Linux's `sk_buff` lifetime can be copied
directly onto this no-allocator embedded design.

DPDK similarly passes already-owned `rte_mbuf` packet objects in bursts to a
selected TX queue and has a separate `tx_prepare` stage. That model supports
late device admission, but it assumes packet storage is DMA-addressable or can
be mapped. The S31 PSRAM result prevents adopting its zero-copy premise for the
software tier.

The public Embassy driver contract intentionally makes `consume` infallible:
capacity failure belongs at token acquisition, before packet construction.
Any lazy extension must preserve that property; returning a metadata-aware
token and failing after Xarxa emits a frame would regress both correctness and
CPU cost.

### Audit of the mature-stack analogy

The external review was checked against current upstream Linux sources. Its
description of mac80211 and mt76 is substantially correct, but its conclusion
that the proposed lazy token recreates the same scheduling boundary is too
strong.

Confirmed facts:

- mac80211 explicitly describes its intermediate queues as per-station,
  per-TID software queues whose purposes include keeping hardware queues short
  and providing fairness. Drivers which delegate scheduling call
  `ieee80211_next_txq()` and return the selected queue later.
- mt76 calls `ieee80211_next_txq()`, repeatedly dequeues from that one TXQ until
  its hardware/non-AQL boundary stops the burst, and performs one queue kick.
  For a station which cannot progress in power-save, the ordinary path does
  not immediately return the TXQ; the wake transition schedules its TXQs
  again.
- mac80211 keeps station airtime deficit and estimated pending airtime per AC.
  It also has bounded per-station/per-AC power-save queues (currently 64 frames
  per AC). These support separate software-backlog, airtime and hardware
  admission resources.
- iwlwifi DQA dynamically allocates a bounded pool of hardware queues and can
  share a queue between TIDs of the same receiver when the pool is exhausted.
  It also reserves a data queue per admitted station and may reject a new
  station when that resource is exhausted. It is therefore evidence for
  bounded/dynamic hardware admission, not evidence that no hardware resource
  ever scales with peer count.
- ath9k's `ATH_AGGR_MIN_QDEPTH = 2` is a threshold expressed in hardware
  aggregate queue depth. It does not mean that only two MPDU buffers are
  sufficient and does not determine an S31 SRAM-pool size. An aggregate at
  that depth can itself retain many MPDUs.

The non-equivalence is the order of operations:

```text
mac80211/mt76
    complete skb already classified to STA/TID TXQ
    -> airtime scheduler selects TXQ
    -> driver dequeues a burst from that selected TXQ
    -> hardware admission

current Xarxa plus egress hook
    socket polling selects the next socket packet
    -> Driver::transmit reserves an infallible logical token
    -> route and neighbor lookup resolve destination
    -> egress hook tells the token which peer happened to arrive
    -> token chooses direct SRAM or PSRAM spill
```

The hook cannot ask Xarxa for “the next packet for peer A”. If socket egress is
interleaved A/B while Core0 grants A, the B packet must spill and a later A may
use the grant. A direct A frame also cannot bypass older staged A frames. It
must either wait behind them while retaining scarce SRAM, or the older frames
must first be materialized. Under sustained multi-peer backlog this can drive
the direct-hit rate toward zero even though the classification is correct.

Consequently, the following claims remain hypotheses and need counters:

- that the direct-grant hit rate is high enough to reduce total copying;
- that 67 SRAM slots are a sufficient fixed production working set for more
  than the measured two equal-rate peers;
- that moving PSRAM-to-SRAM materialization to Core1 leaves enough Core1 budget
  (the measured two-peer staged run already used about 65 percent there);
- that traffic-class/TID scheduling is available. Current high-level Xarxa
  IPv4 and IPv6 representations emit traffic class zero, so the prototype can
  classify best-effort traffic by peer but does not yet supply a real TID.

The hook remains useful because it can measure these facts and implement an
opportunistic single-peer/empty-backlog fast path. It is not the final queue
architecture.

Primary source details used for this audit:

- [mac80211 software TX queue contract](https://github.com/torvalds/linux/blob/master/include/net/mac80211.h)
- [mt76 selected-TXQ burst and one-kick path](https://raw.githubusercontent.com/torvalds/linux/master/drivers/net/wireless/mediatek/mt76/tx.c)
- [mac80211 station airtime, AQL and bounded PS state](https://github.com/torvalds/linux/blob/master/net/mac80211/sta_info.h)
- [iwlwifi DQA contract](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/intel/iwlwifi/mvm/sta.h)
- [ath9k queue-depth definition and use](https://github.com/torvalds/linux/blob/master/drivers/net/wireless/ath/ath9k/ath9k.h)

## Ownership-transfer alternative

An API which transfers an existing packet owner is possible for UDP, but it is
not a small change to `embassy-net-driver::TxToken`. The current Xarxa UDP TX
storage is one byte ring plus a metadata ring. `dequeue_with()` exposes a
borrowed slice only for the duration of socket dispatch and then reclaims that
prefix. There is no independent packet owner which can be moved to another
task, retained out of order, or returned after DMA completion.

The current paths therefore perform these payload movements:

```text
direct SRAM
    application -> UDP socket ring
    Xarxa emit -> final SRAM frame                 one payload copy

current staged reference
    application -> UDP socket ring
    Xarxa emit -> complete PSRAM frame             first payload copy
    selected complete frame -> final SRAM          second payload copy
```

Moving the second copy from Core0 to Core1 changes CPU ownership but does not
remove either copy. A real owned-UDP path would instead use fixed packet
objects with enough headroom for Ethernet, IP and UDP headers:

```text
application acquires PSRAM packet owner
    -> writes UDP payload directly into its payload region
    -> transfers owner plus endpoint to Xarxa
    -> Xarxa resolves route/neighbor and prepends headers in place
    -> per-peer/TID software queue retains the same owner
    -> selected owner is copied once into final SRAM
```

For the current IPv4 UDP ceiling payload, 42 bytes of Ethernet/IPv4/UDP
headroom plus 1,472 payload bytes fit in the existing 1,600-byte staged slot.
The HIL presently allocates a 64 by 1,472-byte UDP TX ring per logical
interface (94,208 bytes) in addition to the 67 by 1,600-byte staged pool
(107,200 bytes). An owned implementation can reuse one canonical packet tier
instead of keeping both full payload tiers. Exact metadata, alignment and
allocator overhead still need a compiled target measurement.

Required API consequences are broader than the egress hook:

1. Replace or supplement Xarxa's UDP byte-ring TX storage with a fixed pool of
   independently returnable owners and an index queue. Owners must be static or
   index-based; a borrowed slice cannot cross an async/core boundary safely.
2. Add an acquire/write/commit Embassy UDP API so `send_to_with()` can construct
   directly in an owned payload region. `send_to(&[u8])` necessarily retains
   its application-to-owner copy.
3. Let Xarxa retain and later return an owner after route failure,
   fragmentation, cancellation, disconnect and scheduler admission. Peer
   generation must be attached only after validating the resolved MAC.
4. Add a driver-facing owned-packet submission contract, or a narrowly scoped
   Xarxa egress-provider trait. Merely adding an owned method to
   `embassy-net-driver` is insufficient because ownership originates in the
   socket storage above Xarxa.
5. Keep the existing token/control reserve for ARP, NDISC, ICMP replies and
   RX-paired responses. These packets are synthesized inside interface
   processing and do not originate from an owned UDP socket packet.
6. Treat TCP separately. Its byte-stream ring must retain unacknowledged bytes
   for retransmission and may segment them differently on each send; moving a
   UDP datagram owner does not solve TCP zero-copy.

This is a legitimate second A/B candidate, not the first implementation. The
first candidate remains one complete PSRAM frame plus one final SRAM copy,
with the copy moved off Core0. Once that path has same-ELF cycle accounting,
the owned-UDP variant can be compared on:

- Core0 and Core1 cycles per delivered datagram;
- number of full-payload copies and bytes copied on each core;
- PSRAM and internal-SRAM high-water marks;
- direct-grant hits, spill count and materialization count;
- order, loss, duplication, reset and disconnect owner conservation;
- single-peer, two-peer equal-rate and asymmetric-rate throughput.

The owned path is acceptable only if it removes the measured socket-to-stage
copy without shifting a comparable cost into owner bookkeeping, checksum or
cache traffic. The Linux `skb` model demonstrates the value of movable packet
objects, but it does not demonstrate this result on S31.

Primary references:

- [Embassy network-driver API](https://github.com/embassy-rs/embassy/blob/main/embassy-net-driver/src/lib.rs)
- [Linux mac80211 driver API and TX queue types](https://github.com/torvalds/linux/blob/master/include/net/mac80211.h)
- [Linux mac80211 per-TID queue state](https://github.com/torvalds/linux/blob/master/net/mac80211/ieee80211_i.h)
- [Linux station airtime/AQL state](https://github.com/torvalds/linux/blob/master/net/mac80211/sta_info.h)
- [DPDK Ethernet TX prepare contract](https://doc.dpdk.org/api/rte__ethdev_8h.html)

## ESP32-S31 AXI-GDMA materialization experiment

The S31 AXI-GDMA can copy a cached PSRAM source into internal SRAM. This is a
different engine and result from the earlier negative experiment in which the
Wi-Fi DMA itself was given a PSRAM packet address. A minimal channel-zero M2M
owner was implemented in the platform PAC boundary so the experiment does not
depend on esp-hal's generic DMA transfer construction.

The implementation was checked against the local ESP-IDF S31 sources. IDF's
low-level M2M test resets the AXI read/write masters, uses trigger ID 6 for
channel zero, starts RX before TX, publishes `size == length` on both descriptor
chains and marks EOF only on the terminal TX descriptor. Its async memcpy path
writes a cached source back before DMA and invalidates a cached destination for
the opposite direction. The S31 cache patch issues address writeback twice.

That comparison found the actual cache-coherency defect in the first Rust
implementation. `CACHE_SYNC_CTRL` has reset value `0x1` (`INVALIDATE_ENA`). A
generated-PAC `write()` starts from that reset image, so adding
`WRITEBACK_ENA` emitted `0x5`; the fields are mutually exclusive. ESP-IDF uses
a raw register write of exactly `0x4`. Changing the Rust operation to
`write_with_zero()` and placing the complete operation in internal executable
SRAM fixed the failure. Disassembly was used to verify both the `0x4` register
image and the `0x2f...` function address.

The diagnostic image then passed SRAM-to-SRAM control plus PSRAM-to-SRAM
copies of 64, 1,536, 4,032, 4,096 and 49,152 bytes. The final uncached status
was `0x600d`, and ordinary HIL startup remained healthy afterward. The
49,152-byte benchmark used 64 iterations, one fixed ELF and the same complete
next-batch preparation in both compared paths:

| Mode | cycles/batch | retired instructions/batch | Interpretation |
| --- | ---: | ---: | --- |
| CPU bulk copy only | 87,345 | 34,701 | fastest copy latency |
| blocking GDMA only | 173,047 | 36,154 | polling is strictly worse |
| CPU copy + next preparation | 1,231,007 | 919,456 | software control |
| serial GDMA + next preparation | 1,307,713 | 920,916 | no useful overlap |
| GDMA overlapped with next preparation | 1,208,535 | 897,577 | 1.8% fewer elapsed cycles and 2.4% fewer instructions than the CPU control |
| interrupt-driven GDMA only | 177,278 | 13,605 | 2.4% more latency than polling, 62.4% fewer retired instructions |

These are materialization-microbenchmark results, not radio-throughput
results. Retired instructions include executor and ISR work and are a useful
work proxy, not a direct Core0-residence percentage. The synthetic preparation
also writes a full cached PSRAM batch while GDMA reads another one, so its
small overlap result includes real external-memory contention.

The architectural consequence is narrow. AXI-GDMA is not a replacement for
the current CPU copy when the radio is waiting for one frame: even at a
32-frame batch its completion latency is about twice the bulk CPU copy. It is
a viable optional promotion engine only when the TX design has a current and
next aggregate, enough independent preparation to overlap the transfer, and
an IRQ-driven wait so Core0 does not spin. The measured bulk CPU copy is about
2,729 cycles per 1,536-byte frame; therefore copy cost alone cannot be assumed
to explain the entire previously observed direct-versus-staged Core0 gap.
Per-frame queueing, classification, cache writeback and publication still
require separate production-path counters.

No third-party network-driver API decision follows from this probe. The next
production A/B must compare CPU and AXI-GDMA promotion behind the same
per-peer/TID scheduler, fixed SRAM pool and double-buffer boundary while
measuring radio idle time, aggregate depth, both-core residence and owner
conservation. A single-frame GDMA path should remain disabled.

## Required experimental sequence

1. Add the feature-gated default hook and egress value to the two TX-token
   traits and the Xarxa-to-Embassy conversion. Forward it through every Xarxa
   and Embassy middleware token. Do not change buffer selection.
2. On the ordinary IPv4 path, populate it after neighbor resolution. Add host
   tests proving that it is visible before `consume` and equals the destination
   MAC and class parsed from the emitted frame. Record the exact behavior for
   ARP, unresolved-neighbor, fragmentation and IPv6 rather than assuming that
   every token follows the ordinary socket path.
3. Build every Embassy network-driver package and every Xarxa PHY wrapper with
   the feature both disabled and enabled. Run the main repository placement,
   stack and source-graph audits. This is the API compatibility gate.
4. Add a lazy token in the open-radio adapter. Initially feed only shadow grant
   decisions and prove that the direct/staged choice predicted from metadata
   matches post-frame classification without changing the actual backing.
5. Enable affine grant consumption and direct SRAM selection for eligible
   frames. Preserve PSRAM fallback and all control paths. Measure direct/staged
   counts, stale/exhausted grants and within-flow ordering.
6. Add the Core1 materialization SPSC for selected PSRAM backlog. Compare it in
   one ELF against Candidate A and the 67-/98-slot direct controls.

### Isolated API prototype result

The uncommitted sibling-repository prototype has not been wired into the Wi-Fi
image and does not alter buffer selection. It establishes only the API and
compatibility surface:

- Xarxa's full selected-feature suite passes 675 tests with the hook enabled;
- Embassy's adapter suite passes eight tests with the local Xarxa revision;
- feature-disabled Xarxa, `xarxa-driver`, `embassy-net-driver` and
  `embassy-net` builds pass;
- feature-enabled IPv4 fragmentation and IPv6 Ethernet configurations compile;
- the focused Xarxa test proves `egress hook -> PacketMeta -> consume` order and
  verifies that the reported MAC equals the Ethernet destination emitted into
  the frame;
- the focused Embassy test proves lossless Xarxa-to-Embassy conversion before
  consume;
- Xarxa tracer, fault injector, fuzz injector and pcap writer, plus Embassy's
  Xarxa adapter and VLAN wrapper, explicitly forward the hook.

Current source-path matrix:

| Egress path | Prototype behavior | Required policy |
| --- | --- | --- |
| resolved Ethernet IPv4/IPv6 | destination hook before emission | may use validated data classification |
| first oversized IPv4 fragment | hook is called before the first fragment is emitted | lazy token must tolerate later drop/cancellation |
| later IPv4 fragments | no hook in `dispatch_ipv4_frag()` | PSRAM/control fallback until destination is propagated |
| unresolved IPv4 neighbor | token is consumed by an ARP request without a hook | dedicated control fallback |
| IPv6 neighbor solicitation | recursively emitted as multicast IPv6 and receives the multicast destination hook | treat as control, never consume a data grant |
| direct ARP reply/request | no hook in `dispatch_ethernet()` | dedicated control fallback |
| IEEE 802.15.4/6LoWPAN | egress value exists but is not populated | unsupported by this Wi-Fi experiment |
| native IP medium | no link destination hook | unsupported by this Wi-Fi experiment |

There is also one ecosystem-level limitation: `embassy-net-driver-channel`
is an asynchronous packet-owning boundary, not a transparent token wrapper.
To preserve egress metadata through it, the channel's `PacketBuf` must store
the value and its lower `TxSlot` must expose it. That increases each enabled
channel slot and requires its own feature and tests. The current open-radio
datapath does not use that crate, so this is not a blocker for the shadow HIL,
but it is a real upstream maintenance consequence and prevents claiming that
the hook is automatically end-to-end in arbitrary Embassy compositions.

Acceptance requires at least 120 Mbit/s for the saturated equal-PHY two-peer
TX case, less than 40 percent Core0 residence, bounded Core1 residence, zero
unclassified loss/reorder/duplication, and an internal-SRAM footprint which no
longer grows by one BA window per peer. If the metadata feature changes code
layout enough to invalidate the comparison, the runner must report that fact
and use an explicit same-functionality control rather than interpreting the
throughput delta.
