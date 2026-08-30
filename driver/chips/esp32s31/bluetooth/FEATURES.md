# ESP32-S31 Bluetooth LE feature frontier

This document describes the source-owned production boundary. It is not a
Bluetooth Core, RF, HIL or product-qualification claim. A register setter,
validation probe, isolated vendor comparison or disconnected state-machine
component is not a live controller capability unless the production owner
graph reaches the physical Bluetooth boundary.

The first controller target is Bluetooth Low Energy only. Bluetooth Classic
BR/EDR, Mesh profiles and LE Audio profiles are separate programs and are not
implied by progress on the LE Controller. The initial Host contract is the
released `trouble-host` 0.7.0 / `bt-hci` 0.9.0 pair. Moving that boundary is an
explicit compatibility change.

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
| Bluetooth platform lease | LIVE | The role-neutral ESP-HAL coordinator privately owns the remaining upstream system singletons and reference-counts the shared PLL source. The affine custom PAC owns MODEM shared-clock leases and baseline restoration. The lease reads the factory base identity through ESP-HAL's safe eFuse accessor, applies S31's reviewed second-universal-address policy and returns a canonical typed address; the generic HCI layer alone converts it to `BD_ADDR` wire order. Wi-Fi has not migrated to this coordinator. |
| Controller clock/reset prerequisite | LIVE | The standalone main-XTAL/100-kHz low-power timer profile has semantic read-back and reverse-order rollback before controller MMIO. |
| Controller initialization | FAIL-CLOSED | The public target path preserves the recovered order from clock/reset through the complete Controller HAL component, sixteen-head scheduler reset, bounded HCI bootstrap, source-127 low-power component, common PHY, BTBB, BLE-PHY, Controller output, runtime timer and atomic stable publication of both interrupt owners. Every state retains the preceding owners and no state can skip or duplicate an enable stage. The initialized scheduler keeps the reviewed `40/46` policy and one pristine bounded runtime instead of vendor task/broker containers. At the terminal published-owner state the retained always-awake marker gates one affine post-enable time acquisition: Pending borrows that exact Controller, Waiting retains the same owner, completion initializes the persistent scheduler epoch from the first live sample and the retained HAL scale, and drop/cancel enters an explicit sample-discarding orphan drain. The same sample remains inseparable from that epoch and supplies one private DTM current. Only the composed owner which also retains the settled Bluetooth PHY client and completed BLE-PHY transaction can turn a later completed standalone-policy time request into an opaque non-`Copy` RF-ready token; neither the marker nor the PHY client alone grants that authority. Initial TX/RX acquire RF-ready after current and then enter private admission, overlap-resolving reservation and later sequence authorization. Recurring RX acquires RF-ready before a fresh current, while recurring TX acquires only current; both recurring paths reserve before sequence authorization. Terminal success, rejection or explicit cancellation returns the exact Controller epoch and role owner, while Drop releases any occupied reservation before leaving a late latch result for orphan drain. The cold epoch path is permanently rejected. The retained owner can issue repeated generation-keyed acquisitions; only current Ready reanchors the persistent epoch to the fresh raw sample while preserving its scheduler image, whereas RF-ready completion only projects its sample into that epoch. The standalone producer performs no RF MMIO because the reviewed disabled-sleep branch is already awake; the sleep-enabled wake branch remains unimplemented. Before any CPU route is enabled, the terminal Controller can publish only its identity-checked first DTM head, immediately consume CPU rollback ownership into the exact PAC-published hardware graph and advance it through the typed dynamic-interrupt, synchronous BTMAC event and RUN suffix. The same Controller can perform one fresh fenced finished-list transfer and immediately join list zero to that exact running epoch; a non-sentinel status advances only to a hardware-owned completion observation. If that transfer retains more lists, source-owned running and completed continuations consume one retained bit per call without another hardware transfer and return unrelated affine observations losslessly. This continuation is not a role dispatcher or autonomous session driver. All three live CPU routes, operational HCI/LLL and powered teardown remain missing. The LLL cannot recover or duplicate opaque HAL owners. |
| Controller HAL initialization | PARTIAL | The complete 50-operation BTDM HAL body has a typed config (`8/16`, `500/1000/2000`, two positional bytes and validated controller SRAM), exact caller-derived standalone outputs (`3`, `22`, `66`, `0x2f000000`) and ordered host regression coverage. Selecting the reviewed standalone profile immediately after clock/reset mints one private affine zero-sized always-awake policy marker and retains it through scheduler, HCI, PHY, BTBB, BLE-PHY and pre-route interrupt-owner publication. The marker performs no MMIO and is neither RF-ready nor controller-time authority. The HAL transition still establishes no scheduler-list, interrupt, PHY, BTBB, Link-Layer or HCI readiness. |
| Common PHY registration | PARTIAL | The target-only async registration transition borrows the outer Controller's platform and shared-PHY HAL for one concrete `register_chipv7_phy` run. Only terminal target success mints `RegisteredBluetoothPhy`, retained by `BluetoothControllerPhyRegistered`; registration failure retains the outer Controller in fail-stop state. Bluetooth-client acquisition is a separate affine edge: it either settles immediately or retains pending/in-flight tracking until the concrete target tracking runner returns a settled `RegisteredBluetoothPhyClient`. A terminal tracking error retains a poisoned lower owner and the outer Controller fail-stop; cancelling the in-flight future instead drops the unique epoch and requires out-of-band hardware reset. The nested always-awake marker records the source-owned profile choice only; neither it nor client acquisition proves current RF state, warm wake, a per-event RF-ready instant, periodic tracking runtime, last-owner release, operational BLE radio readiness or HIL. |
| Bluetooth baseband initialization | PARTIAL | Only `BluetoothControllerPhyInitialized` retaining a settled `RegisteredBluetoothPhyClient` exposes `initialize_baseband`; registered-only, pending, tracking and poisoned states cannot reach BTBB. The transition projects the required byte from the retained registered PHY state and executes only the finite reviewed `bt_bb_v2_init_cmplx(1)` PAC/HAL transaction. `BluetoothControllerBasebandInitialized` keeps the settled client and every earlier Controller owner nested, but does not prove complete common-PHY/baseband operation, BLE radio-engine readiness, teardown, a composed vendor trace or physical qualification. |
| BLE PHY register initialization | PARTIAL | `BluetoothControllerBasebandInitialized::initialize_ble_phy_engine` consumes an address-bound `BluetoothBlePhyEngineCpuOwned`, derives both pointers from its pinned static allocation and retains that owner in `BluetoothControllerBlePhyEngineInitialized`. The complete reviewed base-stack task-enable hardware component is reached only through PAC accessors: it first enables the access-address low-correlation field through a generated fresh RMW and then executes the recovered BLE PHY register-init body. Upper layers pass typed storage addresses and three explicit source-owned positional configuration values, never register images or vendor configuration layouts. The memory owner replaces the vendor allocator with one static graph containing the complete `0x68`-byte environment, its `0x28/0x08/0x04` auxiliary allocations and one `0x40`-byte resolving-list hardware object; binding installs the three internal pointers and reviewed initial list head before publication. Production integration owns a unique `.dma.bss` arena and exposes only a one-shot CPU-owned claim; the following consuming state prepares Controller output and starts the runtime timer while retaining that storage. Optional BQB state, environment word semantics, stable live IRQ routes and physical qualification remain missing, so this is hardware initialization rather than operational BLE. |
| Powered teardown | PARTIAL | Opaque HAL owners retain task and interrupt affinity fail-stop. The disconnected bounded scheduler-disable leaf consumes its task owner, writes the exact command image `1`, fences publication, and—only after CPU routes are already disabled—permits one terminal observation of the `BUSY` bit. Its powered lifecycle prerequisite is intentionally not faked by a PAC `assume_satisfied` token; no production controller state reaches the leaf yet. Both busy and idle observations are terminal: there is no recheck/wake source, packet/bottom-half quiescence, IRQ-output release, BTBB/PHY/clock teardown, or cold reconstruction. No comparison or HIL claim is made. |

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
service complete standalone RF-ready requests; the resulting token is opaque,
non-`Copy` and never accepted as a detached scheduler instant. Initial TX/RX
consume current before RF-ready, recurring RX consumes RF-ready before current,
and recurring TX consumes current only.
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
| Controller interrupt output | PARTIAL | Opaque HAL setup/prepared/register/after-routes owners wrap the restricted PAC, which consumes inactive IRQ ownership, clears and enables the exact primary baseline groups (`0x00008000`, `0x00001300`), publishes the setup strobe at `0x2010_100c`, and reverses the two release strobes before masking those groups. The owner can be staged once for shared primary/NRT ISR storage, but no CPU route is live and the after-routes owner has no public release edge. |
| Interrupt observation | PARTIAL | Source 124 now executes the complete bounded hardware order `acknowledge -> reference-gate read -> conditional SCHEDULER_REFERENCE clear/fence -> fresh work read` and durably publishes one paired scheduler/lock-modify event. Vendor selector 6 performs no hardware action; it only validates members of a private intrusive transaction container that the affine open DTM scheduler does not create. Source 127 and default-profile NRT have finite typed dispositions. Live handler entry and executor notification remain missing. |
| Interrupt masks and CPU route | PARTIAL | Primary source 124, modem timer source 127 and NRT source 133 have typed level-three policies and stable process-wide owner slots. Dynamic masks, finished-list transfer and current-list observation are typed. The public live-route epoch remains closed until handler-to-runner notification and bounded role dispatch are composed; scanner selector 4 and feature-specific NRT policy belong to later roles, not first DTM. |
| Controller timer/scheduler | FAIL-CLOSED | The typed path already covers controller time, sole-item admission, full descriptor ordering, list-zero HEAD publication, dynamic interrupt preparation, RUN, finished-list observation, fenced head retirement, atomic unlink/mailbox arm and TX/RX recycle. Post-unlink readiness is sticky through first publication and mailbox-full coalescing; the Embassy adapter registers before rechecking it and cancellation cannot consume it. Selector 6 is not an open-runtime blocker because its vendor callback only inspects the vendor transaction container. Remaining blockers are a concrete autonomous session pump which owns that retry wait, live route composition, complete on-air descriptor semantics and powered teardown. |
| Controller memory lists | FAIL-CLOSED | Three positional global RX selectors remain restricted to PAC accessors; the memory layer owns their scanner/non-scanner routing, while DTM kind 5 bypasses them and uses its private graph. Static binding installs full private software-list pointers separately from compressed hardware links. TX preparation copies the complete declared packet, and role-specific plans install the reviewed link-state/scheduler words plus scheduler bookkeeping. The exact affine PAC head token consumes rollback authority into pinned `HeadPublished`; the matching RUN proof alone advances it to `Running`. Scheduler items, link-state, both RX headers and hardware-written RX packet words use volatile semantic accessors, and only `Running` can accept a matching fenced list token to classify completion without granting CPU ownership. Fresh typed head observation, software unlink and removal-ready proofs authorize cleanup. TX and RX nonzero-status paths return directly to CPU ownership. The zero-status RX transaction validates the exact two-header capacity-one topology before mutation and binds the typed result to its sole graph owner. Controller composition consumes that result into the same non-copyable receiver session before the owner can commit the reviewed swap/re-arm rotation, and exposes it only after timeline and source-list release. The current blob has no separate software DTM pointer latch. |
| In-process HCI handoff | LIVE | One affine aggregate owns the transport queues and bootstrap state, rejects advertised ACL profiles larger than its static storage and starts only as a pristine HCI epoch. After scheduler initialization it is consumed into the same powered Controller owner; the final split provides matching finite ISR/scheduler-task endpoints plus a `bt-hci::ExternalController` Host transport, raw Controller endpoint and mutable bootstrap state together. Both bounded directions validate complete packets, apply async backpressure, wake on capacity/data, retain packets across short buffers and leave queues unchanged when waits are cancelled. Packet kinds remain typed; no UART/H4 framing or allocator is used. |
| Packet TX/RX | ABSENT | No BLE buffer publication, radio command, completion owner, RSSI result or on-air packet path exists. |

