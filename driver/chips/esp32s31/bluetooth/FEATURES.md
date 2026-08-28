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
ledger and dated HIL records. The pinned vendor-lifecycle classification and
the boundary between silicon requirements and replaceable Controller software
are recorded in
[`verification/vendor/targets/esp32s31/analysis/bluetooth-controller-boundary.md`](../../../../verification/vendor/targets/esp32s31/analysis/bluetooth-controller-boundary.md).

## Hardware lifecycle

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Cold radio ownership | LIVE | One affine `BluetoothStopped<P>` binds the platform lease to the protocol-neutral `RadioHardware` root before any Bluetooth lifecycle step. Clock enable, rollback and clean clock shutdown all return that same aggregate; production cannot split the pair or use the old independent-resource entry. Opaque HAL task/IRQ owners preserve the lower affinity: only an untouched task owner and pristine interrupt history can reconstruct the neutral root; mutable controller or shared-PHY access arms fail-stop reunion. This does not support concurrent Wi-Fi. |
| Bluetooth platform lease | LIVE | The role-neutral ESP-HAL coordinator privately owns the remaining upstream system singletons and reference-counts the shared PLL source. The affine custom PAC owns MODEM shared-clock leases and baseline restoration. Wi-Fi has not migrated to this coordinator. |
| Controller clock/reset prerequisite | LIVE | The standalone main-XTAL/100-kHz low-power timer profile has semantic read-back and reverse-order rollback before controller MMIO. |
| Controller initialization | FAIL-CLOSED | The public path clears bits 19:0 of sixteen scheduler-table entries through a sealed finite `BluetoothControllerHal<'_>` borrow, then can consume one pristine `BluetoothControllerRuntimeResources<N>` aggregate. That fixed no-RTOS owner binds scheduler wake, lock/modify event+worker, finished-list worker and LP-timer queue+epoch+event to the same powered task/IRQ epoch without claiming readiness. The hardware-bound state's sole mutable split yields one matching interrupt publisher and task worker borrow; the Embassy layer now owns only executor wakers and cannot allocate duplicate event cells or inject an unrelated lock/modify worker. A disconnected consuming PAC/HAL leaf applies the exact eight-MMIO source-127 modem-LP-timer register prefix and can transfer its affine task partition into a dedicated ISR-ready owner; it is not wired because the open scheduler item queue, remaining hardware initialization and stable route composition are missing. The LLL cannot recover or duplicate its opaque HAL task owner, and mutable access prevents neutral-root reconstruction. |
| Controller HAL initialization | PARTIAL | The complete 50-operation BTDM HAL body has a typed config (`8/16`, `500/1000/2000`, two positional bytes and validated controller SRAM), exact caller-derived standalone outputs (`3`, `22`, `66`, `0x2f000000`) and ordered host regression coverage. It remains validation-only because the event/list, BLE enable and inactive-IRQ prerequisites are not yet composed. |
| Common PHY registration | PARTIAL | The complete async common-PHY transition and retained result type exist internally, but no valid caller can cross the incomplete controller-init frontier. They are not exported as a disconnected production API. |
| Bluetooth baseband initialization | PARTIAL | The finite `bt_bb_v2_init_cmplx(1)` transaction consumes completed common-PHY ownership internally, but remains unreachable from public initialization and is not exported independently. |
| BLE PHY register initialization | FAIL-CLOSED | A complete reviewed register transaction exists only behind the validation boundary because its controller, storage and inactive-IRQ prerequisites are not owned in production. |
| Powered teardown | PARTIAL | Opaque HAL owners retain task and interrupt affinity fail-stop. The disconnected bounded scheduler-disable leaf consumes its task owner, writes the exact command image `1`, fences publication, and—only after CPU routes are already disabled—permits one terminal observation of the `BUSY` bit. Its powered lifecycle prerequisite is intentionally not faked by a PAC `assume_satisfied` token; no production controller state reaches the leaf yet. Both busy and idle observations are terminal: there is no recheck/wake source, packet/bottom-half quiescence, IRQ-output release, BTBB/PHY/clock teardown, or cold reconstruction. No comparison or HIL claim is made. |

