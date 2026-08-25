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
| Cold radio ownership | LIVE | One protocol-neutral `RadioHardware` root can enter and leave the exclusive Bluetooth route without MMIO or owner loss. This does not support concurrent Wi-Fi. |
| Bluetooth platform lease | LIVE | The role-neutral ESP-HAL coordinator privately owns the required system singletons and reference-counts shared clock dependencies. Wi-Fi has not migrated to this coordinator. |
| Controller clock/reset prerequisite | LIVE | The standalone main-XTAL/100-kHz low-power timer profile has semantic read-back and reverse-order rollback before controller MMIO. |
| Controller initialization | FAIL-CLOSED | The public path stops after clearing bits 19:0 of sixteen scheduler-table entries. Software events/lists, low-power state, controller task and HCI initialization are missing. |
| Controller HAL initialization | PARTIAL | The complete 50-operation BTDM HAL body has a typed config (`8/16`, `500/1000/2000`, two positional bytes and validated controller SRAM), exact caller-derived standalone outputs (`3`, `22`, `66`, `0x2f000000`) and ordered host regression coverage. It remains validation-only because the event/list, BLE enable and inactive-IRQ prerequisites are not yet composed. |
| Common PHY registration | PARTIAL | The complete async common-PHY transition and retained result type exist, but no valid caller can cross the incomplete controller-init frontier. |
| Bluetooth baseband initialization | PARTIAL | The finite `bt_bb_v2_init_cmplx(1)` transaction consumes completed common-PHY ownership, but remains unreachable from public initialization. |
| BLE PHY register initialization | FAIL-CLOSED | A complete reviewed register transaction exists only behind the validation boundary because its controller, storage and inactive-IRQ prerequisites are not owned in production. |
| Powered teardown | ABSENT | Powered states retain owners fail-stop. Complete controller shutdown, last-owner PHY shutdown and cold reconstruction are not proven. |

## Interrupts, timing and packet storage

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Controller interrupt output | PARTIAL | The restricted PAC consumes inactive IRQ ownership, clears and enables the exact primary baseline groups (`0x00008000`, `0x00001300`), publishes the setup strobe at `0x2010_100c`, and reverses the two release strobes before masking those groups. The owner can be staged once for shared primary/NRT ISR storage, but no CPU route is live. |
| Interrupt observation | PARTIAL | Separate restricted transactions preserve the vendor distinction: primary samples the two masked status banks, NRT samples the two raw banks, and both acknowledge through the shared W1C clear banks. The primary dynamic scheduler groups are positionally classified without assigning LL names; baseline and NRT bits remain opaque. |
| Interrupt masks and CPU route | PARTIAL | Primary source 124 and NRT source 133 are typed policies on the configured Controller core at level 3; primary requests IRAM residency and NRT does not. The pinned PAC exposes `BT_MAC`/`BT_MAC_INT1`, and the ESP-HAL adapter compile-checks their numbers and binds/disables the pair on one core without raw casts. Baseline and dynamic masks are exact. Shared ISR storage, baseline/NRT meanings and the public live-route lifecycle remain absent. |
| Controller timer/scheduler | FAIL-CLOSED | The sixteen-entry scheduler-table prefix is live and the later HAL-init register body is implemented but disconnected. Radio epochs, deadlines, command/status semantics and completion handling are absent. |
| Controller memory lists | FAIL-CLOSED | Three selectors, two pointer slots and the compressed SRAM address format are reviewed. RX, TX, free and ready meanings, element layouts and lifetimes are unassigned. |
| In-process HCI handoff | LIVE | An affine split provides a `bt-hci::ExternalController` Host transport and one Controller-worker endpoint. Both bounded directions validate complete packets, apply async backpressure, wake on capacity/data, retain packets across short buffers and leave queues unchanged when waits are cancelled. Packet kinds remain typed; no UART/H4 framing or allocator is used. |
| Packet TX/RX | ABSENT | No BLE buffer publication, radio command, completion owner, RSSI result or on-air packet path exists. |

## LE Controller and HCI

| Capability | Status | Current production boundary |
| --- | --- | --- |
| Direct Test Mode | ABSENT | No transmitter/receiver test command reaches radio hardware. |
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
| Executor-neutral controller core | PARTIAL | The affine bootstrap worker has ordered async stop, retains accepted responses across backpressure/cancellation and rejects Host data before LL readiness. A lock-free pending/marked cell now preserves the reference scheduler event's coalescing contract without an RTOS, but no registered waker, scheduler-list drain, timer, Link Layer or radio event input exists. |
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

1. software event/list initialization and the exact static storage layout;
2. baseline/NRT interrupt meanings, shared same-core ISR ownership, typed
   primary/NRT routing and bounded acknowledgement/wake/re-arm without lost
   work;
3. RX/TX/free/ready memory-list roles and element ownership;
4. radio scheduler command, timer, doorbell and completion ordering;
5. 1M PHY channel, whitening, CRC, access-address and TX/RX result setup;
6. powered rollback and last-owner PHY shutdown.

Until those inputs exist, production must retain the current fail-closed
typestates and must not expose Controller, Link Layer, HCI or on-air readiness.
