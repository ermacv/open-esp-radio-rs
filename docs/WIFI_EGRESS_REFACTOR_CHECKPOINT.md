# Wi-Fi egress refactor checkpoint

Status: canonical architecture checkpoint, 2026-09-02. This document defines
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
- Core1 publishes bounded demand/progress values to Core0, never packet
  payloads or DMA owners; Core0 returns one affine burst/airtime grant through
  a bounded transport, while Xarxa deliberately consumes it in shadow mode.

The missing part is not another packet-copy mechanism. It is a physical-radio
wide policy over the demand catalogs already mirrored for both STA and AP.
The former AP-only candidate/grant echo was measured, rejected and removed:
it added packet-path work without expressing BA, power-save, rate, AQL or
airtime policy. The retained mirror proves lifecycle identity, bounded service
and physical-radio ownership, but it does not yet provide fairness or
`KeyDeferred` authority.

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
- Embassy forwards the lifecycle exactly. At this historical Phase 2 boundary
  the open-radio network device did not yet consume it; Phase 3 now mirrors it
  into the physical Core0 radio owner.

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

## Phase 3 radio-mirror checkpoint

The next evolutionary step now transports the Phase 2 lifecycle into Core0
without making it authoritative. It deliberately does not enqueue an
unbounded history of callback events. Each Core1 interface owner retains two
bounded 16-key views:

```text
desired = latest Xarxa demand state
sent    = state already admitted into the ordered SPSC
```

When the SPSC is full, callback churn only changes `desired`. Once capacity
returns, the publisher reconstructs the minimal ordered suffix needed to make
`sent == desired`: a newer `Reset` supersedes unsent old-epoch work, an old
lifetime receives `Inactive` before a replacement `Active`, and level changes
coalesce to their latest observation. This makes memory proportional to the
maximum distinct radio keys, not to event rate or stall duration. A finite
four-update Core1 turn self-wakes when a successful turn leaves more diff; a
full transport is woken only when Core0 actually frees capacity.

STA and AP own independent SPSC streams, outboxes and telemetry. Their
Core0 endpoints share one level-triggered physical-radio wake and are held by
one affine dual-VIF owner. Every physical service turn gives each VIF its own
finite budget and alternates which VIF is serviced first. This is the required
ownership shape for future radio-wide VIF/peer/TID airtime policy; it avoids
both an MPSC scheduler lock and an AP-only policy owner. The STA stack now
publishes its single-radio-peer lifecycle through this path as well.

This remains a shadow mirror:

- `transmit_for` still admits valid keys from the shared 67-slot SRAM pool
  regardless of mirrored demand;
- the rejected AP run/refill candidate/grant echo has been removed rather than
  retained as dead compatibility machinery;
- Core0 only validates and stores lifecycle ordering. It does not yet select a
  VIF/key, charge airtime, inspect BA/power-save state, or return an
  authoritative burst/airtime grant;
- HIL telemetry reports the STA and AP control streams separately (`ONTXC
  vif=sta` and `ONTXC vif=ap`) so future STA+AP accounting cannot hide work by
  folding both interfaces into one counter set;
- the complete source-only audit, including all 117 isolated feature profiles,
  the final performance image, placement and stack-frame checks, passes with
  this ownership shape;
- AP same-ELF HIL after removal of the old echo passes the one-percent CPU and
  throughput gates described below.

### Phase 3 same-ELF result

The first hardware gate used clean commit `c494994d`, channel 13, one replayed
`diagnostic-core0-rx-coarse` artifact and application SHA-256
`4032c4ec0441e952473bf7f975ca3fab48f7112717ceab574b77acc6a633c27a`.
Laptop WLAN was down and the captured route/lab provenance used the Ethernet
OpenWrt fixture.

The low-rate AP run `1788280542958-0015ca40` completed both cycles. Each cycle
published and consumed 18 demand transitions with no full or rejected update;
the inactive STA stream remained empty. Saturated AP A/B/A then used that exact
saved firmware:

| Mode | Run | Device TX throughput | Demand updates | Old grant echoes |
| --- | --- | ---: | ---: | ---: |
| enabled A | `1788280773846-0015d2cf` | 116.168, 115.734 Mbit/s | 6, 7 | 4,933, 4,914 |
| disabled B | `1788280859305-0015d52a` | 119.882, 119.981 Mbit/s | 0, 0 | 0, 0 |
| enabled A | `1788280946035-0015da77` | 116.729, 115.490 Mbit/s | 6, 6 | 4,957, 4,904 |

This pair does **not** measure the new lifecycle mirror alone. The enabled AP
mode still includes the temporary candidate/grant echo once per aggregate,
whereas lifecycle traffic is only six or seven state changes per cycle. The
roughly 3.3% AP ceiling penalty therefore blocks the old echo from becoming a
production authority; it is not evidence against the lifecycle mirror.

STA provides the clean discriminator because its `SingleRadioPeer` topology
has no old AP grant key. Enabled A/B/A runs `1788281092418-0015debe`,
`1788281384943-0015ec50`, and `1788281618275-0015f045` produced average device
throughput of 117.984, 117.991, and 117.981 Mbit/s respectively. Enabled runs
published only 8--12 lifecycle transitions for roughly 120,000 packets and
reported no full or rejected update. Explicit Core0 lifecycle service cost was
about 32--44 thousand cycles per 12-second repetition. Thus Phase 3 demand
transport is lifecycle-granular and has no measured ceiling loss; the AP
candidate/grant oracle is the expensive component that must be removed.

The A/B also exposed one unnecessary implementation coupling: attaching the
control endpoint made STA execute the AP-only empty burst-lease check on every
admission. Commit `24730873` separated demand from the AP echo. The subsequent
cut removed `EgressCandidate`, `EgressGrant`, `EgressBurstLease`, both echo
queues and all packet-path maintenance; both STA and AP retain only the
lifecycle mirror.

### Accepted demand-only checkpoint

Commit `939bf11273ef6e8628564634bb7688518195bdc3` is the first clean checkpoint
with the rejected request/grant echo completely removed. Its low-rate source
run `1788284016570-001656bb` and saturated replay runs
`1788284217831-0016588b`, `1788284303627-00165ae1`, and
`1788284391263-00165b65` proved that the AP ceiling returned from roughly
116 Mbit/s to 120.5 Mbit/s. The demand-only enabled average was 120.505 Mbit/s
and the same-ELF disabled average was 120.628 Mbit/s, a 0.10% difference.