## Interrupts, timing and packet storage

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Controller interrupt output | PARTIAL | Opaque HAL setup/prepared/register/after-routes owners wrap the restricted PAC, which consumes inactive IRQ ownership, clears and enables the exact primary baseline groups (`0x00008000`, `0x00001300`), publishes the setup strobe at `0x2010_100c`, and reverses the two release strobes before masking those groups. The owner can be staged once for shared primary/NRT ISR storage, but no CPU route is live and the after-routes owner has no public release edge. |
| Interrupt observation | PARTIAL | Separate restricted transactions preserve the vendor distinction: primary samples the two masked status banks, NRT samples the two snapshot banks, and both acknowledge through the shared W1C clear banks. The PAC projects primary status into semantic dynamic, fault and unclassified-source facts; the NRT transaction returns only an affine acknowledgement token. Conditional diagnostic reads remain at their exact fault-lane positions without exporting their images. The source-124 controller step composes this PAC transaction with the affine HAL owner, preserves the two temporal scheduler reads and produces one paired scheduler/lock-modify event; the Embassy endpoint publishes both durable handoffs. PAC and HAL expose an affine ordered scheduler-reference clear, but an idle bank-one reference gate still stops before invoking it because the mandatory selector-6 scheduler invariant is not implemented. The pinned default-profile NRT path now crosses PAC, HAL, controller and Embassy as one opaque acknowledged epoch and deliberately publishes no synthetic work; feature-specific NRT policy remains closed. Source 127 has a separate consuming PAC/HAL owner, bounded classifier and common-handler register step. Its controller layer owns a fixed generational absolute-deadline queue, wrapping epoch, one-expiration-per-step drain, atomic backpressured event cell and a publication gate that prevents final rearm before every removed expiration is durable. Live stable ISR storage/route composition remains missing. |
| Interrupt masks and CPU route | PARTIAL | Primary source 124, modem-timer source 127 and NRT source 133 are typed policies on the configured Controller core at level 3; primary and modem timer request IRAM residency, while NRT does not. The pinned external PAC exposes only `BT_MAC`/`BT_MAC_INT1`, so the ESP-HAL adapter compile-checks and binds/disables sources 124/133 without raw casts but cannot yet bind source 127. Baseline fault and dynamic scheduler masks are exact. The 16-bit finished-list transfer is typed, and a bounded task-side worker yields one positional list index per step without claiming an item completion. Hardware-list-to-item mapping, selector 4 scanner resume and selector 6 post-clear action remain missing. A separate no-RTOS Embassy adapter turns the atomic scheduler cell into an affine register-before-recheck wake channel with cancellation-safe coalescing. The external PAC variant, stable source-127 ISR storage, live ISR composition, feature-specific NRT policy and the public live-route lifecycle remain absent. |
| Controller timer/scheduler | FAIL-CLOSED | The sixteen-entry scheduler-table prefix is live through the narrow controller HAL, the later HAL-init register body is implemented but disconnected, and the task-side finished-list mask transfer plus ISR reference/state MMIO order are typed. HAL exposes the finite finished-list transfer; its bounded worker retains a captured mask and yields only list indices. It cannot select an item, return descriptor ownership or establish completion visibility. The lock/modify transaction has live finite PAC/HAL publication: two fresh operational-word RMW edges, a generated typed START/pointer write and a trailing device fence. BUSY stays interrupt-owned while START/RESULT stay task-owned; a lock-free latest-value event cell and durable single-request worker join semantic observations one event at a time. The terminal transaction result retains the exact typed request identity so a later controller stage cannot attach it to a different prepared graph; this is still only publication-transaction evidence, not scheduler-item or radio completion. Source 124 feeds that event from the same later scheduler snapshot used by the general wake. The source-127 software queue is fixed-capacity, generation-safe and wrapping-deadline ordered; its affine work performs one expiration or one rearm decision per step, applies event backpressure without overwrite and cannot reach the final fresh hardware read before expiration publication. The always-awake controller-time path has live bounded HAL MMIO and one durable logical worker stored beside the unique task owner. The scheduler-disable path still permits only one terminal observation after route disable. Selector 4 and selector 6 remain missing policy/hardware actions. Powered runner/live-route composition, affine head publication, item mapping, packet completion visibility/recycle, BTBB/PHY/clock teardown, counter unit and width, and remaining command/status semantics are absent. |
| Controller memory lists | FAIL-CLOSED | Three positional selectors and their instruction-identical current/next RX pointer leaves retain only MMIO geometry in the PAC. The controller-memory layer owns the exact normal global-insertion split: scanner kind 2 uses selector 1 and admitted non-scanner kinds use selector 2. Selector 3 is serviced by generic software maintenance but has no current hardware-publication caller. DTM kind 5 explicitly bypasses normal global insertion: static binding installs its private graph links, complete common-plus-DTM allocation flags and exact source-configured low-twelve-bit scheduler allocation field; the three public ESP-IDF limits and still-positional private-options halfword are mandatory inputs, with no inherited vendor default. One consuming CPU-only transition then samples the current private TX/RX anchors, validates all three links and writes only the seventeen reviewed link-state/scheduler words; a second consuming transition installs the reviewed in-flight status, cleared control byte and empty software-completion link. All states remain CPU-owned before publication. Complete hardware-consumed layouts, internal DTM pointer latch, hardware current/next rotation, visibility fences and affine reclamation remain absent. |
| In-process HCI handoff | LIVE | An affine split provides a `bt-hci::ExternalController` Host transport and one Controller-worker endpoint. Both bounded directions validate complete packets, apply async backpressure, wake on capacity/data, retain packets across short buffers and leave queues unchanged when waits are cancelled. Packet kinds remain typed; no UART/H4 framing or allocator is used. |
| Packet TX/RX | ABSENT | No BLE buffer publication, radio command, completion owner, RSSI result or on-air packet path exists. |

