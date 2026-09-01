# Wi-Fi egress refactor checkpoint

Status: canonical architecture checkpoint, 2026-09-01. This document defines
the current implementation boundary and the next refactor stages. Older design
notes remain useful as experimental history, but do not override this status.

## Verdict

The refactor is moving in the correct direction, but the current egress
control must remain non-authoritative.

The useful architecture is already visible:

- Xarxa retains bounded software packet ownership and chooses a contiguous
  run for one opaque egress key before final backing;
- the network driver revalidates that key and emits the frame directly into
  the fixed internal-SRAM TX pool;
- Core0 is the sole owner of radio policy and hardware feedback;
- Core1 and Core0 exchange bounded demand and burst-grant values, never packet
  payloads or DMA owners.

The missing part is not another packet-copy mechanism. It is a physical-radio
wide demand catalog and an actual Core0 airtime/admission policy which covers
both STA and AP virtual interfaces. The present control plane is AP-only and
only echoes each candidate. It proves ownership, identity, bounded service and
same-ELF measurement, but it does not yet provide fairness or `KeyDeferred`
authority.

An authoritative cutover now would introduce policy before these lifecycle
and radio-wide contracts are complete. A full rewrite of Xarxa's RX path,
transport protocols or packet API would also be premature. The next change
should instead be an evolutionary interface-owned egress-demand catalog, kept
in shadow mode until its behavior and cost are measured.

## Exact checkpoint

The repository checkpoint used by the measurements below is:

```text
open-esp-radio-rs-wifi commit: deaf5d6f70ccf2d1c04438ab6ad6ec61d2414f5b
branch:                        refactor/wifi-interface-egress-scheduler
Xarxa revision:                56840265711547e3ad8e24d602fc7a6e89258642
Embassy revision:              3b91c62054669719af1cfc97110ae6ddb5498272
application SHA-256:           c0306b28e70780b4fedadb1392d02159d02f937bbc18907ea0ea75fc816592e7
runtime ELF SHA-256:           c9349ef5d6411aed946b83a81a6724b035437daf334bbf38879b8ac3b2738eb8
runtime CRC:                   1f0b00e8
workspace patch SHA-256:       e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

The Xarxa sibling worktree was clean at the pinned revision when this audit
was made. HIL reports remain the detailed evidence; this document records the
interpretation and the gates for the next change.

## Phase 1 implementation checkpoint

The first generic lifecycle contract was added after the measured checkpoint
above and is pinned by the current workspace:

```text
Xarxa revision:   bd2c27fb584445c93aec04e8560835de3dec450e
Embassy revision: c6d0de6fc1b22aea8cd20aac26a183db4c73d447
```

This checkpoint adds `EgressDemandId`, `EgressDemandLevel`, `EgressDemand` and
the ordered `EgressDemandUpdate::{Reset, Active, Inactive}` callback to Xarxa,
the Embassy driver boundary and the Xarxa-to-Embassy adapter. It deliberately
does not call the callback from production Xarxa yet, and it does not change
`transmit_for`, SRAM ownership or radio admission. The existing HIL data path
therefore remains the behavior oracle for the next integration step.

The host catalog model established one important correction to the initial
design: a demand key cannot be owned by exactly one socket queue. Multiple
UDP sockets, IP destinations or future protocol providers may resolve to the
same physical radio key. The catalog therefore gives each provider an affine
O(1) update handle, while aggregating all providers into one nonempty key
lifetime for the driver. The last provider ending emits the terminal update;
a stale provider handle cannot end or mutate a later lifetime.

The model also proves:

- sparse activation is published immediately and never waits for BA32;
- exact queue counts are coalesced through high/low hysteresis;
- a one-frame horizon remains eligible for every nonempty level;
- schedule epoch reset invalidates every provider handle;
- catalog overflow and activation before configuration fail closed;
- aggregate levels saturate only at the public boundary without corrupting
  internal reclamation;
- Xarxa middleware and the Embassy stack adapter preserve exact lifecycle
  identity, key and level values.

At the Phase 1 checkpoint the catalog remained a host model until protocol
providers could own stable handles without payload rescans. Shipping unused
catalog state in every interface was intentionally avoided at that boundary.

## Phase 2 stack checkpoint

The first production shadow publisher is now pinned by:

```text
Xarxa revision:   e903c7f4525689f5b7803d086c3b243390069f29
Embassy revision: 24f4886f75b534b91a9e58716c0dd989eceaf148
```

Xarxa now owns a bounded interface catalog and invokes the demand callback for
UDP indexed queues before synchronous dispatch. Each UDP destination queue
retains a generation-validated lookup hint beside its queue metadata. During
one interface observation, the catalog rebuilds aggregate demand for every
opaque device key from those bounded queue counters; it retains no entries per
socket or provider. Several IP queues and sockets resolving to the same key
therefore publish one coherent demand lifetime. Queue length is maintained on
enqueue/dequeue, so observation never scans packet payload. A schedule epoch
reset invalidates queue-local hints; an unobserved key (including all providers
being removed) is reclaimed and emits one terminal `Inactive`.

The implementation deliberately remains shadow-only:

- `transmit_for` and the existing interface burst arbiter are unchanged and
  remain the only admission authority;
- sparse demand is published immediately; BA32 is a high watermark, never a
  minimum-fill delay;
- a 16-entry catalog bounds distinct interface egress keys, not providers,
  sockets or packet storage. This covers fifteen SoftAP unicast peers plus one
  group domain; STA+AP combines its separate interface catalogues later at the
  physical-radio owner. Overflow omits excess shadow demand without blocking
  ordinary TX;
- TCP, raw, ICMP and generated control traffic do not yet claim provider
  coverage and continue through the existing synchronous path;
- Embassy forwards the lifecycle exactly, but the open-radio network device
  does not yet consume it. The old run/refill candidate protocol therefore
  remains isolated until Phase 3 replaces it with radio-wide demand state.

Host tests cover same-key aggregation across two independent UDP sockets,
BA32 horizon crossing, socket removal, route rekey, epoch reset, stale handle
rejection and terminal disable. This checkpoint still needs same-ELF HIL CPU
accounting before it can be called performance-neutral.

An earlier Phase 2 implementation retained 64 provider entries in every
interface and increased the HIL `start_network_endpoint` async frame to 10,400
bytes, over the 8,192-byte review threshold. The source-only image audit caught
that architectural mistake. The aggregate-only redesign removes provider-sized
state and carries a regression assertion that the 16-key catalog remains at or
below 1,024 bytes; increasing the stack threshold was explicitly rejected.

## Current data path

```text
Core1 / Xarxa