## LE Controller and HCI

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Direct Test Mode | PARTIAL | The exact S31 TX/RX/Test-End command bodies, allocator roles, common scheduler path and recycle callback are mapped. Channel/PHY selection, all eight TX patterns, packet duration, interval/tick arithmetic, initial/recurring windows and bounded overlap resolution are typed. The 936-byte graph is a non-movable static allocation with exact RX/TX headers, private-chain anchors, scheduler context, scheduler-item-to-link-state link and role-specific event/bookkeeping transforms; TX readiness survives every CPU-owned transition. Scheduler initialization proves all heads and its source-owned software list empty, and the sole-item merge is identity-checked and cancellable. A bounded first-event runner owns one semantic HCI TX/RX v1 command across cold or warm current acquisition, preparation, list-zero HEAD publication and hardware RUN. A second opaque runner consumes that exact active owner through fresh completion capture, both retained finished-list continuations, fenced head retirement, atomic unlink/mailbox wait and role-correct recycle. A third bounded runner consumes the reclaimed TX/RX boundary and follows the role-specific recurring time/preparation order back through HEAD and RUN. RX completion metadata remains attached through every wait, retry, cancellation and orphan-drain branch. Before HEAD, cancellation returns the exact CPU owner or a typed drain obligation; after HEAD, rollback is deliberately unavailable. Every step remains finite and no executor future owns affine hardware state. The sleep-enabled RF wake branch, long-lived recurring session composition, Test End command ownership and the HCI endpoint loop remain missing, so no autonomous on-air session is claimed. |
| Legacy advertising | ABSENT | No advertising PDU owner, channel scheduler or HCI advertising commands exist. |
| Scanning | ABSENT | No observer, filter, duplicate list or advertising-report event path exists. |
| Peripheral connection | ABSENT | Connection acceptance, anchors, channel selection, SN/NESN, retransmission, supervision and LL control are absent. |
| Central connection | ABSENT | Initiating and central connection scheduling are absent. |
| ACL flow control | ABSENT | No Host/Controller credits, fragmentation boundary, completed-packet event or controller ACL queue exists. |
| Link encryption and privacy | ABSENT | No Link Layer AES-CCM epoch, packet counters, LTK procedure or semantic resolving-list lifecycle exists. The opaque storage retained by BLE PHY initialization proves only address stability and extent; it does not implement privacy. |
| Extended BLE features | ABSENT | Data Length Extension, 2M/Coded PHY, extended/periodic advertising, PAwR, subrating, direction finding and ISO are outside the initial slice. |
| HCI bootstrap state machine | LIVE | A pure closed table implements Reset, base/LE event masks, LE buffer/list-size reports, public/random address handoff, Host buffer/flow-control configuration and a zero optional-LE-feature report. Reset opens a fresh configuration epoch; malformed known commands do not mutate it. Event Mask Page 2 and advertising/scanning/connection commands remain unclaimed for the future outer Link-Layer router; the standalone bootstrap test harness rejects them fail-closed. Reported filter-list size is zero. |
| Typed HCI Controller | PARTIAL | The in-process transport exposes disjoint Host, raw Controller and bootstrap endpoints through `bt-hci::ExternalController`. A finite portable classifier returns owned semantic bootstrap and DTM commands plus owned malformed or Unknown Command responses; no result borrows receive scratch storage. The ESP32-S31 first-event runner consumes Receiver/Transmitter Test v1 command ownership through hardware RUN. Its private response authority can enter the real bounded HCI queue only through a consuming non-blocking publication to the same source-owned HCI epoch; queue-full and validation faults return the unchanged active owner, while success releases the response exactly once. A cancellation-safe side-effect-free capacity wait allows the affine response to remain outside every awaited future. Reclaimed hardware ownership can now be prepared and published as the next recurring event, but that radio progression is not yet composed independently of start-response backpressure. The endpoint command loop, active Test End and ACL semantics remain absent. |
| Trouble Host integration | ABSENT | A real Trouble 0.7 Runner completes its no-security software bootstrap through those production transport/bootstrap endpoints in host tests. A post-initialization filter-list command is observed and rejected fail-closed. No composed production Runner task, ESP32-S31 session runner, GATT example or hardware path exists. |