That checkpoint still showed a small but repeatable Core1 cost: enabled
admission was about 42 cycles/attempt more expensive and full TX consume about
77 cycles/frame more expensive. Inspection found that every network poll
called the lifecycle outbox flush, and the empty flush rescanned up to sixteen
cached-external key slots even after `desired == sent`. This work was unrelated
to packet admission or radio policy.

Commit `ecf34f404f3ebfa50acef99ea510370de72743f8` adds one persistent dirty bit
to the Core1 lifecycle owner and a local retry bit to the recoverable network
device. The ordinary synchronized poll is now O(1); a blocked outbox still
retries after Core0 frees SPSC capacity, and reconstruction preserves an
already-pending retry. The full source-only audit again passed all 117 isolated
feature profiles, the production and diagnostic builds, placement, stack-frame
and direct-target checks.

The exact HIL source run was `1788284922409-001683a6` on channel 13, with laptop
`wlan0` down and the OpenWrt ingress Ethernet interface up. Its archived image
subjects are:

```text
application SHA-256: 245e527413b249591b910d88150f8bd66c5452e8f0e75384f5f99b4a3611b96c
runtime ELF SHA-256: a390a6f4e8fb973379c52c0e127a54517df963d0e3a2a21d341c24f7e4bdb4d5
build id:            2a9f449ebb72d4dceba3210cbb808c1e6a72e05689126e20651378f059ce4b6b
runtime CRC:         2f1fb9cd
```

The low-rate run delivered 1.130486 Mbit/s in both cycles, published and
consumed 18 AP demand transitions per cycle, and reported no full or rejected
update. Saturated A/B/A replayed the exact image:

| Mode | Run | Device TX throughput | AP demand updates |
| --- | --- | ---: | ---: |
| enabled A | `1788285396109-001689a7` | 120.513, 120.642 Mbit/s | 12, 6 |
| disabled B | `1788285482991-00168bb7` | 120.373, 120.503 Mbit/s | 0, 0 |
| enabled A | `1788285571577-00169037` | 120.485, 120.580 Mbit/s | 12, 12 |

All six cycles negotiated BA32 and reported zero OpenWrt TX retries and
failures. The normalized averages were:

| Metric | enabled | disabled | difference |
| --- | ---: | ---: | ---: |
| throughput | 120.555 Mbit/s | 120.438 Mbit/s | +0.097% |
| Core0 radio cycles / elapsed cycles | 41.563% | 41.523% | +0.039 pp |
| Core1 admission cycles / attempt | 972.0 | 970.1 | +1.9 cycles |
| Core1 consume cycles / admitted frame | 13085.0 | 13085.9 | -0.9 cycles |

The lifecycle mirror is therefore performance-neutral at the resolution of
this A/B. The earlier residual Core1 regression came from unnecessary empty
outbox scans, not from lifecycle transport itself. Absolute Core0 occupancy in
this intrusive diagnostic image is above the production `<40%` goal and must
not be relabelled as a production result; the same-ELF comparison establishes
only the cost of this control plane.

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

The current implementation contains no packet-frequency request/reply
protocol. Core1 maintains the latest desired lifecycle state and the state
already accepted by its affine SPSC. It publishes only the minimal ordered
`Reset`, `Inactive` and `Active` suffix. Core0 validates those updates into one
bounded radio-side view per VIF.

Current constants are:

```text
demand SPSC depth per VIF:    16
Core0 service budget:          4 updates per VIF/turn
Core1 publication budget:      4 updates per callback
```

The finite Core0 budget is a correctness property: the radio owner must not
turn a continuously changing control stream into an unbounded synchronous
turn. Exhausting a budget leaves a level-latched pending flag for the next
turn. STA and AP have independent demand queues but share one physical wake;
the dual owner alternates first service so one VIF cannot starve the other.

`EgressWaitOr` uses check, arm, recheck, then sleep. This closes the usual
producer-publication versus waiter-arming lost-wake window. The unique mutable
`EgressRadioOwner` travels with the connected Core0 datapath owner; the shared
network side only holds a small wake capability.

The lifecycle mirror itself is observational:

```text
Xarxa demand lifecycle
    -> bounded Core1 outbox
    -> affine SPSC
    -> Core0 demand view
```

The separate Core0 shadow policy now derives BA, power-save, rate and modeled
airtime opportunity for each visible physical transaction, but it returns no
grant and does not alter packet order. Packet admission therefore remains
under the existing direct-SRAM arbiter. The paired HIL gate proves that the
current recommendation is requested too late to choose a VIF. It also proves
that a physical owner may outlive the software demand from which it was
materialized, as the ownership model permits. The next policy boundary must
issue an affine quantum before final SRAM admission and retain a separate
admission receipt afterward; it must not resurrect an echo of a Core1 packet
request.

## Reviewed selectable-work boundary

The intended production contract is bidirectional in control, but it is not a
driver callback into a socket:

```text
Core1 / Xarxa                         Core0 / physical radio owner

opaque-key readiness  ------------> bounded demand catalogue
                                      BA / PS / rate / AQL / airtime policy
cached burst permission <---------- affine key + frame/airtime quantum
selected-key final SRAM frames ----> MAC / DMA / radio
modeled terminal airtime <---------- completion feedback
```

Xarxa remains the poll-driven owner of protocol state. It asks the driver for
final backing through `transmit_for(key)` and may try another eligible key when
that key is deferred. Core0 never names or invokes a UDP socket or TCP
connection. The reverse direction is only a bounded permission for one opaque
physical egress identity. One permission must cover a radio transaction or
airtime quantum; a request/reply per packet is rejected.

This model is already mechanically plausible for the indexed UDP path. Xarxa
can retain an exact per-key queue head after `KeyDeferred`, then construct the
eventually selected packet directly in final SRAM. It does not require a
separate complete-frame PSRAM staging queue or a transferable packet object.
Socket-owned PSRAM payload is the canonical backlog; SRAM is the short radio
execution horizon.

It is not yet a generic stack contract:

- only UDP publishes keyed demand and exposes independently removable keyed
  queue heads;
- the current demand stream reports a frame count/high watermark, not a
  stable generic transmission opportunity for every protocol;
- TCP decides segment eligibility from stream, congestion, retransmission and
  timer state during `dispatch()`. Its current implementation also mutates
  some pre-transmit state before the fallible emit boundary. Making TCP
  selectable therefore requires an audited prepare/commit or rollback
  boundary; it cannot be declared covered merely by adding its buffered byte
  count to the UDP catalogue;