## LE Controller and HCI

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Direct Test Mode | ABSENT | The exact S31 TX/RX/Test-End command bodies, allocator roles, common scheduler path and DTM recycle callback are mapped. The controller-SRAM link-state reset and pre-insert scheduler item have exact transforms for their eight- and nine-word reviewed regions, including bidirectional epoch projection. All forty HCI DTM channels compose through exact cross-revision permutation/frequency tables, and role-dependent 1M/2M/Coded PHY-rate images reject TX-only Coded S=2 on RX. All eight TX test-pattern selectors prepare a bounded caller-owned payload; PRBS9/PRBS15 reproduce the complete cross-revision tables without heap or retained vendor data. The 936-byte reviewed graph is now a non-movable static allocation: target binding derives its real field addresses, distinguishes the physical 512-KiB SRAM window from the wider compressed-pointer syntax, rejects zero links and out-of-range extents before mutation, then installs the exact RX/TX headers, five private-chain anchors and scheduler-item-to-link-state link. The event-word owner must first consume itself into the scheduler-bookkeeping state. That retained graph can derive the exact argument-zero request identity internally, but no external publication API exists while the descriptor is incomplete. A native model uses a separate synthetic typed base, and failed binding returns the unchanged allocation. Packet duration, the 625-usec interval rule and the complete source-owned scheduler tick/remainder conversion are exact for all four TX PHY choices; S31's unit and identity conversion leaves make the tick image equal the interval with zero remainder. Initial and recurring TX scheduler windows preserve the exact anchor, maximum-capacity occupancy and late-event phase; catch-up is constant-time instead of the vendor CPU loop. The RX result parser also lives above PAC in controller memory. One bounded LLL transition reproduces the exact low-24 validation, positional high-byte update and wrapping 16-bit received-packet count, then requires a separate append decision for both accepted and rejected words. Ordinary RX re-arm updates packet sentinels and clears the header completion bit as one memory transition; the swap-reserve branch remains quarantined. Production `.dma.bss` ownership, the remaining hardware-read fields, private packet-engine latch, live clock/RF-ready/margin source, semantic IRQ-to-finished-list mapping, publication/completion visibility fences and owned append/quiesce are missing, so no command reaches production radio hardware. |
| Legacy advertising | ABSENT | No advertising PDU owner, channel scheduler or HCI advertising commands exist. |
| Scanning | ABSENT | No observer, filter, duplicate list or advertising-report event path exists. |
| Peripheral connection | ABSENT | Connection acceptance, anchors, channel selection, SN/NESN, retransmission, supervision and LL control are absent. |
| Central connection | ABSENT | Initiating and central connection scheduling are absent. |
| ACL flow control | ABSENT | No Host/Controller credits, fragmentation boundary, completed-packet event or controller ACL queue exists. |
| Link encryption and privacy | ABSENT | No Link Layer AES-CCM epoch, packet counters, LTK procedure or resolving-list owner exists. Register discovery alone is not encryption. |
| Extended BLE features | ABSENT | Data Length Extension, 2M/Coded PHY, extended/periodic advertising, PAwR, subrating, direction finding and ISO are outside the initial slice. |
| HCI bootstrap dispatcher | LIVE | A pure closed table implements Reset, base/LE event masks, LE buffer/list-size reports, public/random address handoff, Host buffer/flow-control configuration and a zero optional-LE-feature report. Reset opens a fresh configuration epoch; malformed known commands do not mutate it. Event Mask Page 2 and every advertising/scanning/connection command return standard fail-closed errors. Reported filter-list size is zero. |
| Typed HCI Controller | ABSENT | The in-process transport and bootstrap worker serve the closed initialization command subset through `bt-hci::ExternalController`, but no ESP32-S31 worker implements operational commands, events or ACL semantics. |
| Trouble Host integration | ABSENT | A real Trouble 0.7 Runner completes its no-security software bootstrap through the production bootstrap worker in host tests. A post-initialization filter-list command is observed and rejected fail-closed. No composed production Runner task, ESP32-S31 Controller worker, GATT example or hardware path exists. |

