# ESP32-S31 Bluetooth controller boundary

Verdict: **INCOMPLETE**. The reviewed public ESP-IDF glue proves the outer
lifecycle order and separates several platform operations from Controller
software. It does not publish the BLE Link Layer, scheduler command meanings,
packet-memory layouts or interrupt meanings implemented by the controller
libraries. Those gaps still block an on-air open Controller.

This review answers a narrower question than vendor equivalence: which parts
of the vendor lifecycle are requirements of ESP32-S31 silicon, and which parts
are implementations that the open Rust Controller must replace. Copying the
whole vendor init sequence would import its FreeRTOS/NPL architecture and would
not establish hardware correctness.

## Pinned public inputs

All sources are from ESP-IDF commit
[`aeab6dcfbeb44aba4b1f8ed102e3086172833153`](https://github.com/espressif/esp-idf/tree/aeab6dcfbeb44aba4b1f8ed102e3086172833153).
The hashes cover the complete raw files used for this review.

| Public ESP-IDF source | Relevant lines | SHA-256 |
| --- | --- | --- |
| [`components/bt/controller/esp32s31/bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c#L78-L145) | shutdown, disable and deinit order | `a7eb5a77c543b244757c08cc595960911c7aff73cae16eb7bd78503322d9f76d` |
| [`components/bt/controller/esp32s31/bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c#L168-L350) | top-level init, enable and rollback order | same file/hash as above |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c#L350-L469) | module clocks, MAC reset, LP timer, PHY and BTBB ownership | `1d681581eedcf5aa23b8462a148bd377a92edae676f941d9318a1e8947bf7585` |
| [`components/bt/porting_btdm/controller/ble/src/ble.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/ble/src/ble.c#L613-L756) | BLE controller, BB callback, packet pool and address setup | `ed793921668c5276e4726a506dbeca2d2ca4ae453f1d6b4c3b4943d7c1c7f279` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_coex.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_coex.c#L40-L235) | standalone coexistence defaults and optional shared-radio hooks | `da92929e125a830553d6d251af301ab2fbdfb6aef31d1342e9d14b483bdac1f9` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_external.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_external.c#L29-L43) | empty S31 external-init wrapper | `843a19181e1f4e25d3d9bdc924b2ec0c112ddee4f5c133c428a4ff95a62e5916` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_log.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_log.c#L115-L129) | empty default log-init wrapper | `952b6cd74004dea4254f615c93298bfd392fff4586820de6231916a0fe107ac5` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L84-L291) | FreeRTOS event queues and ISR wake behavior | `436d009f67fbe7c83fa367a6b80dbf87316727605e751febc03c5118a669b5c8` |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L1084-L1093) | vendor task creation | same file/hash as above |
| [`components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L1107-L1137) | level-3 interrupt allocation, optional IRAM flag and pinned-core routing | same file/hash as above |
| [`components/soc/esp32s31/include/soc/interrupts.h`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/soc/esp32s31/include/soc/interrupts.h#L139-L153) | Bluetooth MAC CPU sources 124 and 133 | `1a4f155b87090376b1a40ac62e19de344c7f10dc53d9b4451b66d545e9e4717d` |
| [`components/bt/porting_btdm/transport/src/hci_transport.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/transport/src/hci_transport.c#L23-L103) | HCI command/ACL/event routing | `db3fea7daa8d313e16f570eac6dc7e87a860edf03e896af49cce9d5dd7d13b45` |
| [`components/bt/porting_btdm/transport/src/hci_transport.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/transport/src/hci_transport.c#L214-L291) | transport selection, callback registration and teardown | same file/hash as above |
| [`components/esp_hw_support/port/esp32s31/Kconfig.mac`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/port/esp32s31/Kconfig.mac) | default two-universal-address policy and Bluetooth final-octet offset | `7b2e44be28d7b51e459ca5a407bf33430c543849836cfa479f2657d7e9fbdd10` |
| [`esp32s31-bt-lib/libbtdm_common.a`](https://github.com/espressif/esp32s31-bt-lib/blob/7f20740dd66ee774ffce5db0b55507892551aa31/libbtdm_common.a) | exact public S31 BTDM object code used to split composite HAL/task boundaries | `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099` |

The exact public ESP32-S31 controller-library revision is additionally pinned
by the target project README. Public glue proves calls and order, not the
internal effects of binary `r_*` entries. Reviewed Blobray facts remain the
evidence source for those internal MMIO transactions.

Reviewing the complete HAL-init body exposed and corrected one pre-existing
register-model error: the words at `0x2010_134c` and `0x2010_1350` contain
eight observed four-bit lanes each, not eight dense two-bit lanes. Each
iteration first clears nibble mask `0xc`, then performs a fresh read and ORs
`1 | ((global_index & 3) << 1)`. Bit one can retain a pre-existing one, so the
operation is not a complete nibble assignment. The reviewed register source,
SVD, raw PAC and bindings now preserve that exact positional geometry without
inventing lane meaning.

The same review now closes the HAL body's runtime-input question. Complete
`r_btdm_task_init` constructs positional inputs `16`, `11`, `33`, `2000` and
`0x2f00_0000`; the complete setter maps period images `500/1000/2000` to a
selector in the same internal object, and the complete helper uses that
selector to scale the two bytes. For the standalone caller the resulting
hardware images are sleep-timer shift `3`, byte zero `22`, byte one `66` and
SRAM prefix `0x2f00_0000`. The restricted PAC represents these accepted
domains directly and executes the complete 50-operation MMIO body. It does
not reproduce the vendor object layout, task, event primitive or IRQ allocator.

The next complete task-initialization component is current symbol
`r_sym_bt_AJP6s1I6EhaAHnz8n72F`. Structural review finds no MMIO in its
complete 182-byte archive body. It calls three software environment helpers
(one reads the public eFuse MAC and reverses its six bytes), clears 48 bytes of
HCI state, initializes a vendor mempool with 272-byte elements, registers
numeric broker source four and initializes a software list head. These are
open-Controller storage, address and dispatch policy rather than silicon
transactions. The ESP-HAL platform lease now reads the factory base identity
through its safe eFuse accessor and applies the pinned S31 two-universal-address
policy: Bluetooth is the next final-octet value after Wi-Fi STA. It retains the
result in canonical EUI-48 order; the generic HCI bootstrap performs the
observed single six-byte reversal only when encoding `BD_ADDR`. The Rust
replacement therefore binds the already initialized scheduler epoch to one
bounded `LeControllerHciResources` owner instead of copying the private
environment, pool or callback node. This state exposes only the conservative
bootstrap dispatcher—not operational Link-Layer HCI.

The interrupt boundary contains three CPU routes. The primary Bluetooth MAC
route uses source 124, level 3 and an IRAM-capable handler request. The modem
low-power timer uses source 127 with the same level and residency requirement.
The non-real-time (NRT) Bluetooth MAC INT1 route uses source 133 and level 3
without requesting IRAM placement. The public OSAL installs all three on the
configured Controller core. The primary setup clears and enables
baseline groups `0x0000_8000` in bank 0 and `0x0000_1300` in bank 1 before the
output strobe. Its handler samples masked status; the NRT handler samples its
dedicated snapshot status. Both acknowledge through the same two W1C clear banks. The restricted
PAC therefore exposes a transition that stages a single shared register owner
before either CPU route is bound and preserves the distinct snapshot modes.

This closes route identity, level, residency policy, baseline masks and the
setup/teardown prefix. The complete primary suffix review now structurally
classifies dynamic groups `0x1820_0000` and `0x0000_0008`, including bank-one
precedence, two distinct scheduler-state observations and the sticky coalesced
work marker. Individual bit names remain positional. The scheduler words are
now transferred with the affine interrupt-register partition, so the hard
handler can preserve the exact read/optional-clear/read order without a
task-side MMIO alias. The public OSAL and complete producer/consumer review
prove that this scheduler work is intentionally coalesced and drained through
a software intrusive completion list, not a hardware FIFO. The default BLE
callback graph further proves that selector 4 drives conditional scheduler
retry and selector 6 drives a post-clear transaction/list consistency action.
The callback registry and list-node layout are vendor software architecture,
but those two effects must have typed open replacements. The distilled
contract and RTOS-free replacement boundary are recorded in
[`bluetooth-interrupt-runtime.md`](bluetooth-interrupt-runtime.md). The
pinned `esp-pacs` revision now names source 124 as `BT_MAC` and source 133 as
`BT_MAC_INT1`. The ESP-HAL adapter compile-checks both identities against the
chip policy, requires level-three handlers, binds the complete pair on one
core and disables both through the same retained core identity. Those
primitives stay crate-private until the staged ISR owner, feature-specific NRT
policy, typed selector-4/6 actions, open completion queue and lost-wake-safe
waker can make a lossless live epoch. The
baseline groups are no longer opaque completion candidates: the complete
source-124 prefix proves four fault/assert lanes, and the restricted PAC
performs their conditional diagnostic reads at the reviewed temporal points
without exporting the opaque words. Bluetooth policy then selects fail-stop
over any simultaneous scheduler wake.

The BLE PHY environment is an address-owned hardware object, not a vendor
configuration ABI. Complete allocator `r_sym_ble_fkNcDe7YmOY2sALPjoRj`
requests one `0x68`-byte environment, then stores separately zeroed allocations
of `0x28`, `2 * 0x04` and `0x04` bytes through pointer fields `+0x30`, `+0x34`
and `+0x38`. Current archive xrefs observe ordinary fields through `+0x38`,
while `r_sym_ble_xxEI8lxgDQ94OX1GWAuO` fills the sixteen-byte region
`+0x40..=+0x4f`; the register-init body separately publishes the `+0x2c`
compressed member and the full `+0x40` address. The open memory owner therefore
replaces those allocations with one zero-based static graph and installs only
the three proven internal pointers. Resolving-list setup calls
`r_os_mempool_init` with element size `0x40`, obtains one block, writes the
initial `0x80000000` head word and publishes that block through the positional
global pair. The first open owner reserves one such opaque hardware object;
this is address/extent evidence only, not a privacy or resolving-list semantic
claim.

## Reference Controller implementations

These inputs are architecture references, not ESP32-S31 register evidence.
They answer which software layers a complete open Controller actually needs
and which apparent Rust integrations still terminate in closed firmware.

| Implementation | Actual boundary | Decision for this driver |
| --- | --- | --- |
| [Trouble 0.8.0](https://docs.rs/crate/trouble-host/0.8.0/source/README.md) | Async Rust BLE **Host** above HCI. It explicitly requires a Controller implementing `bt-hci`; its ESP32, nRF SDC, Pico W and STM32WB examples do not make those Controllers open. | Reuse after the open HCI Controller is real. Do not put timing, Link Layer or ESP32-S31 register policy in Trouble adapters. |
| [`bt-hci` 0.10.1](https://docs.rs/crate/bt-hci/0.10.1/source/README.md) | Packet types and Host-side traits for the standardized HCI boundary, not a Link Layer. The legacy Transmitter Test v1 declaration in this release uses the LE Read Supported States opcode; the correct v2 command types are unaffected. | Keep as the direct in-process Controller contract. Do not use the defective v1 transmitter type or fork the crate: production uses typed v2 commands and the Controller retains its standard-opcode v1 decoder for external Hosts. `bt-hci` cannot supply radio scheduling, packet ownership, ACL credits or LL procedures. |
| [Zephyr open LE Controller](https://github.com/zephyrproject-rtos/zephyr/blob/08e13d28c3aed050b9742ea28d5d573f260c13a7/doc/services/connectivity/bluetooth/bluetooth-ctlr-arch.rst) | Vendor HAL, soft-real-time ticker, hard-deadline LLL, ULL roles/control procedures, HCI and fixed/lockless utility queues. Its documented priority model keeps radio-event handling above preparation, completion, role management and Host work. | Adopt the HAL/LLL/ULL/HCI decomposition and explicit prepare/abort/done flow. Replace Zephyr kernel threads and Mayfly contexts with typed ISR frontiers plus one executor-neutral async owner; do not move inter-frame deadlines into ordinary futures. |
| [Apache NimBLE](https://github.com/apache/mynewt-nimble/blob/1e8ed60276f35a80ed4d4b4f8bb9d9c6fee53845/README.md) | A complete open Host and Controller with distinct `controller`, radio `drivers`, HCI `transport` and OS `porting` directories. Its published Controller hardware support does not include ESP32-S31. | Use its controller-only split, LL/HCI semantics and conformance tests as behavioral references. Do not port its NPL/event-task ABI or Nordic/Renesas radio shims as an S31 HAL. |
| [Pinned `esp-radio` BLE connector](https://github.com/ermacv/esp-hal/blob/7f914782f5c689a74f37df83b89a0827d3800323/esp-radio/src/ble/controller/mod.rs) | Async `bt-hci` facade over BTDM initialization and symbols supplied by the `esp-wifi-sys` binary packages. | Useful interoperability and lifecycle evidence, but not a source-open Controller. Replacing only its connector or OS adapter would leave the closed Link Layer intact. |

The resulting unit stack is larger than PAC -> HAL -> Link Layer -> HCI. The
minimum dependency graph and publication gate for each unit are:

| Unit | Responsibility | Current S31 state | Exit criterion |
| --- | --- | --- | --- |
| Register evidence and SVD | Address, field, access and transaction provenance | Partial but substantial; generated model is fail-closed | Every register used by the first vertical slice is reviewed and generated with no raw-address escape in upper layers. |
| Restricted PAC | Affine peripheral ownership and finite MMIO operations | Cold ownership, scheduler prefix, full 50-operation HAL body, generated BTDM runtime-timer start, memory-pointer geometry, IRQ prefixes and the ISR scheduler read/clear plus worker finished-list mask transfer exist | DTM trace uses only typed operations and has exact rollback/quiescence edges. |
| Platform/HAL lifecycle | Clocks, reset, controller HAL, scheduler prefix, PHY, BTBB, BLE PHY, interrupt matrix, entropy/crypto and coexistence leases | Clock/reset consumes into the complete 50-operation controller HAL state; only that state reaches the scheduler-table prefix, preserving the recovered `r_btdm_task_init` hardware order. The reviewed standalone selection mints one private affine zero-sized always-awake policy marker beside its time scale. That marker remains nested through scheduler, HCI, target PHY registration and client acquisition, BTBB, BLE-PHY, Controller-output/timer start and stable interrupt-owner publication; by itself it performs no MMIO and grants no RF-ready or controller-time authority. Once the same owner also retains the settled Bluetooth PHY client and completed BLE-PHY transaction, a later completed generation-keyed time request through the disabled-sleep policy mints one opaque non-`Copy` RF-ready token without new RF MMIO. The independently bounded modem-timer queue, source-owned scheduler timeline and bounded HCI bootstrap resource epoch remain affine with that powered scheduler owner. Scheduler/HCI splitting returns one powered task endpoint that joins the sole mutable timeline and workers to the exact task-side HAL owner; its finite lock/modify step cannot export register access. The next transition borrows that Controller's platform and shared-PHY HAL for one concrete target registration, and only terminal target success mints `RegisteredBluetoothPhy`. Bluetooth-client acquisition remains a separate affine edge with settled, pending-tracking and in-flight-tracking outcomes. BTBB is reachable only from the settled client owner; terminal registration, acquisition or tracking failure retains the complete outer Controller fail-stop, with a poisoned lower owner after tracking failure. Cancellation of in-flight tracking instead drops the unique epoch and requires out-of-band hardware reset. The finite BTBB and BLE-PHY register transactions then retain that client and the static allocation graph through Controller-output preparation, timer start and stable interrupt-owner publication. The production target composition now binds the live route epoch, executor notifications, Controller actor and Host facade. The standalone token still proves only the reviewed disabled-sleep RF-ready timing edge; it does not prove a sleep-enabled warm wake, operational on-air BLE readiness, last-client release or HIL. | One owner can power and route all three IRQs; a quiesced return to cold remains the exit criterion. |
| Controller timer and scheduler | Radio timebase, prepare/abort/doorbell/completion and collision policy | The always-awake latch request/self-clear/read order has a live private generation-keyed worker retained by the powered Controller. The first affine Pending borrow yields Waiting or a non-decomposable exact-Controller proof whose sample initializes the persistent scheduler epoch. The retained epoch can then reborrow the same Controller for repeated fresh-current requests; only current Ready preserves the prior forward scheduler image while moving the raw anchor to the exact new sample. A distinct request through the terminal powered BLE-PHY owner consumes its completed sample into an opaque RF-ready token without reanchoring. Private phase states enforce initial TX/RX current then RF-ready, recurring RX RF-ready then current and recurring TX current only. Initial paths next acquire a private admission sample, reserve and resolve overlap, and only then publish a private sequence request; recurring paths reserve before their sole private sequence request. Samples and RF-ready images remain private, and all cancellation/Drop paths release an exact retained reservation before late-result drain. The reviewed standalone margin 107 is retained only by the source-owned scheduler config; it has no public constructor, image or preparation argument. Published lifecycle code can recover the retained typestate without exposing epoch data. Mismatch faults the worker and no non-Ready path reanchors. The DTM completion path can capture one finished-list transfer and then consume every retained remainder one observation per Controller call on the exact running or completion-observed owner, without another capture or MMIO transfer. Unrelated affine list tokens remain available to their actual dispatcher, a repeated list-zero token is retained fail-stop, and hardware-head retirement remains closed until the retained drain is inactive. The DTM session actor composes this continuation through recurrence and Test End; it is not yet a general LL role dispatcher. The next Controller edge atomically unlinks the empty-head graph and arms a globally identity-branded, checked-generation capacity-one mailbox; exhaustion rejects before unlink. Every public primary service serializes capture/acknowledgement, both ordinary durable publications and mailbox routing under the same critical section. The exact first post-arm event is retained with the affine unlinked owner; the internal pair has no public constructor, standalone unlink or primary-service bypass. Equal generations in different Controller mailboxes cannot cross-wire take, cancel or re-arm. No-work and command-pending outcomes re-arm the same identity and generation before returning. Ordinary wake dispositions are emitted only by immediate primary service and are not repeated by late consumption. Using source 124 is conservative rather than vendor-mandated: direct vendor BUSY/command rechecks prove neither interrupt causality nor a retry wake. A full mailbox also returns rather than retains a later event, so a command-ready edge can precede re-arm and leave Pending without guaranteed progress. The disabled-sleep producer requires no RF MMIO; the sleep-enabled wake helper contract, a proven wake/recheck source, physical counter contract, actual vendor task-run callback and complete scheduler command/status semantics remain absent. The production composition provides the bounded DTM HCI actor and autonomous recurrence, but not general LL scheduling. | Virtual-time model plus register trace proves one scheduled event, cancellation and late/error handling. |
| Packet memory dataplane | Static RX/TX/free/ready storage, DMA visibility and backpressure | A dedicated controller-memory crate now owns the complete no-heap per-event DTM allocation footprint, the sole TX backing slot, RX/TX extent/header geometry, paired ordinary RX re-arm/result parsing and normal scan/non-scan global-list routing. Production integration owns one unique `.dma.bss` arena; target binding derives real field addresses, validates the complete physical-SRAM extent before mutation and gives a movable CPU owner one non-movable static allocation with exact private links. The separate DTM environment remains LLL state. The DTM private TX-head/RX-tail descriptor path, unconditional entry into the append decision and exact two-header swap-reserve rotation are mapped. Descriptor publication and completion visibility are device-fenced, and the graph advances through affine CPU-owned, head-published, running, completion-observed and reclaimed states. Remaining gaps are exact hardware-consumed field and graph-traversal semantics plus on-air/HIL evidence. | Every buffer has an affine CPU/hardware state and is reclaimed on success, error, abort and shutdown. |
| Lower Link Layer (LLL) | Channel/whitening/CRC/access address, hard ISR turnaround and one radio-event state machine | The DTM radio-event owner composes typed lowering, scheduler publication, completion, recycle and recurrence. Remaining descriptor semantics and on-air evidence keep DTM partial; advertising, scanning and connection roles do not exist. | DTM TX/RX works first; then one advertising event executes without executor-latency dependence. |
| Upper Link Layer (ULL) | Advertising/scanning/connection scheduling, SN/NESN/retry, supervision and LLCP | Absent | Legacy advertising, scanning and one peripheral connection pass deterministic virtual-clock tests and HIL. |
| HCI Controller | Command/event table, capability reporting, ACL flow control and completed-packet credits | Bounded transport/bootstrap, DTM Controller-side commands and the canonical public-address platform input are bound after scheduler initialization to the same powered actor. General LL commands, events and ACL flow control remain absent. | Only implemented LL features are advertised; all supported commands and ACL paths reach owned ULL state with bounded cancellation-safe queues. |
| Host and application | L2CAP, ATT/GATT, GAP/SMP and application policy | Trouble bootstrap works in host tests only | Trouble Runner and a bounded GATT peripheral run through the same production Controller session runner. |
| Qualification | RF, protocol, concurrency, soak, teardown and negative evidence | The machine-checked Bluetooth LE qualification manifest remains incomplete | Dated HIL and conformance cells close each capability independently; interoperability alone is insufficient. |

This order is bottom-up for evidence and hardware enablement, but interfaces
are developed with vertical slices. DTM is the first slice because it closes
PAC/HAL/timer/dataplane/LLL/HCI without prematurely implementing GAP or a
connection. Legacy advertising is next, then scanning, one peripheral
connection, ACL flow control and mandatory LLCP. Encryption, privacy, central
role, extended features, low power and concurrent Wi-Fi remain independent
later slices.

## 1. Four-way classification

Each observed step is assigned to one of four classes:

1. **silicon-required**: clock, reset, PHY, baseband, timer, IRQ or memory
   transaction that an independent Controller must reproduce;
2. **open-controller replacement**: protocol, scheduling, queueing or HCI
   behavior that belongs in the Rust Controller rather than the HAL;
3. **profile-optional**: power, coexistence, diagnostics or extended feature
   work that can be absent from the first declared standalone profile;
4. **unresolved**: the public glue identifies a boundary, but current evidence
   cannot safely split hardware requirement from vendor implementation.

The classification is deliberately about ownership. A vendor function may
contain work from more than one class and must then be decomposed before any
production transition calls it equivalent.

## 2. Init/deinit classification

The public top-level init order is OSAL pool, log, external hooks,
coexistence, clocks/reset, BTDM task, low-power state, HCI flow-control state,
BLE stack and HCI transport. The open lifecycle must preserve only the
hardware dependencies in this order; it must not reproduce the software
container around them.

| Vendor step | Class | Open-driver decision |
| --- | --- | --- |
| `ble_osal_elem_calc`, `btdm_osal_elem_mempool_init` | open-controller replacement | Do not port the object pool or FreeRTOS layouts. Use statically bounded Rust-owned queues, packet slots and intrusive/free-list state with explicit lifetimes. |
| `btdm_log_init` | profile-optional | The default S31 wrapper is empty. Diagnostics may be added through compile-time logging without becoming a radio prerequisite. |
| `btdm_external_init` | profile-optional at this level | The S31 wrapper is empty. Concrete BLE callbacks such as randomness or cryptography must be introduced only by the feature that consumes them. Heap/allocation callbacks are not required by silicon. |
| `btdm_coex_init` | profile-optional for standalone; required for joint radio ownership | With software coexistence disabled, the public implementation is a set of no-op/default adapters. The first profile may own the radio exclusively and implement that declared standalone contract. Wi-Fi plus BLE requires a later shared arbiter and HIL. |
| `btdm_lp_enable_clock` | silicon-required | Acquire the BT and BT-APB clock dependencies, pulse the BT MAC reset and establish the selected low-power timer source. These operations belong to platform/HAL typestates. |
| `r_btdm_task_init` | unresolved composite | Do not port the task. Reviewed binary evidence shows finite hardware scheduler/IRQ initialization inside the boundary, mixed with event/list and OSAL setup. Recover and publish those transactions as separate HAL operations before replacing the task with the async Controller owner. |
| `btdm_lp_init` | split | Publishing the measured low-power clock frequency to controller timing is silicon/controller-HAL work. FreeRTOS tickless callbacks, PM locks, light-sleep retention and wake registration are optional low-power features and are out of the always-awake profile. |
| `r_btdm_hci_fc_env_init` | open-controller replacement | HCI ACL credits and completed-packet accounting are Controller state. Implement them in bounded Rust queues; no vendor flow-control environment is needed. |
| `ble_stack_init` / `r_ble_controller_init` | unresolved composite, predominantly open-controller replacement | This is the closed BLE Controller: Link Layer, scheduling and command/event behavior must be replaced. Only separately recovered register, IRQ, timer and memory transactions may move into HAL. Completion of this vendor call is not an acceptable open typestate. |
| `esp_ble_register_bb_funcs` | unresolved | This callback table may bridge hardware/platform helpers into the closed Link Layer. Recover call sites and effects before deciding which entries are silicon-required. Do not clone the ABI as the open architecture. |
| duplicate-scan cache and public-address setup | open-controller replacement | These are Link Layer/HCI state. The first DTM slice needs neither; later advertising/scanning owns address and duplicate-filter policy. |
| `ble_msys_init` | open-controller replacement | Vendor mbufs are transport/storage policy. Replace with bounded, typed packet ownership shared by LLL, ULL and HCI. |
| `hci_transport_init` | open-controller replacement | UART/VHCI selection and callback registration are not radio requirements. The open in-process `bt-hci::ExternalController` channel is the native path; an H4/UART adapter can remain optional. |

Deinitialization is the reverse ownership proof, not merely a list of cleanup
calls. The current open path must remain fail-stop after the first unpaired
powered mutation until IRQ quiescence, scheduled-command cancellation, packet
reclamation, BTBB release, PHY release and clock release are all proven.

## 3. Enable/disable classification

The public enable path first executes `btdm_lp_reset(true)`. On S31 that
enables the Bluetooth PHY client and only then enables the shared BT baseband.
This establishes a mandatory silicon order:

```text
clock/reset owner
  -> target-registered common PHY
  -> settled Bluetooth PHY client
  -> finite BTBB transaction
  -> BLE radio-engine initialization
```

The corresponding disable path first stops the Controller software, then
disables BLE/coexistence, releases BTBB and finally releases the PHY client.
An open implementation therefore needs a quiesced hardware state before either
shared hardware lease can be dropped.

| Vendor step | Class | Open-driver decision |
| --- | --- | --- |
| `esp_phy_enable(PHY_MODEM_BT)` | silicon-required | Run the common PHY registration through borrowed target capabilities, retain the target-issued `RegisteredBluetoothPhy`, then acquire the Bluetooth client explicitly. Immediate tracking, when due, must finish through the target tracking runner before the owner is settled. A returned tracking failure poisons the PHY epoch while retaining the outer Controller fail-stop; cancellation drops the unique epoch and requires out-of-band hardware reset. Periodic tracking and exact last-client release are still missing. |
| `esp_btbb_enable()` | silicon-required | Run the reviewed finite first-user BTBB transaction only from the settled `RegisteredBluetoothPhyClient`. Registered-only, pending, tracking and poisoned states must not cross this gate. This finite transaction is not a complete common-PHY/baseband or radio-readiness claim. |
| `btdm_coex_enable` | profile-optional for standalone | A no-op success is source-correct only while the product contract excludes simultaneous Wi-Fi. |
| `ble_stack_enable` | unresolved composite | Replace protocol activation; recover any remaining radio-engine activation transaction separately. |
| `r_btdm_hci_fc_enable` | open-controller replacement | Start a fresh bounded HCI credit epoch in Rust. |
| `r_btdm_task_enable` | split, still incomplete | The hardware-only 50-operation BTDM HAL-init body, exact baseline fault masks and diagnostic capture, controller-output strobes, generated runtime-timer start command, typed three-route ESP-HAL primitives, affine ISR scheduler MMIO, dynamic scheduler classifier, RTOS-free coalesced wake cells and pure controller-time latch/epoch phases are recovered as separate contracts. One affine Controller owner now composes the fixed scheduler timeline and low-power hardware with borrowed target common-PHY registration, explicit Bluetooth-client acquisition and terminal initial tracking. Only a settled client can cross the finite BTBB gate and remain retained through BLE-PHY register initialization, Controller-output preparation, runtime-timer start, stable interrupt-owner publication, the three finite interrupt dispositions and durable source-124/source-127 handoffs. Primary capture, both ordinary cell publications and globally identity-branded capacity-one post-unlink mailbox routing now share one Controller serialization boundary; ordinary wakes are exposed only by that immediate service result. Production target composition binds the live-route epoch, executor notifications, DTM Controller actor and Host facade. Registration and tracking failures retain the Controller fail-stop. Periodic client tracking, client/BTBB/PHY teardown, unrelated-list and source-127 expiration consumers, feature-specific NRT policy, command-ready retry liveness under mailbox saturation, physical controller-time proof, typed selector-4/6 actions, on-air BLE readiness and HIL remain unresolved. |
| `r_btdm_task_disable` and `ble_stack_disable` | unresolved composite | Define a stop barrier: mask sources, cancel/abort commands, acknowledge residual status, reclaim every packet, then expose a quiesced owner. |

The public OSAL demonstrates that the vendor implementation uses FreeRTOS
queues, ISR variants and an RTOS task. This is evidence about its software
architecture, not evidence that ESP32-S31 requires an RTOS. The hardware
contract is the ordered interrupt/status/timer/memory interaction underneath
those adapters.

## 4. Target open architecture

The target path is:

```text
SVD / restricted PAC
  -> shared clock, reset, PHY and BTBB leases
  -> BLE Controller HAL: primary/timer/NRT IRQs + scheduler + packet memory + radio command
  -> Lower Link Layer (hard-deadline radio-event engine)
  -> Upper Link Layer and LL control procedures
  -> bounded HCI command/event/ACL worker
  -> bt-hci::ExternalController
  -> Trouble Host
```

The sole Controller owner is async and executor-neutral. Both same-core,
level-3 handlers serialize access to one staged clear-bank owner; each handler
only acknowledges/classifies a bounded hardware snapshot and wakes the async
owner. Hard
radio deadlines, including inter-frame turnaround, stay in the hardware
scheduler and the minimal lower-link-layer interrupt path; they are not
delegated to arbitrary executor latency. No mutex shared with the Host may be
held across a radio deadline.

Trouble starts above HCI. It can validate and consume the Controller boundary,
but it cannot replace the missing PHY, scheduler, packet engine or Link Layer.
The existing software bootstrap proves transport/API compatibility only.

## 5. Evidence-bounded route to first on-air proof

Direct Test Mode is the first physical vertical slice because it exercises the
radio without advertising state, connection timing, ACL flow control or LLCP.
The exact S31 command-to-scheduler path, the initial link-state image and the
separation between scanner resume and scheduler consistency are recorded in
[`bluetooth-direct-test-mode.md`](bluetooth-direct-test-mode.md). In
particular, selector 4 is now known to be scanner-role software and is not a
DTM prerequisite. Selector 6 validates the vendor's private intrusive
transaction container and therefore has no runtime successor in the affine
open scheduler.
The implementation order is:

1. finish restricted PAC access for the remaining scheduler command/status
   words and memory-list lifecycle; the three positional selector pairs now
   have exact current/next RX slot roles and compressed-pointer geometry,
   while the controller-memory layer assigns selector one to normal scan
   insertion and selector two to admitted non-scan insertion. DTM deliberately
   bypasses both and publishes its private TX head/RX tail through its link
   state. The
   conditional lock/modify request already has exact images, predicate,
   diagnostic publication-result projection and affine event phases, but safe
   admission still requires the merge-selected item/list owner. The
   controller-time latch has exact always-awake request/self-clear/read phases
   and a pure epoch projection. Both interrupt
   snapshot modes, baseline setup/teardown masks, route identities, policies
   and typed ESP-HAL pair binding, dynamic scheduler classification, affine
   ISR scheduler MMIO and sticky coalesced wake state are finite components.
   Stable-owner primary/NRT dispatch and serialized source-124 ordinary plus
   identity-branded post-unlink mailbox publication are composed. Atomic
   unlink-and-arm prevents
   a pre-arm event from entering the DTM gate, but command-ready causality and
   retry liveness after mailbox-full delivery remain unproven. Live handler
   notification and feature-specific NRT policy are still absent, so this is
   not a live interrupt epoch;
2. recover a hardware-only init transition from the composite task/BLE init
   functions, with read-back or bounded postconditions and exact rollback up
   to every fallible edge;
3. recover the remaining hardware-consumed DTM fields and hardware traversal
   semantics of the published item-to-link-state-to-private-links chain while
   preserving the existing visibility fences and affine CPU/hardware ownership
   transitions;
4. route both real Controller interrupts into the shared staged owner, define
   the synchronous hard-handler versus async-bottom-half boundary, and prove no
   lost or duplicated work across mask, acknowledge, wake and re-arm;
5. implement one scheduled 1M transmitter-test command, then receiver-test,
   then test-end/result collection;
6. expose only the matching LE Transmitter Test, LE Receiver Test and LE Test
   End HCI commands through the existing dispatcher;
7. qualify register traces first, then HIL frequency/channel/PDU/count results,
   and keep the qualification manifest fail-closed until dated evidence exists.

The complete controller HAL component now consumes the clock/reset owner, and
only its terminal affine state can execute the sixteen-entry scheduler-table
low-bit clear. This matches the recovered hardware order inside
`r_btdm_task_init`. The following consuming HCI transition replaces the
reviewed vendor environment, packet pool and broker source with one bounded
Rust epoch and can only split scheduler plus HCI endpoints together. Its safe
successor now executes the source-127 register prefix and complete low-power
hardware component, then retains the separated timer owner without publishing
ISR storage or enabling a CPU route. The scheduler clear remains only an observed initialization prefix: it neither
proves the meaning of those entries nor establishes that vendor event/list
objects are hardware requirements. Neither state may be promoted to
`ControllerInitialized` until the subsequent hardware command, IRQ and storage
contracts are known.

After DTM, the next growth order is legacy non-connectable advertising,
scanning, connectable advertising plus one peripheral connection, bounded ACL
flow control, mandatory LL control procedures, and only then encryption,
privacy, central role, extended PHY/advertising, ISO, low power and concurrent
coexistence. Each step adds HCI support only after its Link Layer owner is
live.

Legacy non-connectable advertising now has an independent Blobray review scope,
`ble-legacy-nonconnectable-advertising`. Its frontier deliberately does not
inherit DTM packet state: the DTM access address, channel image, timing policy
and descriptor field meanings are role-specific observations. The advertising
scope follows current linked PDU builders, role reset/start, first/next primary
event scheduling, recycling and asynchronous finished-list delivery.

The public `esp_ble_adv_aa_setting` entry is present only in raw
`libble_app.a(94.o)` for the pinned input. It combines its two 16-bit arguments
and stores the result at offset `0x30` of a software environment. This proves a
setter boundary only. It does not prove byte order at the packet engine, the
link-state/descriptor publication edge, or the hardware consumer. Keep those
as blockers and inspect the setter explicitly with:

```console
tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect function ble:esp_ble_adv_aa_setting \
  --artifact _oracles/libble_app.a --member 94.o --full --details
```

The next production admission gate requires current-artifact evidence for all
of the following connected edges: advertising access address and CRC init into
the hardware-consumed packet state; primary channel into both RF frequency and
whitening seed; PDU header, buffer and length publication; event timing/list
role; and completion status plus recycling. Until then the S31 implementation
may prepare and cancel a bounded PDU but must not publish it to hardware.

## 6. Blocking unknowns

The decisive gaps are not HCI packet syntax. They are:

- feature-specific NRT snapshot meanings plus their mask/re-arm ordering; the
  primary fault disposition and diagnostics, dynamic scheduler branch, source
  identities, level-3 policy, exact masks, coalesced marker contract, affine
  ISR scheduler MMIO and shared clear-bank prefix are no longer unknown, but
  live route composition and handler-to-executor notification remain absent;
- an affine open scheduler and bounded completion lifecycle; selector-4 is now classified as scanner
  resume and is deferred with the scanning ULL role rather than DTM;
- remaining scheduler command opcodes, any operational meaning of the
  lock/modify diagnostic result, the raw-status-to-finished-list mapping,
  completion fence and timebase semantics;
- the meanings of the three compressed-pointer RX-list selectors, their
  element layouts, hardware current-to-next rotation, alignment and ownership
  barriers;
- BLE packet-engine configuration for channel, whitening, CRC, access address,
  TX/RX mode and result/RSSI extraction;
- abort/cancel behavior and quiescence proof for powered teardown;
- the hardware/platform subset hidden behind BB callback registration and the
  composite task/BLE initialization entries.

Until those gaps are closed by reviewed public sources, fail-closed vendor
comparison or HIL, the repository can truthfully claim an open PAC/HAL
foundation and HCI-compatible software boundary, but not an open Bluetooth
Controller driver.
