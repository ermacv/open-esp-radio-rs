# Wi-Fi network integration

This reference describes the implemented network/radio boundary. The radio
owns peer and physical execution state; network adapters own their packet
storage and stack-facing contract. Source support limits are listed in the
[ESP32-S31 IEEE 802.11 feature reference](../driver/chips/esp32s31/ieee80211/FEATURES.md).
Hardware readiness is decided by [qualification](../qualification/README.md).

For the reason each stack is retained, public selection names and current
availability, see [Network implementation choices](network-implementations.md).

## Compositions and owners

| Component | Owns | Does not own |
| --- | --- | --- |
| `driver/network/interface` | Logical interfaces, link state and frame/admission errors | Queues, allocator, executor or hardware |
| `driver/network/adapters/embassy/compat` | Released `embassy-net-driver` tokens and bounded complete-frame staging | Radio peer/BA state or final SRAM slots |
| `driver/network/adapters/embassy/owned` | Owned `PacketBuf` handoff and stack wake registration | Radio scheduling or DMA descriptors |
| `driver/network/adapters/xarxa/upstream` | Original Xarxa driver API, bounded packet-owner queues and link epochs | Packet-pool implementation, IP policy or physical radio state |
| `driver/ieee80211/datapath` | Software/physical ownership traits and selected-burst contracts | Concrete allocator, stack or executor |
| `driver/runtime/embassy/esp32s31/ieee80211` | Physical radio runner, SRAM promotion, completion and executor waits | Application sockets or a second network stack |
| `driver/network/research` | Synchronous bounded protocol engine and selected-work construction | Production network integration or hardware qualification |

The ESP32-S31 product selects exactly one of `upstream-network`, `owned-network`
and `compat-network` at compile time. All compose the same physical radio runner
and the product's finite SRAM TX horizon. The research engine is a separate
library, not a selectable product feature.

The radio owns VIF/peer-generation state, power-save eligibility, rate and
retry state, BA sessions, DMA descriptors, physical credits and terminal TX
receipts. The application retains board startup, credentials, network stack,
DHCP and sockets. Stack APIs do not expose radio peer slots or airtime grants.

## Network dependency contracts

| Integration | Stack-facing contract | Dependency guarantee |
| --- | --- | --- |
| `compat-network` | Released `embassy-net-driver` 0.2.0 RX/TX tokens; `embassy-net` 0.9.1 uses smoltcp | The selected production network graph uses unmodified registry packages |
| `upstream-network` | Original `xarxa-driver::Driver` and global `PacketBuf` pool; the application supplies its stack | Original pinned `embassy-rs/xarxa` by default; the explicit `patched-xarxa` composition replaces only the stack while preserving this driver API |
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
lockfile includes the optional network alternatives; an inactive dependency
can still participate in Cargo resolution. The released-network guarantee
concerns the reachable production graph for `compat-network`, not the absence
of fork entries in a shared lockfile. The ESP32-S31 hardware dependencies on
the pinned `esp-hal` and `esp-pacs` forks remain in all product profiles.

## Original upstream integration