- route traffic class is currently zero and AP radio identity is best-effort
  TID 0. QoS intent can later be generic Xarxa metadata, while TID/AC and EDCA
  remain Wi-Fi-owned policy.

The evolutionary cut remains UDP shadow first. It will prove that a Core0
recommendation derived from early demand corresponds to the actual key chosen
by the unchanged Xarxa dispatcher, measure disagreement and policy cost, and
leave `transmit_for` authoritative. TCP, generated control traffic and an
authoritative grant transport are separate gates.

## Portable airtime-policy kernel

`open-esp-radio-wifi-softmac::egress` now defines the executor- and
chip-independent policy state used by the next shadow phase. It owns no packet,
DMA, wake, peer table or association object. Callers supply generation-bound
demand and a radio-valid opportunity containing a bounded frame count,
estimated transaction airtime, queue pending limit and weight.

The kernel implements hierarchical deficit service across VIFs and queues,
plus independent global, VIF and queue pending-airtime limits. A queue keeps
the cursor while it has positive deficit, so equal transaction counts cannot
masquerade as airtime fairness. A sole eligible queue remains work-conserving,
including a sparse packet or a transaction larger than one quantum. Completion
returns an affine non-`Copy` admission receipt and corrects estimated debt with
the exact modeled terminal PPDU duration.

Demand lifetime and physical pending credit are deliberately separate. Removing
or resetting a demand invalidates an outstanding recommendation immediately,
but a queue with admitted airtime remains as an unreusable tombstone until its
terminal receipt returns. This prevents reassociation from leaking pending
credit or letting an old transaction charge a newly reused slot.

Host tests currently prove sparse immediate selection, STA-versus-AP VIF
fairness, equal-airtime service for different peer costs, weighted service,
AQL isolation, oversized-transaction progress, exact versus divergent shadow
observation, affine selection, stale-lifetime rejection and terminal pending
credit after demand removal. The kernel is not yet wired into packet
admission, so none of these model tests is a throughput or fairness claim for
hardware.

### Physical-owner demand mirror

The product composition now retains one `DatapathEgressAirtimePolicy` beside
the permanent dual-interface network frontier. It is static Core0-owned state,
not part of an async runner future and not another cross-core allocator. The
two radio-side lifecycle mirrors label accepted events with VIF 0 or 1 and
apply only those accepted `Reset`, `Active` and `Inactive` transitions to the
portable kernel. Rejected or stale transport events are counted and never
reach the policy mirror.

This wiring still has no scheduling authority. It does not issue a grant,
reserve SRAM, alter `transmit_for`, or choose the packet returned by the
existing production queue. The configured quanta and pending limits are
therefore provisional shadow parameters, not a claimed production fairness
policy. A frame count is never relabeled as airtime.

The next source-level shadow step is now implemented. Immediately before a
role attempts to publish a new physical radio transaction, the physical
policy calls `select_next` using role-derived facts. STA accepts
only the current single-radio class-zero queue with an operational TID-0 BA
window and a fresh HT rate-control result. AP additionally requires the exact
authorized AID slot plus association epoch, an active (non-sleeping) peer,
current HT capabilities and an operational TID-0 BA agreement. Standalone and
STA+AP compositions implement the same value-only contract; observing a
paired role does not lend the shared physical TX owner.

Demand currently carries a ready-frame count but no exact queued byte
geometry. The provisional opportunity therefore models every admitted frame
at the explicit maximum Ethernet capacity, then applies the negotiated S31
A-MPDU byte ceiling, the rate-dependent vendor ceiling and the exact HT PPDU
duration model. This is a conservative data-PPDU estimate, not measured
airtime and not complete medium occupancy: contention, protection, SIFS and
BlockAck remain excluded. HE, ordinary non-aggregate, group/control and
non-zero traffic classes fail closed rather than receiving a fabricated cost.

The recommendation is compared with the immutable egress key stored beside
the actual final-SRAM packet. Exact and different-queue outcomes are recorded
without changing admission. Estimated pending credit is immediately returned
with the same modeled value in this comparison-only phase, so the shadow DRR
state advances while the production AQL/hardware horizon remains untouched.
Selection runs once per physical initial/prepared publication; continuation
leases already joining that burst do not repeat the queue scan. An exact
selection reuses the already validated opportunity instead of querying role
state a second time. Host tests cover exact and divergent decisions, explicit
cancellation when a role publishes no transaction, and prove that comparison
leaves no pending airtime.

The scheduler also keeps an exact `sole_active_slot` summary. Lifecycle
updates recompute it outside the saturated packet path. With one active
demand, `select_next` therefore applies the same DRR deficit and AQL rules in
O(1), without scanning 32 queue slots or constructing a 32-entry opportunity
array for every A-MPDU. The existing bounded scan remains unchanged for zero
or multiple active demands. A lifecycle test covers replacement of the sole
active queue so this fast path cannot retain a stale slot.

### Role-derived shadow HIL result

The first HIL pass found two real boundary defects before it produced a valid
comparison. The standalone station role prepared the successor aggregate by
claiming its first network lease inside the role after publication, bypassing
the physical runner. Removing that internal claim restored outer ownership
while retaining the double-buffered continuation drain. A second defect was
conventional trait forwarding: `DatapathNetwork for &mut N` forwarded the
lifecycle service but silently inherited the default `false` implementations
for recommendation and observation. A focused regression test now proves
forwarding of preparation, observation and cancellation.

The clean station A/B uses one archived image from AP source run
`1788301060881-00191cf6`, application SHA-256
`59dac66e91c6be2b98f51666012167021181240addb310e318c062446fcbd9ad`.
Enabled run `1788301452504-001926c3` and disabled run
`1788301618367-001928c1` replay that exact firmware:

| Mode | Device TX throughput | Recommendations | Exact | Other outcomes |
| --- | ---: | ---: | ---: | ---: |
| enabled | 122.865, 122.759, 122.831 Mbit/s | 3,913 / 3,910 / 3,912 | all | 0 |
| disabled | 123.023, 122.979, 123.054 Mbit/s | 0 | 0 | 0 |

