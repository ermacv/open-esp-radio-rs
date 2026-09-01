# ESP32-S31 Bluetooth LE feature frontier

This document describes the source-owned production boundary. It is not a
Bluetooth Core, RF, HIL or product-qualification claim. A register setter,
validation probe, isolated vendor comparison or disconnected state-machine
component is not a live controller capability unless the production owner
graph reaches the physical Bluetooth boundary.

The first controller target is Bluetooth Low Energy only. Bluetooth Classic
BR/EDR, Mesh profiles and LE Audio profiles are separate programs and are not
implied by progress on the LE Controller. The Host contract is the released
`trouble-host` 0.8.0 / `bt-hci` 0.10.1 pair. The in-process transport implements
the standard `bt-hci` packet traits directly; the Controller does not maintain
a second Host protocol. The upstream legacy Transmitter Test v1 type is not
used because its 0.10.1 opcode collides with LE Read Supported States. The
Controller still accepts the standard v1 opcode through its semantic decoder,
while production smoke coverage uses the correct upstream v2 types.

The status terms are:

- **LIVE**: a bounded production path owns the complete named operation;
- **PARTIAL**: useful production ownership exists, but the named operation is
  incomplete;
- **FAIL-CLOSED**: a typed production path deliberately stops before an
  unverified mutation or readiness claim;
- **ABSENT**: no production protocol owner exists for the operation.

Qualification remains independently controlled by the ESP32-S31 Bluetooth LE
qualification manifest and dated HIL records. The pinned vendor-lifecycle
classification and the boundary between silicon requirements and replaceable
Controller software are recorded in
[`verification/vendor/targets/esp32s31/analysis/bluetooth-controller-boundary.md`](../../../../verification/vendor/targets/esp32s31/analysis/bluetooth-controller-boundary.md).

