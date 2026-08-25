# ESP32-S31 Bluetooth LE Direct Test Mode boundary

Verdict: **INCOMPLETE**. The complete ESP32-S31 DTM command bodies, their path
into the common scheduler, the finite lock/modify head-publication transaction
and the later scheduler-item recycle callback are now identified. They prove
that DTM is a small Lower Link Layer role built from controller-SRAM
descriptors, a common radio wake operation and an event-driven scheduler
lifecycle. They do not yet prove the complete descriptor layout, the hardware
status-to-finished-mask boundary, memory fences or RX buffer reclamation needed
for an on-air open implementation.

This review narrows the first physical vertical slice. It does not make DTM a
production capability and does not import the vendor scheduler, callback
registry, allocator or RTOS event model into the Rust architecture.

## Pinned public inputs

- ESP32-S31 Controller archive
  [`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31),
  `libble_app.a` SHA-256
  `62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`
  and `libbtdm_common.a` SHA-256
  `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  initial `libble_app.a` SHA-256
  `ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`
  and
  initial `libbtdm_common.a` SHA-256
  `bd9007072c6ab94df5f29d8b96dc65a69cb4406568c75a64022c8121e242b96c`;
- role-name reference only:
  [`espressif/esp32c61-bt-lib@c800514c39a3e491bb13bb224987e109623d2cf2`](https://github.com/espressif/esp32c61-bt-lib/tree/c800514c39a3e491bb13bb224987e109623d2cf2),
  `libble_app.a` SHA-256
  `78fd88e769ee48bf290bae3684df9ec8ea5c2f939396e4e50eb8a69f134ea6aa`.

All current-revision S31 behavioral claims below come from complete current
S31 bodies. The initial public S31 archive retains descriptive scheduler and
DTM symbols; its role names are accepted only where control flow, call order
and instruction extent agree with the current body. The C61 archive is used
under the same stricter role-only rule for BLE functions whose S31 history is
obfuscated. Neither older S31 code nor C61 supplies current register behavior
or ABI evidence. Object files and disassembly products remain temporary review
inputs and are not repository artifacts.

## Recovered DTM component

S31 member `55.o` identifies its own source role as
`linkedListMode/ble_lll_dtm.c`. Complete-body comparison establishes the
following useful map:

| ESP32-S31 body | Bytes | Recovered role and exact S31 behavior |
| --- | ---: | --- |
| `r_sym_ble_bWydXXPAXzjyon1EdAMg` | 122 | DTM interval calculation. Its size and body agree with the named `r_ble_lll_dtm_calculate_itvl` reference. |
| `r_sym_ble_7F349oHpjOqsP8rHzlIj` | 558 | Allocate and connect the DTM memory graph. Its callers and complete control/data flow agree with the named `r_ble_lll_dtm_alloc_memory` reference. The S31 body requests link-state kind 6, creates a scheduler item with kind byte 5, installs `r_sym_ble_kdHGLPeGDJlAvxmbjQ6e` as its recycle callback, attaches bounded RX/header storage and rolls every partial allocation back before returning its finite error code. |
| `r_sym_ble_VikJlxpO0kioDchKDFeI` | 278 | Reset one DTM controller-SRAM link-state image. It installs compressed links, a complete `0x71764129` access-address word, low 24-bit `0x555555` state and a rounded-power field. |
| `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` | 618 | Construct and schedule one DTM event. It accepts only role values one and two, wakes the common RF owner, constructs the timing image, begins a scheduler transaction and completes that transaction. |
| `r_sym_ble_XaQTq1AjtothrIFLgEeJ` | 362 | Construct TX context and payload. It accepts the reviewed payload-pattern selector domain, stores channel, length and PHY inputs, calculates the interval and repeatedly schedules TX role one until accepted. |
| `r_sym_ble_odiPxxFv9QenApESv5qy` | 166 | Validate and start a transmitter test, including channel `0..=39`, payload selector `0..=7` and PHY-dependent parameter checks before creating the TX context. |
| `r_sym_ble_x4t8591NiUiinayCMpjZ` | 116 | Construct RX context, store the channel/PHY inputs and repeatedly schedule RX role two until accepted. |
| `r_sym_ble_ssKOV2juzhIVJk3r8x6R` | 112 | Validate and start a receiver test, including channel `0..=39` and the accepted PHY selector domain. |
| `r_sym_ble_PptSRbXfefQwMVyO5jxP` | 52 | Process one received DTM buffer. From third argument offset `+0x04` it reconstructs a `0x2f00_0000 | (low20 << 2)` address. A zero compressed pointer takes the vendor fail-stop edge. The body returns `-1` when any low-24 bit of the word at returned-buffer offset `+0x0c` is nonzero; otherwise it copies the high byte at `+0x0f` into DTM link-state byte `+0x24` and returns zero. The byte meaning remains positional. |
| `r_sym_ble_kdHGLPeGDJlAvxmbjQ6e` | 336 | Recycle one completed DTM scheduler item. Complete control flow agrees with same-chip named `r_ble_lll_dtm_recycle_sch_item` (324 bytes). TX role one increments the DTM result count and reschedules only after zero item status; RX role two drains returned buffers, applies the RX-result callback, returns buffers to the bounded chain and reschedules while active. An unexpected role takes the vendor fail-stop edge. |
| `r_sym_ble_9DFKLYZzjaztWMiPU4NR` | 168 | End the test, serialize the 16-bit receive count, stop the common scheduler when a test is active, clear DTM active/count state and restore the default length state. |

The fixed `0x71764129` word agrees with the standardized DTM access address,
but its descriptor offset is still recorded positionally. Likewise, the
`0x555555` low-24-bit image is not promoted into a public `CRC_INIT` field
until an independent packet-engine consumer proves that field boundary.

The TX payload construction has no allocator requirement. The complete S31
body chooses finite repeated-byte images for the supported patterns and copies
the two fixed PRBS tables for the corresponding selectors. An open DTM role
can therefore use static, bounded packet storage. Reproducing the vendor
allocation and internal object layout is unnecessary.

The RX callback signature and result projection are also narrower than the
vendor memory manager. Complete S31 allocation and RX bodies, plus the named
C61 role comparison, establish that the callback's third argument is a buffer
header carrying a compressed returned-buffer pointer. The callback consumes
only the result word at `+0x0c`: low 24 bits form a fail-closed condition and
the byte at `+0x0f` is the only value copied into DTM state. This exact split is
published as `BluetoothDtmRxResultProjection`; no semantic name is assigned to
the high byte. Packet validity, length, RSSI and the buffer-reclamation edge
remain separate unknowns.

The generic S31 memory-management path further proves why the vendor heap and
callback registry must not become the open ABI. Its exact item-kind predicate
classifies kinds 8 through 10 separately, while the DTM allocation body writes
kind 5 into its scheduler item and link-state kind 6 into the allocation call.
The generic completion callback then follows compressed pointers into the
memory manager before recycling eligible storage. These are software object
kinds, not hardware memory-list selectors. An open implementation should
replace them with affine Rust states rather than preserving the integers or
the vendor object graph.

## Bottom-to-top execution path

The exact S31 path is:

```text
LE TX/RX Test command wrapper
  -> validate DTM parameters and allocate a bounded role context
  -> reset/build one controller-SRAM link-state descriptor
  -> wake the common RF owner and obtain current controller time
  -> build a mode-1 TX or mode-2 RX scheduler item
  -> delay around overlaps (`r_sym_ble_k8n...`)
  -> force insertion (`r_sym_ble_iHRq...`)
  -> common transaction finish (`r_sym_ble_2Nz...`)
  -> BTDM list transaction (`r_sym_bt_YRn...` or `r_sym_bt_VrTm...`)
  -> controller-window publication and later interrupt/completion handling
```

This is not a direct register-per-packet radio. The DTM code writes a
descriptor graph in the fixed controller SRAM window. The common scheduler
then owns publication and execution. `r_sym_ble_G4z...` contains no direct
MMIO; it invokes `r_sym_ble_4Qe...`, whose complete S31 wrapper first calls the
common BTDM RF wake path and then the controller-time provider.

The scheduler completion body `r_sym_ble_2Nz...` first checks timing through
`r_sym_bt_zpuz...`. The initial public same-chip object identifies those roles
as `r_btdm_sched_calc_seq_time`, merge-list removal and
`r_btdm_sched_insert_with_lock_modify`. The named historical
`r_btdm_sched_insert_with_lock_modify` body is 0x154 bytes; the current
`r_sym_bt_VrTm...` body is 0x156 bytes and retains the same control flow, call
sequence and register transaction. The current complete body directly uses:

- `0x2010_1218` as `SCHEDULER_LOCK_MODIFY_REQUEST`: bit 31 is START, bits
  19:0 carry the compressed SRAM pointer and bits 30:27 return a positional
  four-bit result;
- `0x2010_107c` bit 31 as `SCHEDULER_STATE.BUSY`;
- `0x2010_136c` through ordered clear/update operations that combine a
  low-four-bit argument with a value derived from the selected scheduler
  item;
- a second independent publication/wait when the selected list head changes.

Before and after request publication the current body waits only while both
BUSY and START are set. Once that conjunction is false, it publishes result
zero if BUSY is clear; otherwise it publishes request bits 30:27. That byte is
sent as broker-type-3 event one. It is not radio completion: the normal BLE
base scheduler subscribes to broker type zero, while the optional named
`gapFinder_stack_enable` subscribes to type three. Its current
`r_sym_ble_YX2xSKJJFZFRxyO5RlZC` callback only loads and logs the result for
event one, changes no scheduler or DTM state and returns zero. The exact
meaning of the sixteen diagnostic values remains unknown.

The restricted PAC now validates the pointer/argument, builds the two ordered
fresh-read argument images and the complete request image, and represents the
two-word decision observation without joining their physical owners. The
Bluetooth crate adds affine `awaiting publication` and `in flight` phases.
Each `observe` call evaluates exactly one fresh event and returns `Waiting` to
the executor when the conjunction remains active; it contains no polling loop,
allocator, waker or RTOS dependency. Its terminal value is consequently named
a publication result, not a completion. Live MMIO remains deliberately absent
until the task-side request owner and ISR-side state owner are composed without
stale cross-owner observations.

## Radio completion and ownership return

The later software lifecycle is a different broker path. Complete current S31
bodies plus same-chip named-role comparison establish this chain:

```text
finished hardware-list mask
  -> base scheduler broker event 0x8000_0004
  -> r_sym_ble_rmN... = r_sched_txn_onSchedHwListDone
  -> r_sym_bt_M9n... = r_btdm_sched_pick_finished_items
  -> software completed-item queue through item+0x54
  -> coalesced recycle bottom half r_sym_bt_uNi...
  -> r_sym_bt_WHY... = r_btdm_sched_pop_executed_sch
  -> r_sym_bt_QsLK... = r_btdm_recycle_process_dequeued_sch
  -> callback at item+0x58
  -> r_sym_ble_kdH... = r_ble_lll_dtm_recycle_sch_item
```

The current `r_sym_ble_rmNuzAO8kQQQXQIpTzGZ` body is 0x1ce bytes and agrees in
control flow and call extent with the 0x1ca-byte same-chip named
`r_sched_txn_onSchedHwListDone`. It walks the active scheduler list for every
set hardware-list bit and calls current 0x142-byte
`r_sym_bt_M9nG353V0svWrv1l1zGw`, mapped to named
`r_btdm_sched_pick_finished_items`. That common routine unlinks matching
finished items from the hardware-linked list and joins them to the software
completed queue through the intrusive link at `item+0x54`.

The coalesced bottom half is current 0x70-byte
`r_sym_bt_uNi9OHmE7XdXfGqTelU5`, mapped to named
`r_btdm_recycle_in_task`. It repeatedly uses current
`r_sym_bt_WHYoiw8ufY0AEM2KSRK1`, mapped to
`r_btdm_sched_pop_executed_sch`, then current
`r_sym_bt_QsLKLOCC2pct4rL8uFBN`, mapped to
`r_btdm_recycle_process_dequeued_sch`. The latter clears the item's
low-twenty-bit link image, reconstructs the associated controller-SRAM state
and invokes the callback stored at `item+0x58`. DTM allocation installs the
mapped DTM recycle body there.

This is the reference software edge at which a finished DTM item is handed
back to its role. It is the correct boundary for an affine
`HardwareOwned -> Completed -> CpuOwned` open lifecycle. It still does not by
itself prove which raw interrupt/status bits produce each hardware-list bit,
the memory fence required before callback reads or every buffer-return rule.
The open driver must establish those facts before making the Rust ownership
transition live.

The reviewed register model consequently promotes `0x2010_125c` to
`SCHEDULER_FINISHED_LIST_STATUS.FINISHED_LIST_MASK` and retains `0x2010_1260`
as the positional `SCHEDULER_FINISHED_LIST_REPORT`; it does not invent W1C
semantics for that second word. The restricted PAC preserves the exact
read/report order as one task-owned observation. The Bluetooth layer drains
the mask one lowest-numbered list per finite step, allowing the future async
bottom half to yield between lists without a loop or RTOS. Hardware-list to
affine-item mapping remains the next layer above that token.

The always-awake time-read prefix is now exact. Complete
`r_sym_bt_KrvfcwDw4eZoaTPVdFj5` sets `SLEEP_TIMER_CONTROL.LATCH_REQUEST` at
`0x2010_1090`, waits for hardware to clear that same bit and reads
`SLEEP_TIMER_LATCHED_TIME_0` at `0x2010_10ac`. Complete
`r_sym_ble_3ISuZaEAZjklAjtGLFxW` converts the delta from an owned raw-time
anchor into the BLE scheduler epoch, handling either side of the anchor and
rounding the negative side by one when a discarded remainder is nonzero. For
the reviewed standalone HAL profile, the signed scale image is three: the
complete conversion helper shifts a positive raw-time delta left by two and
the inverse helper shifts a scheduler delta right by two. This proves the
arithmetic used by DTM, but not a public time unit or the wrap width of the raw
counter. A production implementation must replace the vendor busy wait with a
nonblocking pending-latch state and bounded wake/poll policy.

## What belongs in each open layer

| Layer | DTM responsibility | Current publication gate |
| --- | --- | --- |
| SVD / restricted PAC | Typed controller-window fields, complete publication/ack order, controller-time reads and the SRAM compression domain | Lock/modify request fields, exact images, wait predicate and positional publication-result projection are finite. Live cross-owner MMIO and the remaining scheduler commands are absent. |
| HAL | Powered controller epoch, common RF wake, cache/device fences, timer conversion, same-core IRQ routing and bounded stop/quiesce | Clock/PHY/BTBB components exist separately; no reachable composed epoch or powered rollback exists. |
| Scheduler core | Affine event item, ordered deadline queue, insert/abort/complete states, hardware-head replacement and consistency check | One head lock/modify operation has affine event-driven phases. The finished-item/recycle call chain and DTM callback edge are mapped; the open queue, raw status source, fences and abort path are absent. |
| Packet memory | Static aligned TX/RX/link-state slots with `CPU -> prepared -> hardware -> completed -> CPU` ownership | Compressed address validation, positional list pairs, DTM software object kinds and the narrow RX result-word projection exist. Descriptor layout and RX reclamation are incomplete. |
| LLL DTM | Parameter validation, channel/PHY/pattern image, TX/RX event state machine and receive counter | Exact command roles, allocation rollback, substantial descriptor writes and the positional RX result split are mapped, but field boundaries and result meaning are incomplete. |
| HCI | LE Receiver Test, LE Transmitter Test and LE Test End command/event semantics for only the implemented variants | Bootstrap transport exists; operational DTM opcodes must remain unsupported until the physical owner is live. |

No ULL advertising, scanning, connection, ACL or LLCP implementation is
needed for DTM. Trouble is not part of this slice: Trouble is a Host above HCI
and does not issue DTM as the foundation of its normal GAP/GATT runtime.

## Scheduler interrupt consequence

The previously positional selector-4 callback is now classified by exact
cross-revision role comparison. S31 `r_sym_ble_q4hMJ7XLGGCzxwmAKSge` matches
the named C61 `r_ble_lll_scan_chk_resume`: both are 102 bytes and retry the
scan-start scheduler operation on `-2`, adding 100 to the delay each time.
Therefore selector 4 is a vendor callback for resuming the **scanner ULL
role**, not a silicon interrupt operation and not a general DTM prerequisite.

The first open DTM profile has no scanner owner, so its typed event graph must
make a scan-resume event unrepresentable. It must not retain an unused numeric
selector-4 callback. When scanning is implemented later, its ULL owner needs a
typed collision/resume policy with equivalent observable scheduling behavior.

Selector 6 remains different. Its S31 consumer audits active scheduler
transactions and asserts on an inconsistent item state. The open replacement
is an affine scheduler invariant plus a deterministic fail-stop disposition
on impossible hardware/list state. It is not a public callback and does not
require reproducing the vendor intrusive list layout.

## Next executable increments

The remaining work should proceed in narrow, testable increments:

1. **Compose the scheduler head transaction.** The exact request image, wait
   predicate, diagnostic result nibble and event-driven pure phases now exist.
   Connect task-side publication with fresh ISR-side observations through one
   lost-wake-safe owner. Keep the result positional and do not couple it to
   radio completion. Do not create a task-side alias for `SCHEDULER_STATE`.
2. **Publish the controller timebase component.** The latch address/order and
   standalone scale arithmetic are now known. Add an affine nonblocking latch
   request, prove the raw counter width/wrap behavior and build the same
   conversion as a pure virtual-clock model before live scheduler MMIO.
3. **Freeze the minimum descriptor.** Trace each word read by hardware for one
   TX-role event and prove the complete field masks for access address, CRC,
   channel/frequency, whitening, PHY/rate, power, payload pointer/length and
   next link. For RX, connect the now-exact low-24/high-byte result projection
   to validity, receive-count and reclamation semantics.
4. **Implement affine static slots.** Introduce no-heap aligned storage whose
   typestates prevent CPU mutation after publication and prevent reuse before
   the mapped recycle callback, abort or quiesce. Include device fences and
   negative address tests.
5. **Implement the open scheduler model.** Use a fixed-capacity ordered queue,
   explicit `Prepared/Scheduled/Running/Completed/Recycled/Aborted` states and
   a single hardware-head owner. Feed the bounded finished-list mask into the
   mapped item-selection/recycle transition. Model selector-6 as an internal
   invariant. Keep scan resume outside the DTM feature graph.
6. **Compose the ISR epoch.** The level-3 hard handler captures/acknowledges a
   bounded snapshot, advances only deadline-critical LLL state and publishes a
   lost-wake-safe token. The executor-neutral owner drains completions and HCI
   work. Neither path blocks, allocates or calls an RTOS.
7. **Run DTM without HCI first.** Add a validation-only typed TX request,
   bounded stop and register-trace oracle; then HIL channel/frequency and
   payload-pattern checks. Add RX and received-packet count only after buffer
   reclamation is proven.
8. **Open exactly three HCI operations.** Route only the implemented DTM test
   variants and Test End through the existing async worker. Capability bits
   and supported-command images remain conservative.

Only after this slice passes deterministic virtual-time tests, compiled
production trace comparison, fault/cancellation tests and dated HIL should the
driver report DTM as live. Legacy non-connectable advertising is the next
vertical slice; scanning follows it and introduces the typed successor to the
now-classified scan-resume path.

## Remaining hard unknowns

- scheduler-head diagnostic result-code meanings, if they are operationally
  relevant at all;
- raw interrupt/status origin of the finished hardware-list mask, its
  acknowledge/re-arm order and the fence that makes completed descriptors
  CPU-readable;
- exact controller-time raw width, wrap and physical unit;
- complete S31 descriptor field boundaries and all hardware-read words;
- which positional memory-list pair owns DTM TX, RX and returned buffers;
- exact primary/NRT bits for TX done, RX done, timeout and abort in the DTM
  feature configuration, including their mapping to finished-list indices;
- meaning of the validated RX result high byte, plus length/CRC/RSSI extraction
  and buffer return ordering;
- bounded abort plus powered quiescence when an event is scheduled or running.

These are the next blockers. HCI packet syntax, Trouble integration and an
RTOS abstraction are not.