socket-owned payload in PSRAM
    -> route and neighbour resolution
    -> EgressRoute { destination, traffic class }
    -> driver-owned opaque EgressKey
    -> interface-wide burst arbitration
    -> Driver::transmit_for(key)
    -> final key/generation revalidation
    -> claim one slot from the global 67-slot SRAM pool
    -> construct Ethernet/IP/transport frame directly in that slot
    -> publish affine owner to Core0

Core0 / Wi-Fi

ready SRAM owner
    -> MAC / CCMP / BA aggregation
    -> Wi-Fi DMA
    -> TX completion / BA / retry / PHY feedback
```

There is no complete-frame PSRAM-to-SRAM copy in the accepted path. The PSRAM
storage is the transport payload backlog; Xarxa performs the ordinary final
frame construction directly into an SRAM token only after selecting a key.
The physical SRAM pool remains global and fixed rather than growing by BA
window per associated peer.

With `tx-egress-metadata`, Embassy constructs the Xarxa UDP TX buffer with
`PacketBuffer::new_indexed_slots`. Therefore selected UDP packets release
their metadata and payload slot immediately even when another key has an
older packet. The generic tombstone-capable byte-ring implementation still
exists for other configurations, but prefix-bound UDP payload reclamation is
not an open defect in this Wi-Fi production configuration.

## Current Xarxa contract

The maintained cross-repository API is:

```text
EgressRoute
    -> Device::egress_key()
    -> EgressKey
    -> Device::egress_schedule()
    -> Device::transmit_for()
    -> EgressAdmission::{Granted, GlobalExhausted, KeyDeferred}