Every enabled snapshot was ready. Key, identity, traffic-class, role, rate,
BlockAck and geometry rejection counters were all zero. Disabled performed no
recommendation, observation or snapshot query. The median ceiling difference
was -0.192 Mbit/s (-0.16%). Median visible Core0 TX phase work was 6,789.9
versus 6,684.0 cycles per datagram, a difference of about 106 cycles. Complete
radio-task poll residence was 4.140 versus 3.902 seconds per 12-second sample,
or approximately +1.98 percentage points of one core. This larger total is the
correct cost gate because recommendation work surrounds, rather than sits
inside, the existing TX phase samples. Core1 network residence did not
increase, so the cost was not merely transferred to Core1.

That first station result established correspondence but did not pass the
desired `<= 1 pp` production-overhead gate. It predates the constant-time
sole-active-demand selection described above.

The AP source run `1788301060881-00191cf6` passed at 119.479 and
119.673 Mbit/s with BA32, zero OpenWrt retries/failures and 5,073/5,082
terminal aggregates. It produced only one recommendation per cycle. All role
snapshot rejection counters were zero, so this is not missing BA, rate,
association or power-save state. The AP role constructs and fills successive
`PreparedStandby` aggregates from its Core0-owned per-flow arena, then exposes
only the prepared publication to the outer runner. Consequently the raw
network FIFO head is not the universal physical transaction boundary.

This invalidated the source-level assumption in the original shadow hook: an
outer FIFO removal is not necessarily the egress identity actually chosen by a
role-specific aggregator. The implementation now exposes the exact selected
transaction identity as a `DatapathTxStart` result at initial and prepared
publication boundaries. Core0 prepares one recommendation immediately before
the physical start, then consumes that result exactly once; a start which
publishes no transaction explicitly cancels the recommendation. There is no
mutable `last_started_metadata` lookup and no transaction identity retained
in the long-lived role owner graph.

That ownership detail is material, not cosmetic. Persisting the metadata in
the owner graph grew the linked STA+AP async frame from 46.89 KiB to 55.64 KiB
and failed its 50 KiB audit. Removing the hidden persistent state restored
46.89 KiB. Returning identity in `DatapathTxStart`, while retaining metadata
only in a genuinely unpublished prepared transaction, produces a 48,208-byte
(47.08 KiB) frame and passes the same audit. The physical result therefore
both identifies the correct transaction and prevents small control metadata
from multiplying through async owner alternatives.

For a prepared AP aggregate, role selection necessarily happened while its
standby arena was built, before this publication-time shadow check. That is
valid for correspondence, but an authoritative scheduler must eventually
participate before standby flow selection. Continuation frames remain
unobserved.

The new boundary has now passed standalone AP and STA HIL. AP coarse run
`1788306058768-0019eef0` reported 5,043/5,043 and 5,029/5,029 exact
recommendations/observations. STA coarse run `1788306295546-0019f1fb`
reported 3,859, 3,864 and 3,860 exact pairs. Every different, cancelled,
unavailable, stale-key, stale-identity, traffic-class, role, rate, BlockAck and
geometry rejection counter was zero in both roles. AP retained BA32 and the
OpenWrt monitor reported zero retries and failures.

The post-O(1) AP task-residence A/B used enabled run
`1788305737503-0019ebd1` and disabled run
`1788305945339-0019edc6` from the same diagnostic ELF. Enabled delivered
119.919/119.809 Mbit/s and disabled 119.666/119.661 Mbit/s. Final radio-task
residence was 6.684 versus 6.566 seconds per 16-second cycle: +0.738
percentage points of Core0. Network-task differences did not show a sustained
transfer of this work to Core1. This passes the `<= 1 pp` shadow-overhead gate.
The corresponding pre-O(1) A/B (`1788305333257-0019e6c1` versus
`1788305539457-0019e868`) was above that gate, so the reduction is attributable
to removing the saturated single-demand scan rather than to a throughput
floor artifact.

The current STA same-ELF coarse comparison used enabled run
`1788306295546-0019f1fb` and disabled replay
`1788306469849-0019f38b`. Device throughput was
121.213/121.349/121.243 Mbit/s enabled and
121.874/122.229/122.078 Mbit/s disabled. The coarse image therefore shows a
median 0.73% ceiling cost, below the 1% throughput gate. Its radio-task counter
is not used as a standalone residence claim; the AP task-residence image is
the valid cost control.

STA+AP task-residence source run `1788306724574-0019f5f2` produced only
20.761 + 27.520 Mbit/s. Replaying the identical archived ELF immediately in
run `1788306928124-001a0120` restored 32.558 + 31.692 = 64.250 Mbit/s against
the 65 Mbit/s offer. The replay recorded 4.403 seconds of Core0 radio-task
residence and 6.394 seconds of Core1 network-task residence over the 17-second
traffic interval, approximately 25.9% and 37.6%. The first result is retained
as evidence of laboratory/run variability; it is not attributed to code and
it prevents treating a single paired-role pass as a ceiling proof. The paired
role path is operational and within the CPU target, but its current
task-residence image does not emit the coarse identity counters, so exact
simultaneous two-VIF correspondence is not claimed from that run alone.

### Typed paired-VIF identity gate

The paired `diagnostic-core0-rx-coarse` path now emits a dedicated
CRC-protected `WifiEgressPolicy` HIL record. It is not reconstructed from UART
text. The record contains global outcomes, a mutually exclusive breakdown of
every unavailable observation and separate selected/actual transaction,
frame and modeled-airtime totals for STA and AP. The host validates the
cross-field arithmetic and rejects missing evidence, stale/rejected policy
updates and any run in which either VIF performed no physical transaction.
The new `station-access-point-tx-egress-identity-core0` scenario additionally
sets the accepted different, cancelled and unavailable counts to zero.

That strict gate correctly fails on the current shadow implementation. Run
`1788309447235-001a9267` completed all three paired workloads with valid typed
evidence:

| Repetition | STA + AP throughput | Recommendations | Exact | Different | Unavailable |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 19.861 + 31.601 Mbit/s | 2,695 | 1,889 | 715 | 91 |
| 2 | 24.220 + 23.773 Mbit/s | 2,471 | 1,316 | 965 | 190 |
| 3 | 32.571 + 30.174 Mbit/s | 2,844 | 2,192 | 522 | 130 |

Across the three repetitions, all 8,010 recommendations reconcile exactly as
5,397 exact, 2,202 different and 411 unavailable observations. Of those 411,
410 report that the actual transaction's key has no active demand in the
Core0 policy mirror, one reports a missing actual key, and zero report no
recommendation or a role opportunity failure. Cancellation and rejected
update/observation counters are also zero. Both VIFs perform substantial
actual work in every repetition, so a dormant interface cannot explain the
result.