## Hardware lifecycle

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Cold radio ownership | LIVE | One affine `BluetoothStopped<P>` binds the platform lease to the protocol-neutral `RadioHardware` root before any Bluetooth lifecycle step. Clock enable, rollback and clean clock shutdown all return that same aggregate; production cannot split the pair or use the old independent-resource entry. Opaque HAL task/IRQ owners preserve the lower affinity: only an untouched task owner and pristine interrupt history can reconstruct the neutral root; mutable controller or shared-PHY access arms fail-stop reunion. This does not support concurrent Wi-Fi. |
| Bluetooth platform lease | LIVE | The role-neutral ESP-HAL coordinator privately owns the remaining upstream system singletons and reference-counts the shared PLL source. The affine custom PAC owns MODEM shared-clock leases and baseline restoration. The lease reads the factory base identity through ESP-HAL's safe eFuse accessor, applies S31's reviewed second-universal-address policy and returns a canonical typed address; it also derives the common-PHY calibration identity from the same eFuse owner and the source-owned RF calibration version. The generic HCI layer alone converts the address to `BD_ADDR` wire order. Wi-Fi has not migrated to this coordinator. |
| Controller clock/reset prerequisite | LIVE | The standalone main-XTAL/100-kHz low-power timer profile has semantic read-back and reverse-order rollback before controller MMIO. |
| Controller initialization | LIVE | `start_esp32s31_bluetooth` validates value-only inputs, the Embassy timebase and HCI capacity before reserving permanent state. It then reserves the final slot and claims the BLE-PHY, DTM and legacy-advertising SRAM graphs before the first MMIO, drives the recovered order from clock/reset through HAL, scheduler, bounded HCI, source-127 hardware, common PHY and internally derived initial tracking, BTBB, BLE PHY, Controller output and runtime timer, anchors the first retry deadline relative to that completed cold start, and publishes stable interrupt owners. The terminal owner is placed without a lossy second initialization attempt, split exactly once and returned as a standard `bt_hci::ExternalController` Host facade plus one eternal hardware runner. Every failure retains its exact affine frontier, claimed memory and final slot; successful output also carries value-only PHY/BTBB/BLE reports and a persistence-safe calibration snapshot. The operation is deliberately non-cancellable after hardware mutation because powered rollback is not recovered. Physical HIL, Link Layer/ACL, sleep-enabled RF wake, complete descriptor semantics and powered teardown remain separate gaps. |
| Controller HAL initialization | PARTIAL | The complete 50-operation BTDM HAL body has a typed config (`8/16`, `500/1000/2000`, two positional bytes and validated controller SRAM), exact caller-derived standalone outputs (`3`, `22`, `66`, `0x2f000000`) and ordered host regression coverage. Selecting the reviewed standalone profile immediately after clock/reset mints one private affine zero-sized always-awake policy marker and retains it through scheduler, HCI, PHY, BTBB, BLE-PHY and pre-route interrupt-owner publication. The marker performs no MMIO and is neither RF-ready nor controller-time authority. The HAL transition still establishes no scheduler-list, interrupt, PHY, BTBB, Link-Layer or HCI readiness. |
| Common PHY registration | PARTIAL | The target-only async registration transition borrows the outer Controller's platform and shared-PHY HAL for one concrete `register_chipv7_phy` run. Only terminal target success mints `RegisteredBluetoothPhy`, retained by `BluetoothControllerPhyRegistered`; registration failure retains the outer Controller in fail-stop state. Bluetooth-client acquisition is a separate affine edge: it either settles immediately or retains pending/in-flight tracking until the concrete target tracking runner returns a settled `RegisteredBluetoothPhyClient`. Tracking policy is private and projected from the registered `PhyState`; application code cannot supply a second vendor-parameter model, and diagnostic-print policy is not part of hardware completion identity. Production delay and PLL time use the board's checked one-megahertz Embassy timebase without narrowing. A terminal tracking error retains a poisoned lower owner and the outer Controller fail-stop; cancelling the in-flight future instead drops the unique epoch and requires out-of-band hardware reset. The nested always-awake marker records the source-owned profile choice only; neither it nor client acquisition proves current RF state, warm wake, a per-event RF-ready instant, periodic tracking runtime, last-owner release, operational BLE radio readiness or HIL. |
| Bluetooth baseband initialization | PARTIAL | Only `BluetoothControllerPhyInitialized` retaining a settled `RegisteredBluetoothPhyClient` exposes `initialize_baseband`; registered-only, pending, tracking and poisoned states cannot reach BTBB. The transition projects the required byte from the retained registered PHY state and executes only the finite reviewed `bt_bb_v2_init_cmplx(1)` PAC/HAL transaction. `BluetoothControllerBasebandInitialized` keeps the settled client and every earlier Controller owner nested, but does not prove complete common-PHY/baseband operation, BLE radio-engine readiness, teardown, a composed vendor trace or physical qualification. |
| BLE PHY register initialization | PARTIAL | `BluetoothControllerBasebandInitialized::initialize_ble_phy_engine` consumes an address-bound `BluetoothBlePhyEngineCpuOwned`, derives both pointers from its pinned static allocation and retains that owner in `BluetoothControllerBlePhyEngineInitialized`. The reviewed task-enable transaction uses only PAC accessors. Production integration owns the unique `.dma.bss` arena and the later final composition binds all three stable live IRQ routes while retaining that storage. Optional BQB state, remaining environment-word semantics, packet-engine behavior and physical qualification remain missing, so this is hardware initialization rather than operational BLE. |
| Powered teardown | ABSENT | Opaque HAL owners retain task and interrupt affinity fail-stop, but no scheduler-stop capability is exposed. Review of complete `r_sym_bt_74l62ZLsZuXg67pPHSd7` proved that the former disconnected leaf was not a valid prefix: the vendor first observes BUSY, and on the busy path masks dynamic IRQ groups, disables the synchronous BTMAC scheduler-run source and waits on a distinct command-readiness predicate before publishing lifecycle image `1`. The incomplete leaf and its synthetic comparison probe were removed rather than retained as legacy API. A replacement must compose that full preamble with packet/bottom-half quiescence, IRQ-output release, BTBB/PHY/clock teardown and cold reconstruction. |