```

The separation is intentional:

- Xarxa owns routes, neighbours, socket queues, payload lifetime and generic
  interface arbitration;
- the driver owns opaque link/radio classification and final backing;
- Wi-Fi owns VIFs, association generations, peer/TID state, BA, power-save,
  rate control, retries, airtime and radio admission.

`GlobalExhausted` means no physical backing is currently available.
`KeyDeferred` means this valid key is not currently admitted. Xarxa must not
collapse them: global exhaustion preserves the current burst, whereas a
key-specific defer permits trying another eligible key.

What Xarxa already does correctly:

- keeps a single interface-wide active burst, preventing independent sockets
  from admitting `A, B, A, B, ...` merely because socket iteration alternates;
- gives UDP a bounded 16-key intrusive index and independently removable
  indexed payload slots;
- groups different IP destinations which resolve to the same opaque device
  key;
- preserves FIFO order within one selected queue;
- resets burst state when the driver scheduling epoch changes;
- revalidates the final driver key before an SRAM claim.

What is not yet general:

- only UDP exposes removable per-key queued packets; TCP, raw and generated
  control traffic do not all expose the same selectable-head contract;
- Xarxa does not publish an explicit active/backlog/empty lifecycle for every
  key to the radio owner;
- queued payload which has not yet been classified carries no association
  generation. The project still needs an explicit policy for whether such
  traffic survives a disconnect/reassociation; the current representation
  cannot enforce revocation at enqueue lifetime;
- `EgressRoute` has a generic `traffic_class` field, but Xarxa's current
  resolved-route constructor always writes zero and the AP grant key hard-codes
  best-effort TID 0;
- queue selection is per network interface, while radio fairness must span all
  VIFs sharing the same physical radio.

The 15-peer `15 * 32 = 480` result is a host selector/model test, not physical
HIL and not a required production queue size. It proves that a fully
interleaved arena with 32 packets for each of 15 keys can be emitted as 15
contiguous BA32 runs. Its 128-packet control proves only that 128 packets
cannot simultaneously provide BA32 look-ahead for every peer. Production may
need much less backlog through backpressure, drop policy and demand-driven
service; the size must be measured from queue residence and radio starvation.

## Current cross-core control plane

The current implementation is not a per-packet request/reply protocol.

Core1 publishes an `EgressCandidate` containing an exact serial, opaque radio
grant key and a non-zero requested frame count. Core0 returns an `EgressGrant`
for that exact candidate. Core1 stores the result in a local
`EgressBurstLease` and spends credits in the synchronous SRAM-admission path.
The normal packet path neither traverses the two SPSC queues nor performs a
cross-core atomic allocation for every packet.

Current constants are:

```text
candidate SPSC depth:        16
grant SPSC depth:            16
requested burst:             32 frames
Core0 service budget:         4 candidates per turn
Core1 maintenance budget:     4 grants per call
refill threshold:             8 remaining frames
```

The finite Core0 budget is a correctness property. Draining until empty can
livelock even with bounded queues: each echoed grant can let Core1 publish a
successor before Core0 observes an empty frontier. The owner must yield after
a finite amount of work and leave a level-latched pending flag for the next
turn.

`EgressWaitOr` uses check, arm, recheck, then sleep. This closes the usual
producer-publication versus waiter-arming lost-wake window. The unique mutable
`EgressRadioOwner` travels with the connected Core0 datapath owner; the shared
network side only holds a small wake capability.

The radio implementation is nevertheless still a shadow echo:

```text
candidate(key, 32)
    -> Core0 service_shadow()
    -> grant(same key, same serial, 32)