This result does not show that shadow airtime policy reduced throughput: the
policy is observational and cannot defer or reorder a frame. It does prove
that it cannot become authoritative at the current boundary. The production
paired owner first chooses a VIF with its historical admitted-frame counter,
then removes that VIF's frame, and only afterwards asks the independent
airtime-DRR policy for a recommendation. A disagreement is therefore an
expected policy comparison, not a packet-identity race. The `demand missing`
outcomes expose a second boundary problem: software readiness is being queried
after the corresponding packet may already have crossed into physical SRAM
ownership. The two frontiers must be separated before a
selection-before-claim experiment.

Source inspection and a focused ownership regression now explain that second
issue without treating it as a lost update. Xarxa reconciles its demand before
socket egress. It may then consume the final software packet and construct it
in a pinned SRAM owner. On the next stack observation the software queue is
empty, so Xarxa legitimately publishes `Inactive`; Core0 may consume that
ordered lifecycle transition before it removes the already-published SRAM
owner from the independent TX queue. The regression
`software_demand_can_close_before_its_pinned_radio_owner_is_consumed` proves
the complete valid sequence:

```text
Active(key)
    -> final SRAM materialization(key)
    -> Inactive(key)
    -> Core0 consumes the still-live SRAM owner(key)
```

Therefore `demand missing` is not evidence that the SPSC mirror lost an
activation. It exposes a category error in the shadow comparison: software
readiness and already-admitted physical work are different frontiers. Keeping
software demand artificially active until radio completion would merge
`QueueCredit` and `DmaCredit` again and is rejected.

The selection-before-claim design must instead create an affine bridge. Core0
selects an active demand and issues one bounded burst/airtime grant. Core1
spends that grant while materializing final SRAM owners and binds the grant's
demand identity to those admissions. A later `Inactive` ends only unadmitted
software readiness; it cannot erase an already-admitted receipt. Physical TX
and terminal BA/retry completion then reconcile the receipt. The current
post-materialization recommendation remains useful as a failing diagnostic
sentinel, but its zero-mismatch criterion is not the authority gate for that
future protocol; typed admission/grant correspondence must become the gate.

## Rejected candidate/grant experiment

The following measurements describe the now-deleted AP candidate/grant echo,
not the current demand-only control plane. `enabled` and `disabled` selected
that shadow protocol at startup while retaining the same Xarxa indexed queue
selection, route classification, 67-slot SRAM pool, MAC, radio, firmware and
ELF layout. Disabled mode did not restore the old global FIFO or disable AP TX.

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

The result established that even an aggregate-refill echo was the wrong
production abstraction: it paid Core1 lease maintenance and extra Core0
service while encoding no radio decision. The implementation was removed
instead of optimized.

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

1. **Radio policy.** STA and AP demand reach one Core0 shadow airtime policy,
   but the historical frame-count arbiter still chooses the actual VIF first.
   Paired HIL therefore observes both policy disagreement and physical owners
   whose software demand has legitimately ended. The shadow has no admission
   authority and currently compares different resource frontiers.
2. **Grant contract.** The rejected echo deliberately left no compatibility
   API. A real grant still needs key/lifecycle identity, bounded frame and
   airtime horizons, unused-quantum return/expiry, and completion accounting.
3. **Completion identity.** The final SRAM owner carries a compact
   `(VIF, physical-pool index)` tag. The indexed CPU-only sidecar retains the
   exact opaque egress key which Xarxa used at final admission, including the
   schedule epoch, association generation and generic traffic class. The slot
   cannot be reused while the affine packet owner is live, and direct,
   staged-promotion and Core1-materializer tests prove that the identity
   follows the same owner. The AP A-MPDU path now independently retains its
   exact `ApAssociationIdentity` through build, hardware ownership, retry and
   terminal release and diagnoses whether that generation is still current.
   This covers the fixed AP VIF/TID0 aggregate path only. Ordinary MPDU,
   A-MSDU, group/control, STA and an eventual grant's schedule epoch still
   need explicit completion binding before authoritative charging.
4. **Protocol coverage.** UDP has the required removable queue geometry;
   TCP/raw/control paths need a deliberate provider or bypass contract.
5. **Pre-classification lifetime.** Current packet storage cannot distinguish
   traffic intentionally preserved across reassociation from traffic which
   policy should revoke.
6. **TID/AC.** Generic route priority is currently always zero and the AP
   grant/data path is best-effort TID 0 only. Early classification, AP
   per-TID BA/sequence state and role-specific WMM policy are all missing.
7. **Wake proof.** Sparse HIL passed, but the dedicated control-only wake edge
   has not been isolated on hardware.
8. **CPU accounting.** The demand-only control plane now passes same-ELF
   accounting. The next policy must still reduce total work, not merely move
   Core0 work to Core1; both cores and normalized cycles/frame/burst must be
   reported together.
9. **Absolute ceiling reproducibility.** Historical 123--124 Mbit/s results
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

### Phase 3: make control physical-radio-wide — complete

- generalize the AP-only control plane to STA and AP VIF demand;
- keep exactly one affine Core0 scheduler owner;
- replace echo-candidate semantics with activation/demand state;
- delete the rejected echo queues and packet-path lease machinery;
- retain a same-ELF shadow/off switch and finite work budgets;
- measure AP after echo removal before adding a new return path;
- eliminate steady-state lifecycle outbox scans and pass the same-ELF
  throughput/Core0/Core1 gate.

### Phase 3.5: retain exact physical-owner identity — complete

- retain the final-admission `EgressKey` in one CPU-only sidecar entry per
  physical pool slot;
- carry only the two-byte `(VIF, pool index)` handle through aggregate, retry
  and role state machines;
- publish and read the sidecar through an explicit Release/Acquire edge;
- preserve the identity through direct SRAM, staged promotion and the
  diagnostic Core1 materializer without enlarging DMA storage or queue
  entries;
- reject embedding the complete key in every `PinnedTxFrame`: the linked-image
  audit measured a 54,112-byte STA+AP frame against its 51,200-byte budget,
  a 26,976-byte AP control frame against 26,624, and an unreviewed 11,216-byte
  AP network-TX frame. The compact handle restored the accepted values to
  47,760 and 22,656 bytes and removed the unreviewed oversized frame;
- keep the key observational until role-specific identity correspondence and
  completion accounting are proven.