DTM admission ownership is now narrower than the table's historical summary:
the published task service is the sole owner of the bounded Timeline and its
exclusive scheduler-list identity. The final split also retains the unique
mutable borrow of one composition-owned DTM runtime, so graph checkout and the
default-power profile cannot be cross-wired with another Controller task. The
production claim binds allocator configuration into the graph itself; checkout
is exclusive and restore rejects any graph from another exact pinned storage
identity, even when a host model uses the same SRAM range. Initial TX/RX
link-state reset is constructed privately from that physical power request and
the typed role rather than accepted from an upper layer. Its preparation
operations never expose the Timeline itself; HCI and interrupt endpoints cannot reserve or release
slots, callers cannot build a raw epoch or event plan, and
`BluetoothControllerTimeSample` is crate-private. A private authority minted
only while splitting the terminal powered BLE-PHY owner lets that same task
service complete standalone post-enable timing requests; the resulting token
is opaque, non-`Copy` and never accepted as a detached scheduler instant. It
does not claim RF wake or analog readiness. Initial TX/RX consume current
before post-enable timing, recurring RX consumes post-enable timing before
current, and recurring TX consumes current only.
Initial TX/RX admission and later sequence authorization are distinct affine
latch requests separated by the actual overlap-resolving reservation.
Recurring TX/RX has a distinct exact-window reservation phase and performs only
the later private sequence request; it cannot fabricate or accept an initial
admission sample. An occupied recurring collision fails closed without
displacement until the vendor removal policy has a reviewed affine model.
Distinct initial and recurring RX-window types select the matching memory-codec
phase; the first-event edge cannot accept a recurring window.