The current DTM frontier includes exact initial and recurring TX/RX timing,
phase-specific descriptor configuration and affine Active TX/RX aggregates
that retain immutable command inputs and the last committed window across
recycle. A prepublication merge is phase-tagged: cancelling an initial merge
returns its fresh graph/session ownership, while cancelling a recurring merge
returns the exact Active owner. Either successful cancellation releases the
exclusive list and private timeline reservation, and a merge cannot cross the
initial/recurring cancellation boundary. These remain preparation and recovery
primitives, not a live radio loop. The first post-enable controller sample
initializes the persistent scheduler epoch, and each owning acquisition
reanchors that epoch before one preparation consumes its private current. The
same movable post-split task service now privately acquires RF-ready, admission and
sequence in their exact source order, publishes the resulting head and owns the
existing RUN-through-recycle suffix. The reviewed standalone margin is retained
by the source-owned scheduler policy, while the completed powered BLE-PHY plus
always-awake time request supplies a non-detachable RF-ready result. The first-event
runner sequences semantic HCI TX/RX v1 through this complete bounded prefix and
retains the resulting active owner through later response backpressure. An
executor-neutral active-completion runner now joins that owner to the complete
finished-list, retirement, unlink, mailbox and role-specific recycle suffix.
The reclaimed boundary enters a second finite runner which follows the exact
role-specific recurring preparation order back to hardware RUN and supports
lossless cancellation before HEAD. A long-lived session still must keep that
radio axis progressing independently of HCI start-response backpressure and
retain Test End through the final completion.