The clean task-residence A/B/A at commit
`0f2b076967ba89e1ffae1f231837a48728d776ce` used one archived image on channel
13. Source run `1788287669374-00170b6a` and replays
`1788287881508-00170e1b` and `1788287969481-00170f78` all used build ID
`ad451ada2ef8b367ab7f879775fa831927eb8ce5782c2194289308d475e09a07` and
application SHA-256
`7812302002db6c4153dd81798cd5a2ef2fdab41b4eee60bcd63e95f8d117ba1a`.
All cycles negotiated MCS7/40 MHz and reported zero OpenWrt TX retries and
failures.

| Metric | enabled A/A | disabled B | difference |
| --- | ---: | ---: | ---: |
| throughput | 120.795 Mbit/s | 120.952 Mbit/s | -0.129% |
| Core0 radio task residence | 37.715% | 37.767% | -0.051 pp |
| Core1 `network + udp_tx` task residence | 75.971% | 76.292% | -0.321 pp |

The switch controls the demand mirror, not sidecar publication, so this A/B
confirms that the mirror remains neutral after the owner-identity change. It
does **not** isolate the cost of retaining exact metadata because the sidecar
is active in both modes. The absolute Core1 residence is also a first-class
constraint: reducing Core0 work by transferring it to the network core would
not satisfy the architectural goal.

### Phase 4: implement policy in shadow — standalone proven, paired gate active

- **Identity-correspondence boundary — implemented and proven for one AP
  peer.** The
  generic adapter decodes only interface, schedule epoch, associated-peer slot
  and generation, plus the unchanged generic traffic class. The AP reads that
  sidecar exactly once when a newly published frame first enters Core0 and
  compares it with its independently admitted `ApAssociationIdentity` and
  current TID0 policy. Exact, unclassified, non-associated, role-unbound,
  interface, slot, generation and traffic-class outcomes are separate
  diagnostic counters. Reprocessing an already retained frame does not count
  it again. The observation is compiled only into TX-phase diagnostics and
  cannot authorize, defer, drop or rekey a frame.
- **AP aggregate terminal boundary — implemented and observed on HIL.** AP A-MPDU
  state no longer reduces association identity to a MAC address. Building,
  hardware-owned, retained-retry and completed states carry the exact peer
  slot and generation, and terminal release returns that identity together
  with the fixed aggregate PHY rate needed by a later estimator. The
  frequently returned completion-progress value remains limited to BA/retry
  observation rather than duplicating the terminal metadata. Diagnostic
  counters split current and stale terminal
  aggregates and frames; they neither accept nor reject completion. This
  change also fixes standby publication bookkeeping so timeout/collision
  accounting uses the batch actually started rather than the previous active
  aggregate. Coverage remains deliberately AP A-MPDU/TID0-only: it is not yet
  an airtime charge, and the stack-side schedule epoch is not yet attached to
  an aggregate transaction.
- **Airtime evidence boundary — specified and host-tested.** Normal S31 TX
  completion exposes no reviewed hardware airtime counter; its portable
  `MacTxStatus::airtime_micros` therefore remains `None`. The MAC now provides
  `ModeledHtAmpduPpduDuration`, a protocol-derived duration in exact 100 ns
  units for the mixed-format data PPDU implied by one published A-MPDU byte
  length, MCS, width and GI. It includes the HT preamble, service and BCC tail
  bits, but deliberately excludes contention/backoff, protection, SIFS and
  BlockAck. Submission also does not prove that a PPDU reached the medium.
  Future accounting must consequently retain three different meanings:
  admission-time estimated pending work, completion-time modeled published
  PPDU duration, and hardware-measured airtime (currently unavailable). BA and
  retry results may select the exact modeled publication lengths, but cannot
  upgrade their provenance to a measurement. Host tests cover HT20/HT40,
  LGI/SGI fractional duration, empty ownership rejection and accumulation of
  retry publications without saturation.
- **Role-derived recommendation boundary — implemented and HIL-proven for
  standalone STA and AP physical transactions.** The portable scheduler now
  sees only demands for which the current
  role can produce a valid HT opportunity from association generation,
  power-save state, BA window, PHY rate and aggregate byte limits. It compares
  one recommendation per externally visible transaction head with the
  immutable key of the unchanged SRAM packet. It neither returns a grant nor
  changes queue order.
  The maximum-frame byte estimate is deliberately conservative until demand
  carries exact byte geometry. Current scope is HT class zero/TID 0; every
  unsupported PHY or traffic class is ineligible rather than assigned a fake
  cost. The first same-ELF STA implementation proved exact correspondence but
  cost approximately 1.98 pp of complete Core0 task residence. Returning exact
  role-selected identity in `DatapathTxStart` and adding an O(1) sole-demand
  scheduler path now gives all-exact AP and STA observations, +0.738 pp AP
  Core0 task residence and a 0.73% median STA coarse-image ceiling cost. The
  implementation observes role-selected initial/prepared transaction identity,
  not a guessed FIFO head, and cancels selections which do not publish
  hardware work. Simultaneous two-VIF typed telemetry and a fail-closed HIL
  gate are now implemented. The gate currently rejects the shadow policy:
  production frame-count VIF arbitration disagrees with airtime DRR, and some
  physical owners remain after their software demand lifetime ends. This is
  now proven valid ownership ordering, not a lost mirror update. No
  authoritative cutover is permitted until recommendation moves before SRAM
  claim and admitted work receives its own affine receipt.
- hierarchical VIF then peer/TID weighted airtime DRR;
- AQL-like estimated pending airtime charged at successful SRAM admission;
- completion reconciliation from exact published PHY/length and retry/BA
  results, explicitly labeled as modeled until hardware evidence exists;
- power-save/progressless-peer eligibility and a separate control reserve;
- compare every shadow decision with actual queue/radio progress.

The clean channel-13 coarse run `1788289195983-001756d8` at commit
`ddd9da87688ad4b87b79f05ec8caa738e46e1a25` used build ID
`da5235bb7aa44064ca8218bb54e24959e2bc2ef4b50a8649a40109a85a88eb29` and
application SHA-256
`811a8d846b7f3ad68be7f627255e5aff44e6d525921c44baa4db344629140c3d`.
Its two cycles delivered 119.915 and 119.833 Mbit/s at 38.997% and 39.172%
Core0 occupancy. All 325,762 published frames were classified `exact`; every
unclassified, non-associated, role-unbound, interface, peer-slot,
peer-generation and traffic-class mismatch counter was zero. OpenWrt observed
MCS7/40 MHz with zero retries and failed transmissions in both cycles.