## Interrupts, timing and packet storage

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Controller interrupt output | PARTIAL | Opaque HAL setup/prepared/register/after-routes owners wrap the restricted PAC, which consumes inactive IRQ ownership, clears and enables the exact primary baseline groups (`0x00008000`, `0x00001300`), publishes the setup strobe at `0x2010_100c`, and reverses the two release strobes before masking those groups. The owners publish once into stable shared-primary/NRT and source-127 storage; integration places the complete chip/Embassy dispatcher before activating the three CPU routes. The after-routes owner still has no public release edge. |
| Interrupt observation | PARTIAL | Source 124 now executes the complete bounded hardware order `acknowledge -> reference-gate read -> conditional SCHEDULER_REFERENCE clear/fence -> fresh work read` and durably publishes one paired scheduler/lock-modify event. Vendor selector 6 performs no hardware action; it only validates members of a private intrusive transaction container that the affine open DTM scheduler does not create. Source 127 and default-profile NRT have finite typed dispositions. The final runtime splits source 127 into a disjoint modem-timer task which exclusively borrows its mutable queue and positional epoch, exchanges the affine HAL owner only through stable ISR storage, and exposes owner-free borrowed readiness plus finite begin/step/rearm operations. Fixed adapter-owned handlers enter the complete chip service and publish matching Embassy wakes; fatal storage loss quarantines the asserted route. The final hardware runner services command and timer work fairly under strict sticky-IRQ priority, and every terminal path attempts to disable all three routes before retaining its exact owner. Feature-specific NRT policy, unrelated-list dispatch and a real modem-timer expiration consumer remain missing. |
| Interrupt masks and CPU route | PARTIAL | Primary source 124, modem timer source 127 and NRT source 133 have fixed adapter-owned level-three handlers, stable process-wide owner slots and one affine same-core route epoch. The complete dispatcher is stable before route activation, handler roles cannot be exchanged, and a fatal service result quarantines its route. Dynamic masks, finished-list transfer and current-list observation are typed. Scanner selector 4 and feature-specific NRT policy belong to later roles, not first DTM. |
| Controller timer/scheduler | FAIL-CLOSED | The typed path already covers controller time, sole-item admission, full descriptor ordering, list-zero HEAD publication, dynamic interrupt preparation, RUN, finished-list observation, fenced head retirement, atomic unlink/mailbox arm and TX/RX recycle. A fresh STATUS/REPORT finished-list transfer consumes exactly one non-copyable scheduler batch dequeued from the source-124 handoff; both ordinary and marked work admit it, a retained multi-list capture continues without repeated MMIO, and an empty wake preserves the complete running owner. The bounded timeline retains only protocol-neutral raw windows and common timing policy; a private DTM envelope separately owns its reviewed item transform and projection epoch. Its hardware-next SRAM field is now terminated and rolled back only through a private complete-word codec; no mask or successor image reaches scheduler/session code, and post-publication recycle requires both completion and software-list-removal proofs. Post-unlink readiness is sticky through first publication and mailbox-full coalescing; the Embassy adapter registers before rechecking it and cancellation cannot consume it. Its separate fallback is a read-only direct recheck, not speculative finished-list REPORT polling. Selector 6 is not an open-runtime blocker because its vendor callback only inspects the vendor transaction container. The final runner owns retry waits through a cancellation-safe absolute deadline and keeps source-127 service live while a retry is gated. Remaining blockers are the exact raw completion-interrupt/REPORT semantics, unrelated-list and timer-expiration dispatch, complete on-air descriptor semantics and powered teardown. |
| Controller memory lists | FAIL-CLOSED | Three positional global RX selectors remain restricted to PAC accessors; the memory layer owns their scanner/non-scanner routing, while DTM kind 5 bypasses them and uses its private graph. Static binding installs full private software-list pointers separately from compressed hardware links. TX preparation copies the complete declared packet, and role-specific plans install the reviewed link-state/scheduler words plus scheduler bookkeeping. The exact affine PAC head token consumes rollback authority into pinned `HeadPublished`; the matching RUN proof alone advances it to `Running`. Scheduler items, link-state, both RX headers and hardware-written RX packet words use volatile semantic accessors, and only `Running` can accept a matching fenced list token to classify completion without granting CPU ownership. Fresh typed head observation, software unlink and removal-ready proofs authorize cleanup. TX and RX nonzero-status paths return directly to CPU ownership. The zero-status RX transaction validates the exact two-header capacity-one topology before mutation and binds the typed result to its sole graph owner. Controller composition consumes that result into the same non-copyable receiver session before the owner can commit the reviewed swap/re-arm rotation, and exposes it only after timeline and source-list release. The current blob has no separate software DTM pointer latch. |
| In-process HCI handoff | LIVE | One affine aggregate owns the transport queues and bootstrap state, rejects advertised ACL profiles larger than its static storage and starts only as a pristine HCI epoch. After scheduler initialization it is consumed into the same powered Controller owner; the final split provides matching finite ISR/scheduler-task endpoints plus a `bt-hci::ExternalController` Host transport and one combined Controller transport/bootstrap command endpoint. That endpoint can claim its initial `CommandReady` authority exactly once; dropping endpoints, resplitting the resources or dropping the token cannot mint another authority, and every later authority is returned only by successful same-epoch response publication. Both bounded directions validate complete packets, apply async backpressure, wake on capacity/data, retain packets across short buffers and leave queues unchanged when waits are cancelled. Packet kinds remain typed; no UART/H4 framing or allocator is used. |
| Packet TX/RX | PARTIAL | DTM owns static TX/RX storage through typed descriptor preparation, scheduler HEAD/RUN publication, finished-list completion, volatile result observation, unlink, recycle and RX append/re-arm. The accepted RX result high byte is now a signed typed RSSI value, proven by the current vendor getter; its physical unit/calibration and the low-24 failure meanings remain unresolved. Remaining hardware-consumed descriptor fields, CRC/length semantics, unrelated-list routing and on-air evidence keep the general BLE dataplane incomplete. |