```

It does not inspect BA availability, power-save state, rate, outstanding
airtime, VIF deficit or peer deficit. Packet admission continues even when no
grant is available, and the grant is used only for accounting. This is why the
control plane is safe to measure but is not yet a scheduler.

It is also AP-only. The AP endpoint is wrapped with egress control and maps an
associated peer to `interface + peer slot + association generation + TID`.
The STA endpoint uses `SingleRadioPeer` and currently receives no such grant
key. Consequently STA+AP radio fairness cannot be implemented by the current
owner.

## Same-ELF control cost

`enabled` and `disabled` select only the current AP candidate/grant shadow
control at startup. Both modes retain the same Xarxa indexed queue selection,
route classification, 67-slot SRAM pool, MAC, radio, firmware and ELF layout.
Disabled mode does not restore the old global FIFO and does not disable AP TX.

Clean A/B/A used the same runtime ELF at commit `deaf5d6f`:

| Mode | HIL run | Throughput | Core1 network time | Core0 radio time | Admission cost |
| --- | --- | ---: | ---: | ---: | ---: |
| enabled A | `1788273618415-00140d48` | 109.505, 109.616 Mbit/s | 91.129, 91.691 us/frame | 47.915, 47.994 us/frame | 2243.3, 2342.5 cycles/frame |
| disabled B | `1788273841198-00141114` | 111.730, 111.135 Mbit/s | 89.173, 90.139 us/frame | 46.947, 47.062 us/frame | 1871.3, 1994.8 cycles/frame |
| enabled A | `1788273958041-00141533` | 110.115, 109.738 Mbit/s | 91.368, 91.516 us/frame | 47.968, 47.911 us/frame | 2341.1, 2324.8 cycles/frame |

The enabled average was 109.744 Mbit/s and the disabled average was
111.433 Mbit/s: a 1.52% ceiling cost in this intrusive diagnostic image.
Enabled mode added approximately:

- 1.77 us/frame to the Core1 network task;
- 0.94 us/frame to the Core0 radio task;
- 380 admission cycles/frame;
- 3,854 explicit Core0 control cycles per granted burst, or about 120 cycles
  per frame when amortized over BA32.

The last value is important: cross-core policy transport is already
burst-granular. Most of the current measured overhead is still in Core1 lease
maintenance/instrumentation and in the extra owner/wake/control path, not in a
per-packet Core0 round trip.

These absolute throughput and task-time values are not the production ceiling.
The image contains intrusive phase and coarse Core0 instrumentation. The clean
observer-free fixed-67-slot indexed path previously delivered approximately
120 Mbit/s, and its matching coarse image measured 38.94--39.10% Core0 task
residence. Historical 123--124 Mbit/s results also depend on a controlled lab
and channel baseline and must not be compared to this diagnostic image without
same-image and lab-provenance controls.

## Sparse progress and the former hang

Low-rate same-ELF run `1788274182792-001418e7` passed two 8-second cycles at
about 1.13 Mbit/s. It serviced 24/25 candidates and grants, with no candidate
full, grant full or rejected grant. This rejects a current saturation-only
progress dependency.

The run recorded `radio_wakes=0`: existing payload/radio activity carried all
control work, so it did not independently exercise the dedicated signal wake.
The code has the required level-latched check/arm/recheck invariant and unit
coverage, but a HIL scenario in which a control publication is the sole wake
source remains an explicit gate before authority.

The exact historical no-lock hang was never proven to be caused solely by a
mutex removal. The current topology removes two concrete risk classes:
unbounded drain-until-empty and edge-only waiter arming. It is therefore fair
to say the current baseline no longer reproduces the hang, but not to claim
that every older symptom has a single established root cause.
## Comparison with mature Wi-Fi stacks

The applicable Linux/mac80211 lesson is topology, not its generic machinery.
Current mac80211 documentation states that intermediate queues are per
station/per TID, with additional per-station and per-VIF queues, specifically
to keep hardware queues short and provide fairness between stations and
interfaces. A driver may ask mac80211 for the next TXQ, dequeue work from that
selected queue, and return it afterward:

- [mac80211 software TX queue contract](https://codebrowser.dev/linux/linux/include/net/mac80211.h.html#90)
- [mt76 selected-TXQ burst dequeue](https://codebrowser.dev/linux/linux/drivers/net/wireless/mediatek/mt76/tx.c.html#458)
- [mac80211 station airtime/AQL initialization](https://codebrowser.dev/linux/linux/net/mac80211/sta_info.c.html#695)
- [mac80211 AQL admission checks](https://codebrowser.dev/linux/linux/net/mac80211/tx.c.html#4165)

The reusable rules are:

1. software backlog is keyed by scheduling identity;
2. one queue is selected before hardware admission;
3. the selected queue is drained as a burst, not one scheduler round per
   packet;
4. hardware/DMA work stays short and globally bounded;
5. fairness is charged by airtime, not packet count;
6. already-admitted estimated airtime is a separate AQL-like resource;
7. sleeping or progressless peers do not retain scarce hardware credits.

The S31 difference is that cached PSRAM cannot be used directly by the current
Wi-Fi DMA path, while internal SRAM is scarce. The accepted direct-construction
path adapts the mature topology without copying Linux's `skb`, qdisc, RCU or
allocator design and without adding a complete-frame staging copy.

## Target ownership and policy

The target is one physical-radio scheduler with a hierarchy similar to:

```text
physical radio
    -> VIF weighted DRR                 (STA versus AP)
    -> access category
    -> AP peer/TID weighted airtime DRR
       or STA upstream peer/TID
    -> BA / PS / rate / retry eligibility
    -> AQL-like pending-airtime limit
    -> bounded burst grant
