# ESP32-S31 Bluetooth interrupt runtime

Verdict: **PARTIAL**. The primary source-124 handler now has a lossless typed
prefix and disposition: its four baseline groups are fatal assertion lanes,
including the two conditional diagnostic captures, while its dynamic suffix
retains two temporally distinct scheduler-state observations, affine MMIO
ownership and a coalesced deferred-work contract. Its later observation also
retains the semantic current hardware-list index from the same sample. The
exact software-list producer/consumer graph and the default BLE consumers of
selectors 4 and 6 are recovered. The initialized scheduler now returns a
powered task endpoint that keeps the sole lock/modify worker beside the exact
task-side HAL owner; one finite event step can therefore reach the restricted
PAC without an MMIO capability escaping into executor code. Primary service is
also serialized with both ordinary durable publications and a capacity-one DTM
post-unlink mailbox. Selector actions,
the hardware-list-to-item relation, NRT feature meanings and the live async ISR
owner remain unresolved, so neither CPU route may yet form a live production
interrupt epoch.

Blobray schema 11 records the two callback mechanisms separately and gives
static event routes an explicit receive/run delivery contract. The
source-124 path proves the exact `R9 -> iEs -> gs5` call chain, static event
initialization, both enqueue sites and conservative CFG ordering from the
generic `eventq_get` call to `event.run`. This ordering is structural and does
not claim path feasibility. The route remains `INCOMPLETE` because the
enqueue-side queue producer is not yet resolved to the exact consumer queue
instance, linked IR does not yet preserve the `eventq_get` result token into
the `event.run` argument, and no replay proves delivery. The finished-list
path proves
source-zero attachment and a conservative CFG witness from the exact
subscriber callback-pointer store to subscription, plus selector
`0x8000_0004` and its merged-mask payload, plus the guarded `uwrf -> rmN`
continuation; it remains `INCOMPLETE` until subscriber lifetime and every
preceding listener's selector-specific continue result are joined into the
same broker epoch. Neither route closes source-124 readiness causality or
post-unlink retry liveness.

This review separates silicon behavior from the internal callback and RTOS
architecture of the reference Controller. The Rust driver does not reproduce
the vendor callback registry or FreeRTOS event queue.

## Pinned inputs