## LE Controller and HCI

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Direct Test Mode | PARTIAL | The S31 TX/RX/Test-End command bodies, channel/PHY/pattern lowering, interval arithmetic, bounded scheduler path, static graph, wake-gated completion drain, recycle and Reset/Test-End response order are represented by affine finite transitions. Controller-side Receiver/Transmitter Test v1 and v2 normalize into one role token each while retaining the exact ingress opcode through RUN, failure, busy rejection and response backpressure. V2 opens the reviewed 2M, generic Coded RX and Coded S=8/S=2 TX projections without version-specific runners; the receiver modulation-index assumption is validated but requires no S31 descriptor field. The private memory codec now checks the complete `scheduler item -> link state -> selected header -> bound packet/PDU` address chain before the builder and exposes no standalone packet/header image escape. Production cold start returns the target Embassy actor's sole hardware runner with all live ISR/waker routes including source 127, absolute-deadline retries and the `bt-hci` Host facade. The standalone ESP32-S31 example issues typed `bt-hci` v2 Receiver/Transmitter Test commands, performs real board/executor composition and concurrently polls the Host read side and hardware runner. Successful hardware `RUN` remains the only edge to a successful start response; terminal or unsupported observations retain their exact owners after full-route quarantine. Remaining hardware-consumed graph semantics, exact finished-list interrupt/REPORT semantics, sleep-enabled RF wake and on-air/HIL evidence keep the capability partial. Nonzero finished lists and timer expirations remain fail-stop until a real LL role owns them; they are not list-zero DTM prerequisites. |
| Legacy advertising | RECURRENCE LOGIC CLOSED | A hardware-independent `open-esp-radio-bluetooth-ll` foundation validates and encodes exact `ADV_NONCONN_IND` PDUs, retains public/random TxAdd semantics and advances an affine non-connectable lifecycle only after one exact generation/event completion. It now supports an internally owned 31-byte value for long-lived async sessions without making the actor self-referential, while retaining a checked borrowed form for immediate callers. The selected non-empty 37/38/39 map is one ordered backend event, matching the reviewed hardware rather than three executor-driven submissions. The S31 chip boundary consumes a pinned physical-SRAM graph, installs the PDU directly in its role-neutral TX allocation and closes the reviewed chain `packet -> TX header -> link state <-> 1..=3 scheduler items -> separate common scheduler context`; the restricted RX chain remains absent. A private codec consumes semantic dBm, PDU and channel-plan inputs, applies the reviewed LE 1M/no-RX/no-CTE/no-privacy reset, links contiguous per-channel item windows before `RUN`, and retains per-item completion statuses as diagnostics. Production cold start now claims that dedicated graph before MMIO and retains it in the sole hardware task service. The common timeline reserves the whole chain, publication mints `InFlight`, and recycle waits for every active item before returning one completed LL event plus the reusable CPU graph. Completion retains the nominal phase, accepts a fresh source-owned 0--10 ms delay, rebuilds the graph, projects the whole next chain, takes an exact non-displaced recurring reservation and rejoins the same publication/completion path without executor waiting. The generic HCI crate decodes standard LE Set Advertising Parameters/Data/Enable through `bt-hci`; Parameters and Data pass through the production affine command-order router into one Reset-scoped configuration owner. Enable now freezes parameters, data and the resolved public/random address while retaining HCI response order, and S31 has an explicit minimum-requested-interval HCI-to-LL projection. Until the production timing/graph actor consumes that deferred start, the S31 route recovers the unchanged idle task and returns Hardware Failure; it cannot manufacture Success. Unsupported advertising roles and invalid values fail closed without changing configuration. No raw SRAM link, field mask, rounded-power or frequency image escapes the memory crate. Production actor/time-observation composition, physical Enable start, response-capable mode and on-air evidence remain absent. |
| Scanning | LOWER SCHEDULER LIFECYCLE CLOSED | The restricted passive LE 1M graph owns two RX nodes, one link state, one scheduler context and three scheduler items in pinned Controller SRAM. Its selected item detaches from the private free chain through a cancellable CPU-only edge, then joins the same exclusive list-zero epoch used by DTM and advertising. Before the first irreversible write the Controller validates the exact scheduler identity and typed head encoding; the infallible suffix publishes selector-one RX memory, the fixed standard-backoff scan command, the common scheduler head, dynamic interrupts and RUN. Completion now consumes the common fenced finished-list drain, exact head retirement and serialized post-unlink mailbox/removal gate before any RX access. The memory graph rejects stale producer/epoch sentinels and non-contiguous completion, copies only the bounded on-air PDU plus signed RSSI, and restores both private lists for reuse. The vendor dynamic-manager epoch is intentionally replaced by the stronger sole static affine graph epoch. No upper layer constructs a register image or SRAM word. Common timeline admission, a production scanner resource/runner, portable PDU parsing, duplicate filtering and standard `bt-hci` scan commands/events remain missing, so this is not yet an operational scanner. |
| Peripheral connection | ABSENT | Connection acceptance, anchors, channel selection, SN/NESN, retransmission, supervision and LL control are absent. |
| Central connection | ABSENT | Initiating and central connection scheduling are absent. |
| ACL flow control | ABSENT | No Host/Controller credits, fragmentation boundary, completed-packet event or controller ACL queue exists. |
| Link encryption and privacy | ABSENT | No Link Layer AES-CCM epoch, packet counters, LTK procedure or semantic resolving-list lifecycle exists. The opaque storage retained by BLE PHY initialization proves only address stability and extent; it does not implement privacy. |
| Extended BLE features | ABSENT | DTM v2 can select the reviewed 2M/Coded test modes, but no 2M/Coded Link-Layer role exists. Data Length Extension, extended/periodic advertising, PAwR, subrating, direction finding and ISO are outside the initial slice. |
| HCI bootstrap state machine | LIVE | A pure closed table implements Reset, base/LE event masks, LE buffer/list-size reports, public/random address handoff, Host buffer/flow-control configuration and a zero optional-LE-feature report. Reset opens a fresh configuration epoch and also clears the adjacent legacy advertising configuration owner; malformed known commands do not mutate either. Event Mask Page 2, scanning and connection commands remain unclaimed for future outer Link-Layer routers; advertising Enable is owned by the adjacent advertising router. The standalone bootstrap-only test harness rejects all Link-Layer commands fail-closed. Reported filter-list size is zero. |
| Typed HCI Controller | PARTIAL | The in-process transport exposes a disjoint Host transport and one combined Controller command endpoint through `bt-hci::ExternalController`; mutable bootstrap state cannot be separated from Controller command authority. Its initial `CommandReady` claim is one-shot for the complete resource lifetime, including endpoint drop and resplit. The raw Controller transport is private. Finite intake consumes the unique authority and returns either an opaque authority-bound owned classification or the unchanged authority with its retry state; chip idle and active owners route that opaque value immediately instead of accepting a separately supplied classification. Portable RX/TX deferred starts expose a closed success or Hardware Failure domain but do not claim a hardware fact. The ESP32-S31 composition supplies that proof: RUN consumes a start into Success, while only a recovered idle task plus chip-classified finite rejection can reach Hardware Failure. Neutral cancellation and shutdown owners expose no response edge, and chip policy presents the adapter only a closed response-pending or fail-stop outcome. Immediate responses and Reset carry the same authority directly. There is no public constructor for a generic response-pending state. Queue full, validation failure and endpoint mismatch retain both ordering and radio ownership, while successful publication returns the only authority that can admit the next same-epoch command. Response-capacity waits, affinity checks, Reset completion and exact-once publication all use the combined endpoint, so no chip or adapter path can bypass command order through a raw transport handle. The S31 layer encloses returned authority with its task owner as an opaque `IdleCommandTask`; successful idle or active Test End and Reset publication returns that aggregate rather than a bare task. Host tests cover portable cancellation/empty intake, command FIFO, cross-epoch rejection, full-queue retry, recovered Hardware Failure, command-actor transition policy and ordered start-to-Test-End publication; they do not execute the target-only hardware actor path. Production actor and board wiring are live; Link Layer/ACL semantics, typed Host-side RX/TX Test commands and all on-air/HIL evidence remain absent. |
| Trouble Host integration | ABSENT | A real Trouble 0.7 Runner completes its no-security software bootstrap through the production Host transport and combined Controller command endpoint in host tests. A post-initialization filter-list command is observed and rejected fail-closed. The ESP32-S31 Controller session runner and hardware path now exist independently, but no composed production Trouble Runner task, LL router or GATT example exists. |