## Async runtime and coexistence

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Executor-neutral controller core | PARTIAL | One final Controller owner plus one production DTM runtime now split into disjoint finite ISR service, movable DTM/scheduler task service and raw HCI/bootstrap endpoints. The task service and every post-enable/current/preparation pending state own the sole HAL task owner by value while retaining the unique DTM-runtime borrow, eliminating the self-reference that previously prevented a concrete async session task. A bounded semantic first-event runner preserves that owner from HCI TX/RX v1 through cold/warm time, preparation, HEAD and RUN, with explicit reversible cancellation and irreversible post-publication ownership. Preparation cancellation drains any abandoned time latch and restores the exact checked-out graph before returning a reusable Controller. Pre-RUN hardware rejections retain an opaque role-consistent retry owner. A second finite runner owns completion through recycle and parks its affine state outside scheduler and post-unlink waits. A third finite runner owns recurring role-specific time, preparation, publication and RUN, including exact pre-HEAD cancellation and orphan draining. None of the runners loop on hardware. Lock-free scheduler, lock/modify and timer cells replace RTOS broker objects; every hardware-owned wait returns control. The remaining executor-neutral DTM blocker is the two-axis long-lived session, Test End ownership and endpoint command intake. Lock/modify admission remains an explicit unsafe boundary outside that first empty-list path. Selector-6 policy, Link Layer and live radio input remain absent. The Embassy adapter supplies wake primitives but does not yet own the complete controller task. |
| Executor-neutral HCI transport | LIVE | Packet arrival and capacity are wake edges; cancelled reads, writes, publications and capacity waits cannot consume or publish a packet. Capacity readiness reserves nothing, so the final consuming `try_publish` remains the source of truth. The mutex domain is selected by the platform and requires no RTOS. |
| Embassy controller owner | PARTIAL | A target-only adapter drives every immediately ready first-event phase, parks the affine runner outside caller-provided recheck futures, and parks the running response outside the HCI-capacity wait. Serialized primary ISR service now wakes the ordinary scheduler and lock/modify consumers as well as the post-unlink waiter, without creating duplicate event state. It preserves every cancellation/retry owner but does not yet compose command intake, active-completion wait selection, recurring events, Test End, ISR route installation or powered shutdown into one sole task. |
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