```

The value crossing from Core1 to Core0 should describe demand, not carry a
packet owner:

```text
Demand {
    opaque key,
    queue/lifecycle epoch,
    active state,
    bounded backlog or useful watermark,
}
```

Publication should be level-like and coalesced: empty-to-nonempty activation,
meaningful backlog watermark change, empty/deactivation, lifecycle invalidation
or power-state change. It must not send one message for each enqueue.

The return value is an affine quantum:

```text
Grant {
    exact key and epoch,
    frame credits,
    estimated airtime quantum,
}
```

Core1 consumes the grant locally for a contiguous run. A grant is permission,
not a pre-reserved SRAM buffer and not airtime already consumed. Physical SRAM
is still claimed by the shared pool. Estimated pending airtime is charged only
after successful final admission, then reconciled from Core0 completion/BA/
retry/PHY feedback. This avoids counting a deferred packet, a failed SRAM
claim and a hardware-pending frame as three units of work.

Sparse traffic must never wait for BA32. BA32 is a maximum aggregation/admission
horizon, not a minimum batch. An uncontended one-packet queue is eligible
immediately; the scheduler can send the available prefix and return/expire the
unused logical quantum. Under saturation it should amortize one control
transaction across a full aggregate.

### Payload and backlog ownership

The normal fast path should not create a second queue of complete Ethernet
frames in PSRAM. The preferred representation is:

```text
canonical upper-layer payload owner in PSRAM
    -> egress catalog stores handle + scheduling metadata
    -> selected handle receives a radio grant
    -> final frame is constructed directly in SRAM