Neutral pre-`RUN` shutdown cancellation is terminal once prepared-graph
cancellation is rejected or scheduler `HEAD` is published. Both paths retain
the exact runner inside one opaque fail-stop owner exposing only reason and
role; no runnable, decomposition or HCI-response edge exists until powered
quiescence is implemented.

The current DTM frontier includes exact initial and recurring TX/RX timing,
phase-specific descriptor configuration and affine Active TX/RX aggregates
that retain immutable command inputs and the last committed window across
recycle. A prepublication merge is phase-tagged: cancelling an initial merge
returns its fresh graph/session ownership, while cancelling a recurring merge
returns the exact Active owner. Either successful cancellation releases the
exclusive list and private timeline reservation, and a merge cannot cross the
initial/recurring cancellation boundary. These transitions are now composed by
the production command actor and finite event runners. The first post-enable controller sample
initializes the persistent scheduler epoch, and each owning acquisition
reanchors that epoch before one preparation consumes its private current. The
same movable post-split task service now privately acquires post-enable timing, admission and
sequence in their exact source order, publishes the resulting head and owns the
existing RUN-through-recycle suffix. The reviewed standalone margin is retained
by the source-owned scheduler policy, while the completed powered BLE-PHY plus
always-awake time request supplies a non-detachable timing result without an RF-readiness claim. The first-event
runner sequences semantic HCI TX/RX v1 through this complete bounded prefix and
retains the resulting active owner through later response backpressure. An
executor-neutral active-completion runner now joins that owner to the complete
finished-list, retirement, unlink, mailbox and role-specific recycle suffix.
The reclaimed boundary enters a second finite runner which follows the exact
role-specific recurring preparation order back to hardware RUN and supports
lossless cancellation before HEAD. A long-lived affine session carries every
completion and recurring phase beside a separately typed response-order proof,
preserving radio progress during HCI backpressure. The Embassy Controller actor
also retains a rejected first-event HEAD or RUN transition in place and exposes
a retry boundary; it no longer terminalizes the core's unchanged pre-RUN retry
owner. An endpoint-bound
Test End transition now suppresses recurrence, finishes at most one published
event and retains its terminal response through graph restore. The remaining
runtime work is general completion routing beyond DTM list zero, source-127
expiration consumption and the first real Link Layer roles.