This result proves correspondence only for the saturated, single-peer AP data
path exercised by that scenario. It does not prove reassociation races,
multiple peers, group/control traffic, sparse traffic or authoritative
scheduling. Access-point report schema 6 records the complete `ORC0TXI`
snapshot per cycle and rejects malformed or cycle-count-mismatched diagnostic
evidence, so future runs do not depend on manual UART inspection.

Host/report commit `4b9275a6f1efa3dcf414ce7cd2b6a3bd99d0c41d`
replayed that exact archived firmware in run `1788289688064-00176463`. The
schema-6 report bound 163,297 and 163,521 exact observations to its two cycles,
with every mismatch counter still zero. The replay delivered 120.175 and
120.347 Mbit/s at 39.068% and 39.033% Core0 occupancy. This replay validates
the report extraction and archived-firmware path; it is not a new firmware
performance comparison because the firmware remains the `ddd9da87` artifact.

Commit `93d74474cf4217cc9e2f686d3b05d9441acd1bc6` extended that boundary through
terminal AP A-MPDU ownership. Clean run `1788290889195-0017b1f3` and exact
firmware replay `1788291195847-0017b7d0` each completed two channel-13 cycles.
Every cycle used full 32-frame aggregates except one terminal packet outside
the A-MPDU path: terminal current frame counts were respectively
162,304/163,200 and 162,656/162,976, while exact entry counts were one larger.
All terminal stale counters and every entry mismatch counter were zero.

The same evidence exposes a performance regression which is not accepted as
an architectural cost. The clean run measured 119.444/120.109 Mbit/s at
43.254/43.389% Core0 residence; exact replay measured 119.699/119.939 Mbit/s
at 43.355/43.430%. The prior identity-only image was near 39.0% Core0. The
replay proves this is a property of the new image, not one anomalous radio
sample, but does not identify whether the added cost is the current-generation
lookup/counters or an induced code-layout/API effect. The existing same-image
egress-control switch therefore also suppresses only this terminal diagnostic
observation in its disabled mode; it does not change retry, release, retained
identity or hardware behavior.

That A/B used one ELF from commit `556f743c`: enabled runs
`1788291482425-0017c443` and `1788291801480-0017c85b` averaged 44.946% and
44.955% Core0, while disabled run `1788291698268-0017c6f3` averaged 44.799%.
All three averaged approximately 119.9 Mbit/s. Disabling the generation lookup
and counters therefore recovered only about 0.15 percentage point; it did not
explain the multi-point regression. The additional diagnostic branch itself
also moved the image from approximately 43.4% to 44.9% without materially
changing retired instructions, direct evidence that this diagnostic ELF is
highly sensitive to hot code shape.

Commit `c9321e0a4dd7d271383b137962684c3eaa3a4a95` then removed association and
rate duplication from the hot `AmpduProgress` return value. Clean run
`1788291987639-0017d159` delivered 119.910/120.064 Mbit/s at
42.749/42.808% Core0. This recovered about 2.17 percentage points, but retired
instructions remained approximately 706 million per cycle rather than the
previous 669--670 million. A smaller return ABI was beneficial, but it did
not remove the added work.

Source inspection localized that work to aggregate preparation. The exact
association identity was already bound once by `begin()`, but `push()` copied
and compared the complete 12-byte `(slot, generation, MAC)` identity again for
every MPDU. That check ran approximately 163 thousand times per cycle while
preparing the active and standby BA32 batches. It could not add generation
correctness: all calls used the same batch admission from which `begin()` had
already captured the exact identity. Commit
`12c42c52c2f1982b66701cda1f663be2b8da0f7a` therefore retains exact identity
through the entire aggregate lifetime but restores the per-MPDU check to the
six-byte peer address, whose purpose is only to reject a mixed-peer batch.

Clean channel-13 run `1788292393796-0017e36a` proves the result. Its two
cycles delivered 119.922 and 119.630 Mbit/s at 40.037% and 39.812% Core0.
Radio retired instructions fell to 670.841 and 668.215 million; publish-phase
instructions fell to 111.304 and 108.052 million. Exact entry observations
were 162,949/162,561, terminal current frames were exactly one fewer per
cycle, all aggregates contained 32 MPDUs, and every stale or mismatch counter
was zero. The accepted identity boundary is therefore exact once per affine
aggregate lifetime, with only the minimal peer-consistency check in the
per-MPDU hot path. Shadow airtime work may now resume, but every new accounting
boundary must preserve this normalized Core0 baseline rather than treating a
per-packet identity protocol as an acceptable policy cost.

Commit `9badaf84f843e9b52fc24501255f5e384de93cde` completes the next shadow
boundary without making it authoritative. The AP MAC returns the exact
initial and retained-retry publication vectors to the single Core0 radio
owner. That owner binds the first frame's stable
`(interface, schedule epoch, peer slot, peer generation, traffic class)`
identity once per aggregate, accumulates modeled data-PPDU duration for every
actual publication, and reconciles the association slot/generation at every
terminal completion, timeout or collision path. The accounting state exists
once outside the active/standby DMA arenas; it is not duplicated with physical
storage. Access-point report schema 7 adds an optional `modeled_airtime`
record and preserves `hardware_measurement: unavailable` as parsed provenance,
not explanatory prose.

The clean channel-13 source run `1788294706225-00184f86`, disabled replay
`1788294955903-00185bbb` and enabled replay `1788295065959-00185da3` form one
same-ELF A/B/A. All use build ID
`5968a01c8ae51ae8ed071151830936bba336c2c8e4d4aa913abf941cc8061604`
and application SHA-256
`1bdb8e87c4ba0dd95467ee75d8b22e10ed00aa98165d78144a59ce98722bcd49`.

| Run/mode | throughput | Core0 residence | Core0 cycles/datagram |
| --- | ---: | ---: | ---: |
| enabled A1, two-cycle mean | 119.966 Mbit/s | 39.862% | 12,521.2 |
| disabled B, two-cycle mean | 119.382 Mbit/s | 39.584% | 12,494.8 |
| enabled A2, two-cycle mean | 119.517 Mbit/s | 39.851% | 12,564.7 |
| enabled A1+A2, weighted | 119.742 Mbit/s | 39.856% | 12,542.9 |