- public Controller archive:
  [`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31),
  `libbtdm_common.a` SHA-256
  `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099`;
- public BLE archive at the same revision, `libble_app.a` SHA-256
  `62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  initial `libble_app.a` SHA-256
  `ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`
  and initial `libbtdm_common.a` SHA-256
  `bd9007072c6ab94df5f29d8b96dc65a69cb4406568c75a64022c8121e242b96c`;
- public OSAL source:
  [`btdm_osal_freertos.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_osal_freertos.c#L139-L211),
  SHA-256
  `436d009f67fbe7c83fa367a6b80dbf87316727605e751febc03c5118a669b5c8`.

Complete function bodies were reviewed from temporary, hash-verified public
artifacts. Neither object files nor disassembly products are repository
inputs. The table below records only the distilled control and MMIO facts.

| Body | Size | Recovered contract |
| --- | ---: | --- |
| `r_sym_bt_R9GZfnUbtn7k6mHtoZbv` | 166 bytes | Primary ISR: masked sample, shared W1C acknowledgement, selector 0, dynamic scheduler suffix, selector 1. |
| `r_sym_bt_hkluARKZBNTcJqMeSNys` | 190 bytes | Primary fault prefix: tests exactly bank-zero source 15 and bank-one sources 9, 8 and 12 in that order; source 9 reads `0x2010_11c8/11cc` and source 12 reads `0x2010_1070` before asserting. |
| `r_sym_bt_3DcjgYC1ikQrP7jNyXGR` | 58 bytes | Walks a linked callback list in registration order and stops when one callback returns nonzero. |
| `r_sym_bt_MDZbajeLxsB0cvuBOREd` / `r_sym_bt_2VbAPsx8lRNZdbVZTEOe` | complete leaves | Read `0x2010_105c/1068`, then copy both images to `0x2010_1058/1064`. |
| `r_sym_bt_6lAYUFKOuBLyOZ6Kvsv5` / `r_sym_bt_Ak3CRkSbyZRhUlneqclG` / `r_sym_bt_DOVkQWJHjeuid8jcS9Bq` | 24 / 24 / 16 bytes | Enable, disable and W1C the exact dynamic images `0x1820_0000` and `0x0000_0008`. |
| `r_sym_bt_37HcX0qW6j1XVtKakUIG` / `r_sym_bt_zczKhmPr5kLPCXpBc7GE` | 80 / 92 bytes | Decode `SCHEDULER_STATE`; the boolean consumed by deferred-work construction is exactly bit 31 AND bit 29. |
| `r_sym_bt_iFvwGI2tL5M1WM3fIkHq` | 12 bytes | Instruction-identical to same-chip named `r_btdm_sched_get_current_link_index`: one `SCHEDULER_STATE` read returns exactly the zero-based hardware-list index in bits 23:20. |
| `r_sym_bt_iEsFo1nbR5S71P2lMKhY` / `r_sym_bt_gs5GeSH15pdzrMDbb7oK` | 90 / 78 bytes | Build one static deferred event, optionally set its one-bit argument, and optionally publish the decoded state through selector 4. |
| `r_sym_bt_uNi9OHmE7XdXfGqTelU5` | 112 bytes | Same-chip named `r_btdm_recycle_in_task`: consume and clear the marker, drain scheduler work, then publish selector `0x8000_0001`; this is a drain event, not one callback per hardware edge. |
| `r_sym_bt_E8c5Eimm0z6kYe9v4wHr` / `r_sym_bt_YRnBzKlWCjsIbotqvNyS` | 360 / 574 bytes | Insert a scheduler item through its `+0x54` intrusive link into the manager-root completion queue; task-side removal/reordering calls the insertion path. This proves a software producer, not a hardware completion FIFO. |
| `r_sym_bt_WHYoiw8ufY0AEM2KSRK1` | 120 bytes | Same-chip named `r_btdm_sched_pop_executed_sch`: on every worker pop attempt, copy the low halfword from `0x2010_125c` to a zero-high image at `0x2010_1260`, merge the pending finished-list mask, synchronously publish broker event `0x8000_0004` when nonzero and then attempt a software completed-list pop. |
| `r_sym_ble_rmNuzAO8kQQQXQIpTzGZ` / `r_sym_bt_M9nG353V0svWrv1l1zGw` | 462 / 322 bytes | Same-chip named `r_sched_txn_onSchedHwListDone` and `r_btdm_sched_pick_finished_items`: walk set finished-list bits, unlink matching scheduler items from the hardware-linked list and append them to the software completed queue through `item+0x54`. |
| `r_sym_bt_QsLKLOCC2pct4rL8uFBN` | 130 bytes | Same-chip named `r_btdm_recycle_process_dequeued_sch`: clear the dequeued link image, recover its link-state pointer and invoke the item-specific recycle callback stored at `item+0x58`. |
| `r_sym_ble_uwrf0kLZsRbzFJ7u8SEr` / `r_sym_ble_T40PqM3CeultOGiVkAp0` | 136 / 186 bytes | Default manager-0 selector-6 consumer and its scheduler action. Selector 6 walks active BLE scheduler transactions/list entries and asserts on an inconsistent `item+0x38` state. |
| `r_sym_ble_zrorswmoCrQoX5oTeECu` / `r_sym_ble_3wftOXafF5ZkxLriL8L3` / `r_sym_ble_q4hMJ7XLGGCzxwmAKSge` | 62 / 188 / 102 bytes | Default manager-0 selector-4 consumer. It checks BLE scheduler/current-item state and, for a false publication when the predicate holds, retries a scheduler operation while it returns `-2`, increasing the delay in steps of 100. |
| `r_sym_ble_ywjh0f9yjTBeI7XgS5da` | 74 bytes | NRT ISR: raw sample at `0x2010_1340/1348`, shared W1C acknowledgement and selector `0x8000_0000`. |

## Exact primary suffix

The complete primary handler first preserves the implemented
sample/sample/ack/ack order. Its next complete function treats precisely the
baseline enable groups as fault/assert paths:

| Pending source | Diagnostic capture before assertion |
| --- | --- |
| bank 0 source 15 | none |
| bank 1 source 9 | complete words `IRQ_DIAGNOSTIC_DETAIL_0/1` at `0x2010_11c8/11cc` |
| bank 1 source 8 | none |
| bank 1 source 12 | complete `IRQ_DIAGNOSTIC_STATE` at `0x2010_1070` |

This is not a Link-Layer completion group. The restricted PAC therefore
projects the acknowledged status into one semantic
`BluetoothPrimaryInterruptEpoch` and never exports the raw bank or diagnostic
images. `BluetoothPrimaryFaultSources` also marks any pending status outside
the reviewed dynamic and fault source groups as unclassified. The Bluetooth
classifier gives known fault lanes and unclassified status precedence over
simultaneous dynamic bits and returns `BluetoothPrimaryControllerFault`; a
future live owner must retain it, skip ordinary LL work and enter
fail-stop/quiesce. Rust does not reproduce the vendor assert routine in
hard-interrupt context.

Only after that prefix does the reference handler dispatch selector 0 with the
two-word snapshot, execute the following dynamic branch, then dispatch selector
1 with the same snapshot.

Dynamic classification is positional because Link-Layer names are not yet
proven:

| Pending image | First work input | State publication requested | Extra reference gate |
| --- | ---: | ---: | --- |
| bank 1 source 3 | bank 0 source 27 OR 28 | yes | yes |
| no bank 1 source 3; bank 0 source 27 OR 28 | yes | bank 0 source 21 | no |
| only bank 0 source 21 | no | yes | no |
| none of these sources | no work | no | no |

Bank 1 source 3 has precedence over the bank-zero branch. Its reference gate
reads `SCHEDULER_STATE` at `0x2010_107c`; when bit 31 is clear it writes zero
to `SCHEDULER_REFERENCE` at `0x2010_1078` and dispatches selector 6. The
default BLE selector-6 consumer immediately performs a scheduler
transaction/list consistency action. The typed classifier therefore returns
`ClearReferenceAndRunPostClearSchedulerAction`: executing only the register
write is deliberately not representable as a complete disposition. Any
dynamic branch then makes a later, independent `SCHEDULER_STATE` read. The
derived reference state is `bit31 && bit29`. Deferred work is marked only when
the first work input and that derived state are both true. Selector 4 receives
the same derived state when the table requests state publication; the default
BLE consumer uses both its presence and boolean payload to decide whether to
retry scheduler work.

The reviewed current-link leaf proves that bits 23:20 of this same register
are the zero-based current hardware-list index. The restricted PAC projects
that field from the later sample and the durable primary scheduler event
retains it beside the busy/reference observation. This identifies the active
list at that temporal point; it does not identify an affine scheduler item,
imply that the list is finished or authorize descriptor reclamation.

The two reads cannot be folded into one observation: the reference clear and
selector-6 software path occur between their positions. The Rust classifier
therefore uses distinct reference-gate and work-observation types.

The post-unlink DTM removal gate does not authorize a second primary
capture/acknowledgement. The Controller atomically consumes the empty-head
proof into `SoftwareListUnlinked` and arms a capacity-one mailbox under the same
critical section. On its first arm, that mailbox obtains a checked globally
unique opaque identity; every arm also carries a checked generation. Identity
or generation exhaustion rejects before unlink. Every public primary service then
serializes capture/acknowledgement, both ordinary durable cell publications and
the mailbox transition under that same boundary. An idle mailbox returns the
event to the general route; an armed mailbox stores exactly its first later
`BluetoothPrimaryPublishedInterruptStep`; and a full mailbox returns, but does
not overwrite with, the newer event. Ordinary scheduler and lock/modify wake
dispositions are returned immediately even when the affine event payload is
stored for DTM. Those ISR notification dispositions are not repeated by the
later DTM consumer; it retains only the event observation needed by the return
gate. There is no public standalone unlink, primary-service bypass or
constructor for the internal graph/event pair, so a pre-arm event cannot enter
the removal consumer through safe public code.

The affine awaiting owner consumes only its matching mailbox identity and
generation, so equal generation numbers from two Controller instances cannot
cross-wire an event, cancellation or re-arm. Its later consumer may project
BUSY from the stored scheduler event and, only when idle,
perform the separate task-owned command-zero then conditional command-one
reads. `NoSchedulerWork` and command-pending outcomes re-arm the same owner
before leaving the serialization boundary. This closes temporal pairing and
lost-owner holes, but not progress: if the retained first event is not ready, a
later command-ready edge can arrive while the slot is full and is not retained
as the next DTM event. Complete vendor removal bodies directly re-read BUSY and
the command fields, so they prove neither command-ready-to-source-124 causality
nor a guaranteed retry wake. Source-124 causality, registered consumer wake and
bounded retry liveness therefore remain session-runtime work rather than an
interrupt-classifier claim.

## Event multiplicity and RTOS-free replacement

The exact public OSAL refuses a second insertion while the same static event
is queued. The marker assignment occurs before insertion, so a later marked
edge upgrades an already queued ordinary event, and a later ordinary edge
cannot clear a marker. Dequeue clears the queued flag before the event handler
runs. Consequently the required deferred contract is:

1. the first publication opens a pending epoch and wakes the sole worker;
2. repeated publications coalesce into that epoch;
3. `marked` is sticky across every coalesced publication;
4. dequeue atomically consumes pending plus marker;
5. a racing publication after dequeue opens a new epoch and emits a new wake.

`BluetoothSchedulerWakeCell` implements this contract with one `AtomicU8`.
It is not an async primitive by itself: the platform must still install a
lost-wake-safe waker registration and the Controller worker must drain the
real scheduler list before considering the epoch complete.

The drained list is not a hidden hardware FIFO. Complete producer review shows
that task-side scheduler item removal/reordering inserts items into a
manager-root intrusive completion list through the link at `item+0x54`. On
every worker pop attempt, `r_sym_bt_WHY...` performs the low-halfword transfer
from `0x2010_125c` to `0x2010_1260`, merges the pending finished-list mask and
synchronously publishes broker event `0x8000_0004`. Reviewed registration,
source-domain, selector and continuation facts anchor the intended path to
`r_sym_ble_rmN...`, which walks set list bits and calls
`r_sym_bt_M9n...` to move finished hardware-linked items onto that same
software queue. The worker then pops an item, and `r_sym_bt_QsLK...` invokes
its role-specific callback at `item+0x58`. For DTM this is the mapped
`r_ble_lll_dtm_recycle_sch_item` body. Thus the lock/modify request result is
not a completion signal; finished-mask selection plus item recycle is the
reference software ownership-return path. Blobray does not label the broker
delivery complete until subscriber lifetime is joined to the publisher epoch
and every preceding listener is proven to continue for this selector, or the
exact broker epoch is replayed.

An open Controller must therefore define its own typed scheduler item
lifecycle and bounded completion queue, with an explicit hardware-finished to
CPU-readable fence. Reproducing the vendor list nodes or OSAL event object is
neither required nor desirable. The restricted PAC now names and transfers
the exact 16-bit `SCHEDULER_FINISHED_LIST_STATUS` mask while keeping the
destination word positional as `SCHEDULER_FINISHED_LIST_REPORT`. The Bluetooth
core can drain one list bit per bounded event step; it does not yet map a bit
to an affine item or callback. The separately retained current hardware-list
index and every finished-list bit use the same `0..16` hardware-list domain.
A finished bit identifies a list, while the current index identifies the
active list at a distinct temporal point; neither selects an affine item.

Complete default BLE registration review found exactly three manager-0
consumers. `r_sym_ble_uwrf0kLZsRbzFJ7u8SEr` owns selector 6,
`r_sym_ble_zrorswmoCrQoX5oTeECu` owns selector 4, and
`r_sym_ble_xwc69LJVHnjhZA8uSJnQ` handles selectors 0 and `0x8000_0000` while
ignoring 4 and 6. Thus selectors 4 and 6 are replaceable software ABI, but
their applicable effects are not optional. Exact role comparison with the
named C61 body `r_ble_lll_scan_chk_resume` classifies the 102-byte S31
`r_sym_ble_q4hMJ7XLGGCzxwmAKSge` retry body as scanner-role resume. An open
DTM-only scheduler therefore has no selector-4 successor at all; a later scan
ULL owner must provide the typed collision/resume policy. Selector 6 remains
the active-transaction consistency action and becomes an internal affine
scheduler invariant plus fail-stop disposition. The current wake cell retains
only the proven pending/marked coalescing contract; it intentionally does not
pretend to implement either higher-level action. The DTM-specific consequence
and evidence chain are recorded in
[`bluetooth-direct-test-mode.md`](bluetooth-direct-test-mode.md).

The linked callback selectors are boundaries between closed vendor software
components, not ESP32-S31 hardware ABI. An open Link Layer may use its own
typed events. It must nevertheless preserve every proven MMIO ordering edge,
the sticky marked-work distinction, and any behavior later shown to be
required by the scheduler-list consumer.

## NRT default-lifecycle result

The NRT ISR belongs to callback-manager ID `0x4003` and synchronously dispatches
selector `0x8000_0000` after raw sample/sample/ack/ack. Complete relocation and
caller review across the pinned BLE, common and classic Controller archives
found no registration into manager `0x4003`; the manager is initialized with
an empty list in the default lifecycle. Other managers do have concrete
registrations, so absence here is not inferred merely from stripped names.

This establishes an acknowledge-only NRT path for the pinned default software
lifecycle, not a silicon guarantee that source 133 can be removed or that its
snapshot fields have no meaning under future features. The open ISR must retain
the affine acknowledgement token and shared-clear ordering. It must not invent
LL wakes until a feature-specific producer/consumer path or HIL establishes
them.

## Remaining publication blockers

- feature-specific meanings and mask/re-arm policy for NRT snapshot status beyond
  the pinned default lifecycle's acknowledge-only callback graph;
- typed open equivalents for the selector-6 post-clear consistency action and
  selector-4 reference-state-driven scheduler retry;
- an affine scheduler item lifecycle and bounded completion queue that replace
  the recovered software intrusive list, including the semantic
  item ordering/selection within each hardware list, plus the status-to-
  finished-list edge, memory fence, abort and shutdown drain;
- executor-neutral waker registration with a register-then-recheck poll
  protocol and a quiescent teardown barrier;
- a proven source-124 or other wake for command readiness, plus a bounded
  policy that cannot lose a later ready edge while the capacity-one DTM slot
  retains an earlier no-work or command-pending event;
- live same-core composition of the staged interrupt-bank, scheduler runtime
  and powered task endpoint, including stable waker registration, without
  returning either MMIO capability while the routes are live;
- compiled-production trace and HIL for simultaneous, repeated and teardown
  interrupt epochs.

Until these are closed, the dynamic masks remain disabled in production and
the typed ESP-HAL routes remain private.