## Async runtime and coexistence

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Executor-neutral controller core | PARTIAL | One final Controller owner plus one production DTM runtime split into a finite ISR service, a DTM/scheduler command task, a disjoint source-127 modem-timer task, Host transport and combined Controller command endpoint. The modem-timer task alone owns the mutable queue and epoch; its borrowed readiness view carries no affine state, and finite begin/step/rearm calls retain every HAL owner inside the endpoint across recheck, publication backpressure and stable restore rejection. The final split consumes the endpoint's one-shot initial `CommandReady` claim into an opaque idle command task; drop or resplit cannot recreate it. The task service and every post-enable/current/preparation pending state own the sole HAL task owner by value while retaining the unique DTM-runtime borrow. A bounded first-event runner preserves that owner plus an opaque deferred RX/TX start from classification through cold/warm time, preparation, HEAD and RUN. In the ESP32-S31 composition, RUN creates the successful pending response; before RUN, only a fully recovered task plus chip-classified finite rejection can create Hardware Failure. Neutral cancellation across current, preparation and prepared graph phases recovers shutdown ownership through separate opaque types with no completion edge. Orphan-drain, restore-rejected, invariant and post-HEAD owners likewise expose no completion edge. A second finite runner owns completion through recycle and parks its affine state outside scheduler and post-unlink waits. A third finite runner owns recurring role-specific time, preparation, publication and RUN, including exact pre-HEAD cancellation and orphan draining. The active-session aggregate transports one pending or returned command-ready proof unchanged through every radio transition. Test End and Reset share one terminal-neutral quiescence core, and successful response publication returns the recomposed opaque idle command task. Lock-free scheduler, lock/modify and timer cells replace RTOS broker objects; every hardware-owned wait returns control. The target-only sole Embassy actor owns the complete command lifecycle without choosing raw HCI status. Production cold start now performs exactly one final split from a pre-reserved stable slot, exposes the Host side as `bt_hci::ExternalController<_, 1>`, and returns a Wi-Fi-shaped `{ hci, runners: { hardware } }` system plus initialization evidence. The eternal hardware runner gives sticky IRQ faults first polling priority, rotates command and modem-timer work, gates retries on a completed absolute deadline, yields after finite progress and retains every unsupported or failed owner only after full-route quarantine. Unrelated finished-list dispatch, a modem-timer expiration consumer, Selector-6 policy, Link Layer, ACL and live radio input remain absent. Lock/modify admission remains an explicit unsafe boundary outside that first empty-list path. |
| Executor-neutral HCI transport | LIVE | Packet arrival and capacity are wake edges; cancelled reads, writes, publications and capacity waits cannot consume or publish a packet. Capacity readiness reserves nothing, so the final consuming `try_publish` remains the source of truth. The mutex domain is selected by the platform and requires no RTOS. |
| Embassy controller owner | PARTIAL | One target-only sole command actor composes idle intake, bounded first-event drive, recovered preparation-failure completion, the two-axis active session, Test End and Reset while retaining every affine owner in an actor slot across borrowed waits. Chip code presents only a closed recovered completion, so the adapter cannot choose an HCI status or relabel fail-stop ownership. Borrow-only scheduler and post-unlink readiness futures register before recheck without owning the task runtime or consuming durable state. Radio-first selection races them against response capacity before publication and against epoch-bound Host command intake afterward; losing or cancelling receive consumes no packet. Separate borrowed stopping and terminal-response waits cover scheduler, post-unlink, absolute Controller-time recheck and non-reserving HCI capacity without moving their affine owners. Non-command packets remain typed for the future LL/ACL router. Serialized primary ISR service wakes the ordinary scheduler and lock/modify consumers as well as the post-unlink waiter, without creating duplicate event state. A separate cancellation-safe source-127 driver observes owner-free readiness and performs one finite transition without taking ownership from the outer runner. The concrete recheck provider retains an absolute Embassy deadline across cancelled waits and advances only after completion. Production cold start, final split, board spawning and runner wiring are live and fail-closed; unrelated-list dispatch, modem-timer expiration ownership and powered shutdown remain missing. |
| Standalone coexistence hooks | PARTIAL | The source-owned coexistence core and Embassy mailbox accept Bluetooth requests, but are not attached to Bluetooth lifecycle. |
| Concurrent Wi-Fi + BLE | ABSENT | Wi-Fi still owns the platform singletons independently. Safe joint composition is intentionally impossible until it migrates to the common coordinator. |
| Bluetooth low power | ABSENT | Low-power clock setup is not sleep ownership. Retention, wake compare, clock drift and exact resume/rollback are absent. |