The upstream adapter implements the original [Xarxa driver contract](https://github.com/embassy-rs/xarxa/blob/14c369bbcbe8ee7167488ac9c9e18be059d83555/xarxa-driver/src/lib.rs).
The station example defaults to original Xarxa and uses [Embassy revision c0fdd08e](https://github.com/embassy-rs/embassy/tree/c0fdd08e94138105fba8be3133c4ced91afc30fc/embassy-net),
which itself pins that Xarxa revision. In `upstream-xarxa`, no network source is replaced with a
fork or locally edited copy. The full revisions appear in the manifests and
are checked against Cargo's resolved production graph.

`Esp32s31WifiDevice::into_upstream()` transfers the device to the application.
The application places the driver and `StackStorage`, creates the original
Embassy `Stack`, and calls `add_iface`. IP configuration belongs to that
interface. Upstream supports multiple interfaces in one stack; the radio
adapter does not require a separate IP stack per VIF.

TX queues retain the exact upstream packet owner. The existing radio scheduler
selects the interface and copies the frame into its final SRAM allocation;
the upstream owner is then released. RX copies decapsulated bytes directly
into an upstream packet. Pool placement and capacity are upstream's global
policy, not the maintained fork's separate RX/TX allocators. There is no
end-to-end zero-copy claim.

The adapter notifies the stack when TX queue capacity returns and again when
the retained packet releases its pool slot. Upstream has no public
packet-pool release notification, so RX allocation failure returns
`PoolExhausted` to radio drop accounting. It never leaves a frame waiting on a
nonexistent pool event. The radio RX contract selects `DropFrame` for upstream
pool exhaustion and `WaitForRelease` for pools with release notification; STA,
AP and paired-role batch cursors follow that distinction. The endpoint records
global-pool allocation refusals separately through `rx_pool_drops`.
Each RX drain is bounded by the endpoint queue depth:
when a second core keeps replenishing RX, the device returns `None` and wakes
the next turn so socket consumers can run. Link down discards queued owners;
admitted TX requests keep their original epoch across reconnect.

The pinned original stack has a UDP backpressure limitation: a blocked send
marks Xarxa's `tx_starved`, Embassy wakes its runner after the send attempt,
and Xarxa wakes waiting senders at the next stack poll without first proving
capacity returned. A full queue can therefore cause repeated application/stack
wakes without packet progress. This is not a driver promise to busy-poll.
The pinned Embassy `wait_send_ready` implementation checks only whether the
socket is open; it does not establish queue or packet-pool capacity. Neither
case is worked around in `upstream-xarxa`. The separate `patched-xarxa`
composition corrects device-capacity wakeups inside Xarxa, as described in the
[implementation overview](network-implementations.md#why-keep-original-and-patched-xarxa).

The host backpressure test uses the original stack and production adapter. It
reports wake-driven poll counts while the radio consumer is stopped, then
checks send recovery and quiescence after downstream packet disposal:

```console
cargo test -p open-esp-radio-hil-runner --test upstream_backpressure -- --nocapture
```

The task-poll and MAC IRQ [HIL diagnostics](../hil/targets/esp32s31/README.md)
separate packet admission, executor residence and interrupt service. A high
throughput result does not establish an efficient CPU budget, and excessive
polling alone does not establish the cause of variable radio throughput.

Git Embassy and the registry platform would otherwise introduce two packages
with `links = "embassy-time"`. The station example, HIL target and host integration-test
workspaces explicitly map only upstream `embassy-time-driver` to its unmodified
official crates.io version 0.2.2 through Cargo's `[patch]` source selection.
This unifies the timer ABI; it does not patch Xarxa, Embassy networking or any
library implementation. The reusable upstream driver itself needs no such
mapping. The HIL target selects `upstream-xarxa` by default;
`--network patched-xarxa` selects the explicit stack patch. HIL also composes
`upstream-smoltcp` and `owned-xarxa` through their own driver contracts. Each HIL role
keeps a separate stack and interface, preserving the existing workload and
placement boundaries. `cargo hil run-all` builds these upstream images; archived
runs retain their own exact source and firmware provenance. Checksum-cost
experiments wrap the original driver only inside HIL; ordinary product
capabilities continue to request software checksums.

Released Embassy and Git Embassy documentation describe different interfaces.
Use the [released driver reference](https://docs.embassy.dev/embassy-net-driver/0.2.0/default/trait.Driver.html)
and [released UDP reference](https://docs.embassy.dev/embassy-net/0.9.1/default/udp/struct.UdpSocket.html)
for compat. The owned adapter must match the sources pinned by its manifest,
rather than a moving upstream `main` API.

## Owned TX path

```text
Core1 network stack
    constructs a general-memory PacketBuf
    classifies the Ethernet destination and transport headers once
    publishes into destination/transport queues sharing one owner pool
                         |
Core0 radio runner       v
    selects a destination and claims only its software owners
    validates VIF / peer generation / TID and power-save eligibility
    reserves the complete SRAM batch before consuming selected owners
    copies each selected frame once into fixed DMA-visible storage
    encodes and publishes radio work
    returns SRAM credits after terminal completion
```

General-memory backlog and scarce SRAM execution credits are separate
resources. Selection occurs before SRAM admission. Failed owned submission
returns the original `PacketBuf`; a failed batch reservation removes no source
prefix. The cross-core queue tags owners with the interface link epoch. AP-retained
owners additionally carry the association generation established by radio
admission. Ethernet destination metadata alone grants no association or
security authority.

`TxQueues` in the portable datapath owns shared owner storage and two levels
of queue metadata. The radio selects an Ethernet destination; each take rotates
among its nonempty transport FIFOs. Multiple flows do not create additional
outer destination turns, and they can supply frames to the same aggregate.
`pending_for` and readiness count all frames for the selected destination.
The owned adapter adds cross-core locking and wake registration. Classification
runs on the producer before locking; selection scans metadata, never packets.
A free owner slot always permits a new destination or flow, preserving the
stack's destination-blind `can_transmit()` contract. There is no per-peer or
per-flow payload reservation. Additional flow metadata lives with the CPU-only
endpoint resources; the physical SRAM pool does not grow with flow count.

The owned AP pulls matching aggregate members through `DestinationTxQueues`.
The outer runner does not push unrelated packets into a selected standby.
Waiting for more members observes the selected destination and count; other
queues cannot satisfy or wake that wait. Failed materialization retains source
owners. Power-save and rollback storage remain in the radio; compatibility
sources retain their bounded regrouping arena.

For destination-aware sources, active retention, awake unicast PS backlog and
producer queues share one AP selection cursor. `next_head_after` inspects the
next source destination in address order, wrapping at the end; AP merges it with
retained and awake PS heads after releasing the source lock. A destination
present in several sources gets one turn. Its buffered PS prefix precedes active
retention, which precedes newer source packets. Metadata-slot reuse and
transport-flow count do not create extra destination turns. One bounded source
scan is sufficient; unselected packet owners stay in place. Initial TX and
standby preparation use this same selection boundary. An already selected burst
continues within its existing limits; matching refill cannot claim newer packets
ahead of an awake PS prefix. FIFO compatibility sources merge visible retained
and awake PS heads; their opaque producer FIFO still requires bounded regrouping.

Stale retained association generations release their packet owners before
selection. Selection and matching refill recheck power-save before encoding.
A sleep transition moves retained packets to PS storage and updates TIM.
PM=0 refreshes readiness without reserving a release or removing a queue entry,
including RX during TX and parked STA+AP RX. Selecting the awake destination
acquires its affine release. Publication rechecks the association generation
and awake state; resleep restores the original owner and queue position.
PS-Poll keeps its explicitly requested single-frame release, and DTIM keeps its
group-release policy. These protocol releases remain separate from the common
voluntary destination cursor. Awake PS frames currently use ordinary MPDU
publication. Default selection is destination round-robin. The optional deficit
mode below includes awake buffered heads in its candidate set; it does not
combine buffered PS packets into aggregates.

Active retention, unicast power-save and DTIM group queues share one bounded
radio owner arena. Preparing a power-save release removes its scheduling entry
but keeps the packet in its original arena slot through terminal TX. Rollback
restores only queue metadata; concurrent admission cannot consume that slot.
Successful release frees the slot and original packet owner exactly once.
`AccessPointTxStorage` owns this CPU-only arena outside the movable AP service
future. Each `Esp32s31AccessPointNetworkTx` borrows it exclusively for one AP
epoch; queue indices cannot cross that epoch. Dropping the epoch releases all
remaining retained packets. After physical TX has detached, `into_storage`
returns the same empty storage for reuse. The production supervisor carries
this resource through standalone AP, concurrent STA+AP and fault teardown;
custom compositions do not need static placement. This storage holds software
owners, not DMA frame bytes, and does not increase admission capacity.

For owned TX, `TX_QUEUE_DEPTH` limits all admitted software owners: packets
waiting in adapter queues and packets held by the radio share that admission
budget. Dequeue transfers the credit with the owner; power-save preparation
and rollback keep it occupied. Packet destruction returns the general pool
slot before releasing admission and notifying a blocked producer. Ordinary
SRAM materialization consumes the software owner and releases its admission
while physical credits remain occupied through DMA completion. Power-save
releases keep their software owner and credit until terminal completion so
rollback can restore the same frame. Endpoint resources must outlive every
claimed frame; no static placement is required by the owned adapter.

The production owned composition derives endpoint admission and radio retention
from `AP_SOFTWARE_TX_CAPACITY`. Every admitted owner can therefore move into
power-save storage, even when all admitted traffic targets sleeping peers or
DTIM groups. Active retention and prepared/in-flight PS release tickets occupy
the same radio arena. A rollback needs no fresh storage or admission.

Ingress and radio retain separate metadata/storage for handles; payload ownership
moves between them. The common credit budget bounds their combined occupancy,
so a valid production owned transfer cannot exhaust a second, smaller arena.
There is no per-peer reservation. Custom compositions must keep endpoint
admission within the radio retention capacity to obtain the same guarantee.
Compatibility adapters retain their existing admission contracts.

For sources without that shared admission guarantee, an exhausted retention
arena drops the newly claimed owner. This is an
explicit overload loss after network admission, not producer backpressure.
Attached diagnostic observers distinguish active-queue, unicast power-save
and group power-save capacity losses. A dropped power-save frame is never
added to the advertised TIM count.

`TransportFlow` classifies ordinary Ethernet-II IPv4 and IPv6 TCP/UDP by exact
directional IP addresses, protocol and ports. IPv4 options are respected;
checksums remain the stack's responsibility. Fragmented IPv4 uses one coarse
addresses/protocol FIFO, including the first fragment. IPv6 extension chains
are not traversed and use an addresses/initial-next-header FIFO. Other Ethernet
encapsulations, including VLAN, and malformed IP headers use an opaque FIFO.
These are scheduling hints, not acceptance or security decisions; packet bytes
and ordinary radio rejection remain unchanged. Header boundaries follow
[IPv4](https://www.rfc-editor.org/rfc/rfc791) and
[IPv6](https://www.rfc-editor.org/rfc/rfc8200).

FIFO order is preserved within each classified queue. There is no fragment
cache linking fragmented packets to the corresponding unfragmented transport
flow, so ordering across those classes is not guaranteed. Power-save storage
retains the order selected at source admission; it does not independently
reschedule transport flows during release. Packet round-robin is not byte or
airtime fairness, and a full shared admission budget can still prevent a new
sparse flow from entering. TID remains 0; default board compositions use
destination RR and do not enable the optional airtime scheduler or AQL.
Full aggregates and low error counters do not establish a fairness guarantee.

The portable datapath `airtime::AirtimeScheduler` owns bounded per-peer deficit
accounts separately from packet storage. A key includes association generation,
not transport flow; duplicate candidate keys receive one share. `TxQueues::heads`
borrows one packet per outer queue without dequeueing or rotating transport
flows. Callers supply each eligible peer's minimum useful exchange cost.
`DestinationTxQueues::head_for` carries the next Ethernet length and total
destination backlog across the owned adapter's synchronization boundary. The
snapshot copies only metadata; it does not rotate flows, claim a packet, return
admission credits or change readiness registration. AP geometry admission can
use `Esp32s31ApAmpduBudget::admit_ethernet_len` before borrowing payload bytes.
This snapshot does not reserve the head: teardown, another consumer or
publication can change the queue, so the claimed packet still needs geometry,
association-generation and security validation. Ethernet length alone does not
provide a complete exchange cost or an airtime grant.
Idle/asleep peers earn no credit, lose positive surplus and retain existing debt.
Exhausted rounds advance arithmetically without a timer, retry loop or wakeup.

`AirtimeStorage` owns the bounded CPU metadata. `scheduler()` borrows it
exclusively; the small scheduler handle may move while active and standby
reservations retain the same borrowed storage identity. Their lifetimes prevent
moving or replacing that storage while a token remains usable. Independent
ledgers cannot cancel or settle each other's tokens even when peer keys, local
serial numbers and budgets coincide. This uses references and an identity check,
without allocation, a global ID counter, a lock or `unsafe`.

`reserve` deducts a grant before preparation. Active and standby grants occupy
the same bounded reservation horizon. An unpublished `AirtimeReservation` can
be cancelled; successful publication converts it to `AirtimeInFlight`. Retries
keep that same owner. After terminal detach, `completed(work)` joins the final
receipt and reservation in an `AirtimeCompletion`. Physical and packet storage
can be released independently of the still-unresolved scheduling charge.
The caller supplies publication/detach evidence; these budget methods do not
operate hardware or prove DMA quiescence themselves.

`settle` consumes the completion with an explicit nonzero modelled cost and
returns its receipt for reporting. Errors return both the receipt and reservation;
unknown cost is not converted into zero or an automatic refund. Dropping a token
does not refund it, and reborrowing the same storage preserves outstanding
accounts. Retirement pins an old generation while work remains outstanding;
callers must retire disconnected peers and provision metadata for overlapping
old and new generations. Capacity errors preserve accounts and require a real
release edge. Host tests cover the ledger and real AP runtime publication/completion methods
with model hardware and real software/DMA pools.

AP `Esp32s31AccessPointNetworkTx::new_with_airtime_accounting` optionally binds
an `AccessPointAirtimeStorage` and an explicit `AccessPointAirtimeCost` callback.
The ordinary constructor leaves accounting disabled. Board compositions default
to that constructor; `Esp32s31RadioConfig::with_access_point_airtime` attaches an
explicit standalone-AP model and selection policy. The board retains its ledger
through role transitions and faults; only a successfully prepared fresh AP epoch
resets it. Accounting mode measures the destination already
selected by round-robin, not airtime-based selection: the singleton candidate
funds that selected account; other accounts retain debt but lose unused positive
credit when absent from the candidate set. Balances are not fairness measurements.
Accounting alone limits neither aggregate geometry nor active/standby depth.

`AirtimeStorage::with_observer` attaches an optional synchronous observer of
successful grants, settlements and cancellations. The observation includes the
reserved budget, modelled charge and remaining outstanding reservations. Failed
operations and dropped tokens produce no successful-transition event. The
callback must remain bounded and must not log, await or re-enter the ledger.
The board's `AccessPointAirtimeConfiguration::observer` preserves this binding
when a fresh AP epoch resets storage. Configuration and ledger share stable
board storage; role transitions carry its exclusive reference.

An observed balance is the value after that peer's transaction edge, not a
permanent snapshot: another peer's selection may advance rounds or remove
eligible demand later. Telemetry consumers own cumulative history and must
distinguish association generations, cancellation refunds and charged service.

Selection binds one reservation to the exact flow key, including its association
generation. Retained and destination-addressable source heads reserve before
dequeue; awake buffered traffic reserves before acquiring its release. A FIFO
source exposes its destination only after dequeue, so the owner first requires
space to return the frame if reservation fails. With accounting enabled, lack of
that space returns `SelectionStorageFull` before claiming a source frame.

The successor owns either a selected head or a built standby. Moving the selected
head into ordinary preparation or standby preparation transfers the same grant.
In RR modes an already retained frontier without a selection grant reserves at preparation.
The active exchange and its successor share the two-reservation horizon.

`new_with_airtime_admission` adds a prospective HT A-MPDU ceiling to the
accounting mode. Its `AccessPointBlockAckTiming` callback supplies an explicit
compressed-BlockAck PHY assumption for the reserved peer. The callback must
remain stable throughout that reservation; the terminal cost callback must use
a consistent model. There is no implicit ACK rate or inference from RTS fields.
Unknown response timing returns `UnpricedAggregate` before DMA promotion.

`new_with_airtime_scheduling` combines that HT limit with destination selection
by modelled airtime deficit. It merges current retained heads, awake buffered
heads and destination-addressable producer heads before asking the portable
ledger for one reservation. Each association generation receives one share even
when it has demand in several stores. Its buffered prefix precedes newer retained
and producer frames. Multicast destinations share the group account and rotate
by address within that account. The scan copies bounded metadata only; source
inspection is bounded by the initial backlog and stops before address wrap.

The caller supplies the ledger quantum, a common minimum useful grant and the
terminal cost callback. Selection funds all currently eligible candidates;
published costs, including recorded retries, create debt that affects subsequent
choices. An empty demand snapshot clears unused positive credit. No timer, retry
loop or synthetic wake replenishes deficit. Sleeping or invalid RR frontiers
still enter the existing buffering/rejection path without earning airtime credit.
PS-Poll and DTIM keep protocol priority and their direct accounting path.

Unreserved leftovers from a preceding aggregate return to the retained queues
and compete with other heads. Returning them requires both arena and flow metadata
capacity; failure preserves the frontier and reports `SelectionStorageFull`.
An already selected head or built standby keeps its reservation. This mode also
supports ordinary TX without a standby arena. An opaque FIFO source reports
`DestinationQueuesRequired` at selection before claiming its next frame; the RR
accounting and compatibility paths continue to support FIFO.

This is an explicit model-driven mode, disabled by default in board compositions.
It does not supply per-head physical cost estimates, cap ordinary/A-MSDU/retry
work, or establish measured airtime fairness. The HT response-timing and terminal
cost obligations remain the same as for admission mode.

The initial active pair and every standby build/refill intersect the existing
length accumulator with the byte ceiling derived from their own grant, HT
rate, SIFS and compressed BlockAck timing. Existing peer/radio byte limits,
slot limits, delimiter/padding geometry and security checks still apply. The
budget snapshot can only tighten its limit; refusal leaves the admitted prefix
unchanged. Active completion may change the peer's balance but cannot change
the standby reservation already issued to that peer.

A grant too small for the initial pair leaves an ordinary frontier; it does not
wait for time to replenish credit or retry the same impossible pair. The selected
head keeps that same reservation when falling back to ordinary TX. A later
refused frame stays unencoded at the next exchange's frontier. This ceiling
bounds the modelled first HT aggregate exchange only: ordinary MPDUs, A-MSDU,
MAC retries and contention are outside its admission limit and are charged by
the terminal callback. It does not establish a physical TXOP bound, cover HE,
change destination RR or provide airtime fairness. The HIL RR/deficit comparison
enables the same admission model in both arms; ordinary product startup leaves
it disabled.

The AP network-TX owner reserves before frame preparation, converts the grant
at successful publication, and settles ordinary or aggregate work after terminal
hardware detach/release. Selective retries retain the same grant; standby holds
its own unpublished reservation. Cancelled or rejected preparation refunds only
unpublished work. Standby metadata is retained across partial construction and
failed publication so cancellation can find the physical builder before refund.
PS-Poll and awake unicast releases charge their association generation; DTIM
and other multicast traffic charge a separate group account. Buffered sleeping
backlog has no reservation. Management/control publications are outside this
network-data ledger. Disconnected generations are retired without reassigning
their outstanding charge to a new association.

PS-Poll and DTIM can preempt an unpublished selected head: its grant is cancelled
and its packet remains retained for later selection. They cannot cancel a built
standby's grant through selection cancellation. Standby cancellation releases
the physical builder before refunding its reservation. A peer that sleeps after
selection returns the unpublished grant and moves the packet into power-save
storage.

The cost callback receives the peer and complete `MacTxWork` receipt after
hardware release. `None` returns `UnpricedWork`, keeps the outstanding budget
and exposes the receipt through `unresolved_airtime_work`; it does not admit a
successor. A failed detach keeps published ownership outstanding even when
preparation is cancelled. Dropping the owner does not reconcile or refund that
work; callers must inspect unresolved work before disposing of the owner, and
must not treat a new ledger as settlement of an old hardware transaction.
A fresh accounting domain requires fresh caller-owned storage. This API does
not provide a default full-exchange cost model or establish measured airtime fairness.

`MacTxWork` separates committed publication work from delivery and elapsed
time. Both retained A-MPDU storage and ordinary `TxSlot` keep the receipt in
pinned CPU metadata, outside movable completion states. Each publication uses
its own PSDU length and rate; selective retry compaction and ordinary rate
fallback therefore charge their actual submitted geometry. Preparation failures
add nothing. Abort/detach preserve the receipt and do not attest on-air delivery.
An ordinary exchange resets its receipt after admission validation, before its
first publication; a successful aggregate `begin` resets its receipt. Read before
starting the next exchange. A failed first publication leaves an empty receipt.
Ordinary direct slot users explicitly call `reset_work` while free to establish
an exchange boundary; reservation alone does not reset a retry's accumulated work.

STA exposes `ordinary_work` and `aggregate_work`; the AP ordinary MAC and
aggregate owner each expose `work`. Every ordinary publication through `TxSlot`
is accounted, including control, group and management frames. The optional
runtime observer covers ordinary transactions serviced through connected STA
and AP network data, including buffered/group release. Autonomous hardware ACKs,
BlockAck replies and AP management/control outside that observation path are not
part of this HIL bank. A single MPDU detached from an aggregate contributes to
the ordinary receipt only; summing the two banks does not charge it twice.

`nominal_data_micros` retains the simple bitrate estimate for comparison.
`ppdu_micros` includes the PHY preamble, SERVICE/tail bits, whole-symbol rounding
and the 2.4-GHz signal extension. HT uses the exact bits/symbol table, avoiding
rounded nominal SGI rates. Supported timing is DSSS/CCK, ERP-OFDM and HT mixed
format with one spatial stream, BCC and no STBC. HE still contributes to
`unestimated_ppdus`: its LDPC/BCC padding, pre-FEC padding and packet extension
are not replaced by an HT approximation. The wire-byte receipt does include
HE S-MPDU delimiter, alignment and FCS through the existing APEP geometry.
A nonzero unknown count makes `ppdu_micros` a partial total; saturation also
makes totals lower bounds. Neither is a complete scheduling charge.

The receipt also sums programmed `aifs_slots` (AIFSN) and selected
`backoff_slots` for each committed publication. `unreported_contention` counts
publications without those facts; a known zero selection does not increment it.
`TxContention` carries these inputs independently of BSS timing; `TxAccess`
combines them with an explicit slot duration for a planned cost. These are
programming facts, not observed channel waiting. In particular, SIFS is not
included in `aifs_slots`, CCA may freeze a countdown, and trigger/medium ownership
can affect whether programmed contention is exercised. Do not multiply a receipt
spanning different BSS timing configurations by one assumed slot duration.
The S31 selector is `EdcaBackoffState::select_slot`; despite the configuration
field name `contention_window`, the published value is the selected count,
not the window maximum. Retries retain their independently selected counts.

The portable `tx_cost` model accepts PSDU geometry, PHY timing and explicit
response/protection/access facts. It separates data PPDU, SIFS plus ACK or
BlockAck, CTS-to-self or RTS/CTS, and AIFS plus selected backoff slots. A failed
response can supply an explicit timeout budget. No response is an explicit zero;
an unknown response rate or absent access facts remain `None`. Overflow prevents
a total instead of wrapping into a small charge. `exchange_micros` excludes
contention; `service_micros` adds selected contention but excludes CCA freezes,
CPU service and hidden hardware retries. A timeout budget is waiting time, not
transmitted energy or observed channel occupation.

Response basic-rate selection must come from the peer/BSS contract or measurement.
The published RTS-rate field alone does not establish ACK/BlockAck rate. The
model's RTS/CTS option predicts an explicitly described exchange; it does not
enable the currently unsupported hardware protection path. The production
receipt currently accumulates the supported PPDU component, not an assumed
complete exchange charge. `MacTxStatus::airtime_micros` remains unavailable.
No live scheduling or contention policy consumes these estimates yet.

`MacTxWork::estimated_exchange_micros` adds an explicitly supplied uniform
response/protection overhead to every publication, including software retries.
It rejects empty, unknown-PPDU, saturated and overflowing receipts. The caller
must establish that the overhead model applies to every attempt; this helper
does not infer peer response PHY, success versus timeout, or hidden MAC retries.

`PpduTiming::maximum_psdu_bytes` inverts the duration model with integer
arithmetic. `TxCost::maximum_psdu_bytes` first reserves the explicitly expected
response and protection overhead. An unknown component or a budget too small
for any nonempty PSDU returns `None`; it never becomes an unrestricted grant.
The result includes FCS, delimiters and padding, not just Ethernet payload.

For S31, `HtRate::ampdu_exchange_byte_limit` covers one unprotected HT aggregate
and a compressed BlockAck with caller-supplied expected PHY timing. It intersects
the calculated length with the original peer byte ceiling and recovered vendor
rate limit. Feed the result to the existing free aggregate owner's
`configure_max_aggregate_bytes`; its ordinary admission checks still enforce
frame count, DMA capacity and aggregate geometry remain authoritative.
The returned byte bound alone does not prove that two real MPDUs fit.
Recompute for each grant or changed rate/peer, using the negotiated peer ceiling,
not the previous grant's clipped length. A rejected grant means defer or acquire
more budget, not silently bypass the budget through ordinary MPDU fallback.

The AP's `length_budget` snapshots a free/building HT prefix using the existing
length accumulator, intersecting its configured and rate-dependent ceilings.
Active and standby preparation check candidate Ethernet lengths before DMA
promotion and before consuming sequence numbers or CCMP PNs. A frame that does
not fit stays ahead of later frames. Standby retains it in the existing
unencoded next-frame frontier; that owner prevents further pulls until the
batch is published, without a separate full flag. Its diagnostic stop is
`CapacityLimit`. Failed burst materialization restores the entire selected
prefix ahead of that frontier, preserving order. When a
fresh pair exceeds the current geometry limit, both owners are preserved and
ordinary transmission remains available. This is geometry fallback under the
current packet scheduler, not permission to bypass a future airtime grant.
The length snapshot reserves no DMA and does not replace peer/key/backing checks.

These are planned single-publication exchange bounds. They exclude AIFS/backoff,
CCA freezes and executor/CPU residence; they do not limit a complete retry
transaction, enable hardware protection or acquire a multi-PPDU TXOP. A future
integration must separately settle committed retry work and validate the applicable
PHY/publication constraints. No live airtime fairness is enabled by these
helpers, and no new queue or DMA ownership layer is introduced.

Response timing remains an explicit assumption. The current AP advertisement
marks only CCK rates basic; the generic highest-basic-rate rule must not be
blindly reused as a complete HT BlockAck policy. The separate control-response
selection in [ns-3's station manager](https://github.com/nsnam/ns-3-dev-git/blob/master/src/wifi/model/wifi-remote-station-manager.cc)
is a reference model, not evidence of the rate actually used by our peer.

S31 `TxCompletion` exposes completion class, ACK SNR and trigger-flow packet
counts. The trigger counts are not ordinary retry counts; the reviewed
completion contract does not expose response PHY or measured airtime. AP
`data_tx` observation already exports software ACK-timeout, CTS-timeout and
collision republications through the typed HIL Stop event. Its scope is AP-epoch
unicast data, whereas `OTXW` uses terminal transactions within the observation
window and also includes group data. Their counts are not interchangeable.
Local ACK-rate register initialization describes ESP-generated responses, not
responses received from a peer. `HeRate::checked_maximum_apep_bytes` is a vendor
TXOP admission bound with estimated BlockAck overhead; it is not an exact HE
PPDU-duration implementation. Neither source supplies the missing measurements.

The PHY formulas follow the BCC symbol construction and signal-extension
boundaries in [ns-3 HT PHY](https://github.com/nsnam/ns-3-dev-git/blob/master/src/wifi/model/ht/ht-phy.cc).
[mac80211 airtime estimation](https://github.com/torvalds/linux/blob/master/net/mac80211/airtime.c)
provides a separate reference for rate-based scheduling estimates; estimates
must retain their scope independently of measured completion residence.

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