The enabled-minus-disabled normalized cost is approximately 48.2 Core0
cycles/datagram, or 0.39%, and 0.27 percentage point of Core0 residence. It is
inside the one-percentage-point gate. Enabled throughput was 0.36 Mbit/s
higher rather than lower, which is treated as run variation rather than an
optimization claim. Core1 TX consume cost was 13,417 cycles/datagram enabled
versus 13,461 disabled, so this shadow did not move work from Core0 to Core1.

Across the four enabled cycles, all 20,338 terminal aggregates were bound to
the current identity and every terminal mismatch was zero. The MAC published
20,369 A-MPDU vectors: the extra 31 retained retries prove that the evidence
does not cover only the no-retry path. Their summed modeled data-PPDU duration
was 550,700,432 × 100 ns. This is approximately 55.070 seconds of submitted
data-PPDU model during 64 seconds of workload; it remains neither measured
airtime nor a complete medium-occupancy estimate. In the disabled replay all
terminal/model counters were zero while the unchanged entry-side identity
mirror remained exact, proving the runtime switch isolates the new boundary.

### Phase 5: authoritative cutover

The portable policy kernel now contains the first host-verified grant
lifecycle, but it is not connected to the cross-core transport or production
admission. A valid selection can become one copyable, serial-bound burst
grant. Issuance charges no airtime. The first successful final-SRAM admission
starts the grant and creates the affine pending-airtime receipt; the grant
remains the sole physical preparation horizon until Core1 closes it once.
An unused grant becomes stale after `Inactive` or a VIF epoch reset, while a
started grant and its receipt survive those transitions until terminal radio
reconciliation.

Grant close carries `used_frames` plus the exact remaining demand level from
Core1. Core0 must not derive that value by subtracting from its earlier demand
snapshot: concurrent enqueue and coalesced demand watermarks make such a
subtraction ambiguous and can produce a duplicate grant. Consequently the
close record and ordinary demand lifecycle must share one ordered
Core1-to-Core0 control stream. The host state tests cover sparse grants,
unrelated demand churn, unused-grant invalidation, epoch reset after start,
receipt tombstones and post-materialization demand installation.

The adapter now also has the non-authoritative transport primitive required
by that model. Core0-to-Core1 grants use a two-entry affine SPSC queue per
logical interface. Core1-to-Core0 demand and `Started`/`Finished` grant
records share the existing bounded ordered queue rather than racing through
independent channels. A full grant queue retains the rejected value at Core0;
when Core1 returns capacity it level-latches a Core0 retry. A grant publication
wakes the permanent network task after the value is visible. Host tests prove
the ordered record sequence and the full/return/retry edge. No production
device consumed these grants at that checkpoint, and no admission outcome had
changed.

The next dependency/API checkpoint pins Xarxa
`bada2fb00646d8e44c5a072bbc96dff29a05db3a` and Embassy
`2d869890bf0f2c38ec6b7b63d3d1261f1b35d44a`. Xarxa now has explicit
`StackSelected`, `Shadow` and `Authoritative` modes. In `Shadow`, the stack
observes one radio grant for a complete interface dispatch round, preserves
its existing key choice, counts only matching final emissions and closes the
grant with `used_frames` plus a freshly reconciled exact remaining demand.
`Authoritative` exists only as a host-tested API/state-machine path; the Wi-Fi
composition does not select it.

The pinned adapter now consumes the Core0-to-Core1 grant transport when the
same-ELF egress-control switch is enabled and advertises only `Shadow`. It
retains the affine grant next to the permanent Core1 device. A matching
`TxToken` records the first `Started` edge after the final SRAM buffer is
written and before its index is published to Core0. Xarxa's terminal callback
then publishes `Finished`; a dropped, unused token cannot create `Started`.
Full progress publication is retained for retry and has a distinct
`grant_progress_full` counter. Until HIL proves both demand and progress-full
counters remain zero, the shared-stream ordering under a prolonged stalled
Core0 is a shadow validity gate rather than production authority.

The next implementation boundary is now present in host-tested shadow form.
After one bounded Core0 demand/progress service turn, the physical radio
policy revalidates role facts, selects one demand and sends a frame/airtime
quantum to Core1. A full grant ring does not reselect: the exact affine value
is retried. Xarxa consumes matching credits locally and the pinned adapter
reports `Started` only after final SRAM materialization and before physical
publication. Exact `Finished { used_frames, remaining }` then closes the
quantum on Core0. Selection therefore precedes stack materialization; the
removed post-factum recommendation path can no longer be mistaken for a
fairness mechanism.

The telemetry schema was cut over with the implementation. It now reports
`grants_issued`, `grants_started`, `grants_finished`, used/unused grants and
progress-without-grant. The obsolete recommendation/actual agreement fields
and qualification criteria were removed rather than aliased. Host tests cover
transport retry, stale serial rejection, unused close after epoch reset and a
started admission receipt surviving reset until close.

This remains a non-authoritative scheduling shadow. `transmit_for` and the
existing stack choice still determine SRAM admission and packet order. The
current shadow also returns modeled pending airtime when Core1 finishes
materializing the grant, not at terminal BA/retry completion; that is adequate
for lifecycle/cost validation but not yet AQL authority. The immediate next
gates are therefore:

- same-ELF disabled/shadow HIL for STA, AP and simultaneous STA+AP;
- prove bootstrap, sparse dispatch and saturated progress with zero demand,
  grant and progress queue overflow;
- measure throughput plus Core0, Core1 and total normalized work per used
  grant and per datagram;
- bind a started grant's admission receipt to terminal radio completion and
  reconcile actual modeled aggregate/retry cost;
- only then enable authoritative `KeyDeferred` behavior and remove shadow
  mode after its rollback gate has passed.

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
- no demand/grant overflow in supported saturation;
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

The role-neutral selected-transaction boundary and its standalone STA/AP cost
gate are complete. The next gate is not an immediate authoritative cutover.
It is simultaneous multi-demand evidence: add paired-role/multi-peer identity
telemetry, then prove sparse latency, saturated fairness, retry/power-save
progress, aggregate depth, service gaps and both-core normalized work with the
fixed SRAM pool. Only after those observations pass may the policy gain an
affine burst/airtime return path and participate before AP standby flow
selection. Before that return path becomes authoritative, modeled terminal
duration must reconcile the exact VIF/schedule-epoch/peer-generation/TID
transaction through retry and terminal release. Any future grant is proactive
and burst/airtime-bounded; no packet-frequency request/reply API may be
reintroduced.