The software Timeline remains private inside the Controller task runtime across
the whole powered epoch. The executor-facing task service can operate on it
only through phase-ordered affine preparation and recovery transitions.

## First operational profile

The first useful profile is deliberately smaller than the ESP32-S31 data-sheet
feature set:

1. LE-only Controller;
2. 1M PHY and legacy advertising;
3. one peripheral connection;
4. bounded ACL packets and explicit Host/Controller credits;
5. direct typed `bt-hci` integration with a Trouble GATT server;
6. no capability advertisement for encryption, privacy, extended advertising,
   Coded/2M PHY or ISO until each independent owner graph is complete.

Direct Test Mode, advertising, scanning, connection, security, coexistence and
low power each require separate HIL and qualification cells. Success with a
phone or Trouble is interoperability evidence, not Bluetooth qualification.

## Evidence required for the next physical publication

The next on-air transition is blocked by synchronous evidence for:

1. an affine open scheduler-item lifecycle and bounded completion queue,
   replacing rather than cloning the vendor static event and intrusive list;
2. feature-specific NRT interrupt policy, typed primary/NRT routing and
   handler-to-executor notification without lost work; the staged same-core
   ISR owner and core-owned durable primary publication are already composed;
3. the undocumented hardware interpretation of the private DTM RX/TX graph,
   complete element ownership, hardware current/next rotation and the
   completion-side visibility fence; the current blob has no separate
   software latch and the PAC now orders descriptor writes before head
   publication;
4. scheduler completion selection, timer/deadline wake and callback ordering;
   scanner-role resume is deliberately outside the DTM-only event graph and
   the vendor selector-6 container assertion has no open DTM successor;
5. 1M PHY channel, whitening, CRC, access-address and TX/RX result setup;
6. powered rollback and last-owner PHY shutdown.

Until those inputs exist, production must retain the current fail-closed
typestates and must not expose Controller, Link Layer, HCI or on-air readiness.