## Async runtime and coexistence

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Executor-neutral controller core | PARTIAL | The affine bootstrap worker has ordered async stop, retains accepted responses across backpressure/cancellation and rejects Host data before LL readiness. A lock-free pending/marked cell preserves the reference scheduler event's coalescing contract without an RTOS, the first lock/modify request phase and controller-time latch return control on every hardware-owned wait, and the finished-mask drain handles one list per event step. The item-recycle source is a replaceable software lifecycle, but no typed scheduler queue, selector-6 invariant, registered time-latch waker/recheck, Link Layer or live radio event input exists. |
| Executor-neutral HCI transport | LIVE | Packet arrival and capacity are wake edges; cancelled reads, writes and publications cannot consume or publish a packet. The mutex domain is selected by the platform and requires no RTOS. |
| Embassy controller owner | ABSENT | No composed sole hardware task, ISR wake queue, timer adapter or powered shutdown exists. The portable bootstrap worker deliberately owns none of these platform concerns. |
| Standalone coexistence hooks | PARTIAL | The source-owned coexistence core and Embassy mailbox accept Bluetooth requests, but are not attached to Bluetooth lifecycle. |
| Concurrent Wi-Fi + BLE | ABSENT | Wi-Fi still owns the platform singletons independently. Safe joint composition is intentionally impossible until it migrates to the common coordinator. |
| Bluetooth low power | ABSENT | Low-power clock setup is not sleep ownership. Retention, wake compare, clock drift and exact resume/rollback are absent. |

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
2. feature-specific NRT interrupt policy, composition of the staged same-core
   ISR owner, typed primary/NRT routing and bounded acknowledgement/wake/re-arm
   without lost work;
3. the internal consumer/latch of the private DTM RX/TX graph, complete
   element ownership, hardware current/next rotation and CPU/device visibility
   fences;
4. radio scheduler command, timer, doorbell and completion ordering, including
   the selector-6 consistency invariant; scanner-role resume is deliberately
   outside the DTM-only event graph;
5. 1M PHY channel, whitening, CRC, access-address and TX/RX result setup;
6. powered rollback and last-owner PHY shutdown.

Until those inputs exist, production must retain the current fail-closed
typestates and must not expose Controller, Link Layer, HCI or on-air readiness.