```

For UDP, the current indexed packet slot is already such an owner. TCP already
retains unacknowledged bytes for retransmission, so its provider should expose
an emit-able connection/range rather than copy those bytes into another Wi-Fi
queue. The final emission still reads/copies payload bytes into the physical
SRAM representation while constructing headers; what is avoided is an extra
complete-frame representation and a later terminal copy.

This does not eliminate buffering. When instantaneous producers outrun the
radio, the system must apply bounded queueing, backpressure or drop. Sleeping
peers, multicast/DTIM traffic, forwarded L2/L3 packets and generated control
traffic may not all have a stable UDP/TCP owner. Each needs either a retained
upper-layer owner, a bounded role-specific owner or an explicit drop/
backpressure policy. Therefore “the TX catalog contains handles only” is the
target; “Wi-Fi can never need a PSRAM-backed owner” is not yet established.

The attractive interpretation of 67 SRAM slots as roughly one current BA32,
one next BA32 and a small reserve is plausible but unmeasured. It must not
become a correctness assumption until ownership counters show actual current,
standby, retry and control occupancy under saturation.

### Generic priority versus Wi-Fi QoS

Xarxa should carry generic priority intent, not application semantics and not
Wi-Fi policy. Possible sources are explicit packet/socket priority, IPv4/IPv6
DSCP and VLAN PCP. The Wi-Fi driver maps that value through role policy into
user priority/TID, WMM access category, admission-control downgrade and EDCA.
The resulting radio queue identity is at least `VIF + peer generation + TID`;
BA sequence and aggregation state are per peer/TID.

The codebase already has validated VLAN/DSCP WMM classification, station-safe
DSCP bleaching, WMM parameter parsing and STA support for multiple TX TIDs.
That classifier currently consumes a completed Ethernet frame. It therefore
cannot select a pre-SRAM queue without moving equivalent generic intent to the
earlier Xarxa egress contract. AP TX still owns only TID 0, so multi-TID AP
queueing also requires AP BA, sequence, hardware-queue and policy integration;
changing `EgressRoute.traffic_class` alone is insufficient.

QoS service differentiation and peer airtime fairness must remain separate.
A latency-sensitive AC may be selected earlier, but its actual airtime still
charges the peer/VIF deficit. An elevated marking must also pass the role's
admission and trust policy; Xarxa must not turn arbitrary DSCP into an
unconditional Voice grant.

Critical control and management traffic needs a small global reserve/bypass
with explicit classification. It must not wait behind bulk data airtime, and
bulk data must not consume the reserve.

## Required Xarxa refactor depth

Do not start with a complete owned-packet rewrite. The smallest architecture
which covers the missing boundary is an interface-owned egress catalog with
protocol-specific providers:

- UDP provider: existing indexed per-key queues and independently removable
  payload slots;
- TCP provider: the connected remote route/key plus currently emit-able work;
- raw/ICMP/generated control provider: explicit bounded control class or
  reserve path;
- interface arbiter: one active-key catalog, one granted burst and bounded
  provider scanning.

The provider API should expose only generic facts: opaque key, epoch,
nonempty/backlog, selected head and successful/failed dispatch. It must not
expose Wi-Fi AID, BA, power-save or airtime concepts. Those stay in the driver
and Core0 policy.

This is an invasive but bounded refactor of Xarxa egress. It can replace the
current mixture of interface burst state plus UDP-private selection without
rewriting RX, IP parsing, TCP state machines or socket ownership. If the
provider model later proves unable to express ownership and reclamation
without scans, the next escalation is an independently owned TX packet pool,
not a wholesale stack rewrite.

## Known gaps and risks

1. **Radio-wide scope.** Current control covers only the AP endpoint; STA+AP
   fairness is structurally impossible until STA demand enters the same Core0
   owner.
2. **Echo policy.** Core0 currently grants every candidate and has no airtime,
   AQL, BA, PS or rate decision.
3. **Demand lifecycle.** The candidate/refill protocol has exact serials but
   no explicit empty/deactivation or unused-quantum expiry contract.
4. **Grant horizon.** Refilling at eight remaining credits with another 32 can
   leave a local permission horizon larger than one BA32. That is harmless in
   shadow mode but must be explicitly bounded before authority.
5. **Protocol coverage.** UDP has the required removable queue geometry;
   TCP/raw/control paths need a deliberate provider or bypass contract.
6. **Pre-classification lifetime.** Current packet storage cannot distinguish
   traffic intentionally preserved across reassociation from traffic which
   policy should revoke.
7. **TID/AC.** Generic route priority is currently always zero and the AP
   grant/data path is best-effort TID 0 only. Early classification, AP
   per-TID BA/sequence state and role-specific WMM policy are all missing.
8. **Wake proof.** Sparse HIL passed, but the dedicated control-only wake edge
   has not been isolated on hardware.
9. **CPU accounting.** The next design must reduce total work, not merely move
   Core0 work to Core1. Both cores and normalized cycles/frame/burst must be
   reported together.
10. **Absolute ceiling reproducibility.** Historical 123--124 Mbit/s results
    need a fixed channel, route, OpenWrt state, source archive and same-ELF
    control before they become a regression gate.

## Evolutionary implementation plan

### Phase 0: preserve the oracle — complete

- keep the current direct-SRAM 67-slot path;
- keep the same-ELF egress-control switch;
- retain the exact clean A/B/A and sparse HIL runs above;
- do not make shadow grants authoritative.

### Phase 1: specify the generic demand catalog — complete

- define key activation, backlog update, empty/deactivation, epoch invalidation
  and unused-grant return/expiry;
- define bounded scanning and work budgets;
- define generic priority provenance and lifecycle without embedding Wi-Fi
  TID/AC or trusting arbitrary application markings;
- add model and host tests for lost wake, stale epoch, duplicate update, sparse
  flow, one saturated plus one sparse flow, multiple traffic classes and
  all-keys-deferred progress;
- keep packet admission unchanged.

### Phase 2: refactor Xarxa egress providers — UDP shadow complete

- move active-key arbitration to one interface-owned catalog;
- adapt the existing UDP indexed queues without changing their payload owner;
- add explicit TCP and control providers instead of falling back silently to
  head-only behavior;
- make provider identity include generic traffic class where ordering and
  radio aggregation require it;
- preserve `GlobalExhausted` versus `KeyDeferred` and direct SRAM emission;
- measure observer-free single-peer and two-peer paths before proceeding.

### Phase 3: make control physical-radio-wide

- generalize the AP-only control plane to STA and AP VIF demand;
- keep exactly one affine Core0 scheduler owner;
- replace echo-candidate semantics with activation/demand state and bounded
  burst/airtime grants;
- retain a same-ELF shadow/off switch and finite work budgets.

### Phase 4: implement policy in shadow

- hierarchical VIF then peer/TID weighted airtime DRR;
- AQL-like estimated pending airtime charged at successful SRAM admission;
- completion reconciliation from actual rate, retry and BA results;
- power-save/progressless-peer eligibility and a separate control reserve;
- compare every shadow decision with actual queue/radio progress.

### Phase 5: authoritative cutover

- make missing or exhausted key grants return `KeyDeferred`;
- prove the empty-pipeline bootstrap through demand activation;
- prove immediate sparse dispatch without waiting for a full aggregate;
- perform same-ELF off/shadow/authoritative A/B and retain rollback until all
  gates pass;
- remove the old non-authoritative path only after the cutover is accepted.

### Phase 6: fairness and scale qualification

- 1, 2, 4, 8 and 15 associated AP peers;
- one, two and many saturated peers;
- one saturated plus sparse peers and all-sparse traffic;
- equal and unequal PHY rates, retry-heavy peer, sleeping peer;
- mixed BE/BK/VI/VO intent, per-TID BA state and admission-control downgrade;
- simultaneous STA+AP traffic and per-VIF weights;
- TX-only, RX-only and bidirectional load, including HE20 STA.

## Acceptance gates

Correctness gates:

- no stale-generation admission, duplicate owner, leak or within-key reorder;
- no candidate/grant overflow in supported saturation;
- bounded progress when every data key is deferred;
- control traffic progresses under bulk saturation;
- fixed SRAM ownership remains independent of associated-peer count;
- minimum DMA/BA credit and queue invariants remain valid.

Performance gates:

- no more than 1% same-ELF single-peer throughput regression from the disabled
  direct-SRAM control;
- recover the clean channel-controlled HT40/MCS7 production ceiling; use
  120 Mbit/s as the minimum gate and the historical 123--124 Mbit/s result as
  the target pending a reproduced channel-13 baseline;
- preserve approximately 120 Mbit/s aggregate for two saturated AP peers with
  the fixed 67-slot pool;
- Core0 task residence remains below 40% at the accepted ceiling;
- Core1 and total normalized work do not increase merely to satisfy the Core0
  gate;
- control overhead is at most 1 percentage point per core and is reported as
  cycles per granted burst and per admitted frame;
- sparse-flow service latency is bounded independently of BA32 fill;
- radio idle gaps, aggregate depth, pending airtime and per-peer service gap
  are reported alongside throughput.

## Explicit non-goals for the next phase

- no complete-frame PSRAM staging in the production fast path;
- no SRAM pool growth proportional to peer count;
- no per-packet cross-core candidate/grant exchange;
- no shared cross-core allocator or mutex around packet admission;
- no attempt to teach Xarxa Wi-Fi association, BA, power-save or rate policy;
- no immediate rewrite of Xarxa RX or all transport state machines;
- no cache-layout or hot-SRAM optimization used to hide an unlocalized
  scheduler regression;
- no authoritative fairness claim from throughput alone.

The next code change completes Phase 2 protocol coverage and measures the UDP
publisher cost. Only after that boundary is stable should Phase 3 replace the
AP-only Core0 echo with physical-radio-wide demand state and real burst/airtime
grants.
