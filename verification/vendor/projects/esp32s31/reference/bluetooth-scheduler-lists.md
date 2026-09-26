# ESP32-S31 Bluetooth scheduler list contracts

This reference describes how the vendor Controller keeps several scheduler
items and several hardware lists live at the same time: list ownership,
time-ordered insertion, conflict resolution, insertion into a running list and
completion. It is the input for a multi-item open scheduler. Single-item DTM
publication and completion are described in
[Direct Test Mode](bluetooth-direct-test-mode.md); the interrupt and deferred
work path is described in [Bluetooth interrupts](bluetooth-interrupt-runtime.md).
No open implementation of the contracts below exists yet.

## Pinned public inputs

- ESP32-S31 Controller archive
  [`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31):
  `libbtdm_common.a` SHA-256
  `fa22a8a2aca48b807addda2bbad78868d6774c82bcdeb8090f9140f6cbccd099`
  (scheduler member `19.o`) and `libble_app.a` SHA-256
  `62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`;
- same-chip role-name reference only:
  [`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
  `libbtdm_common.a` SHA-256
  `bd9007072c6ab94df5f29d8b96dc65a69cb4406568c75a64022c8121e242b96c`
  (member `btdm_sched.c.o`) and `libble_app.a` SHA-256
  `ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`
  (members `sched_txn.c.o`, `sched_stack.c.o`).

Every behavioral statement below comes from the current-revision instruction
body. Names come from the initial archive and are carried through every
intermediate archive revision by
[`oer-symbol-lineage`](../../../../../tools/symbol-lineage/README.md): a name
crosses revision `5e37d4d`, which introduced generated names, on its unchanged
source name or on an identical relocation-normalized body, and later revisions
keep generated names stable. The bodies that differ from the initial revision
were read in their current form. The current `r_sched_txn_onSchedHwListDone`
body differs only in its assertion helper.

`r_sym_bt_DPWY0umixzmXEaFuUyCI` is `r_btdm_sched_run`: its revision-`5e37d4d`
body is identical to the initial one. Revision `7729629` moved that body into
the new function `r_sym_bt_PVKilXLQPu1BjRkm4C6O`, which has no recovered name,
and made `r_btdm_sched_run` its caller.

| Initial name | Current symbol | Current body |
| --- | --- | --- |
| `r_btdm_sched_insert_with_lock_modify` | `r_sym_bt_VrTmsQfPlkmys4UL0NZp` | one added instruction |
| `r_btdm_sched_insertion_begin` | `r_sym_bt_EabYtUaAIR05LXw3qZSA` | changed |
| `r_btdm_sched_insertion_end` | `r_sym_bt_4KfpZh0Hu5NprlqcNu0D` | changed |
| `r_btdm_sched_execution_lock` | `r_sym_bt_9H3AnHbaHJ3auzSvPDme` | changed |
| `r_btdm_sched_execution_modify` | `r_sym_bt_rPoPGH6BBYjZaunDU5FV` | changed |
| `r_btdm_sched_wait_lock_modify_idle` | `r_sym_bt_B8fDByebTHuwRe8FZuRt` | identical |
| `r_btdm_sched_merge_list_remove_overlap` | `r_sym_bt_YRnBzKlWCjsIbotqvNyS` | changed |
| `r_btdm_sched_check_overlap_in_list` | `r_sym_bt_FovcDCPDYkKMaCv7y4Wb` | identical |
| `r_btdm_sched_remove_unshareable_entries` | `r_sym_bt_E8c5Eimm0z6kYe9v4wHr` | identical |
| `r_btdm_sched_rm_item_directly` | `r_sym_bt_hddDtuOCoB0U4KYRErRq` | identical |
| `r_btdm_sched_delete_from_list` | `r_sym_bt_KmzLfKJ5UUi7zkqz1Nwn` | changed |
| `r_btdm_sched_delete_specified_items` | `r_sym_bt_qkNMymdnaJnYUfzpKgEp` | identical |
| `r_btdm_sched_search_deleted_items` | `r_sym_bt_1Q7VVJH4siSgF2fbAf3f` | changed |
| `r_btdm_sched_skip_specified_sch` | `r_sym_bt_DkrQYQcoyzIHdYi0CzWE` | identical |
| `r_btdm_sched_stop` | `r_sym_bt_74l62ZLsZuXg67pPHSd7` | changed |
| `r_btdm_hal_link_skip_specified_tl` | `r_sym_bt_t4aeyhcVrKTNMSlq45XR` | identical |
| `r_btdm_sched_pick_finished_items` | `r_sym_bt_M9nG353V0svWrv1l1zGw` | identical |
| `r_btdm_sched_run` | `r_sym_bt_DPWY0umixzmXEaFuUyCI` | changed |
| `r_btdm_sched_get_hw_list_header` / `set_hw_list_header` | `r_sym_bt_6wSHUtNRioHeB7CKjVJA` / `r_sym_bt_8m3cRMNRZNfaJ7qVvayk` | identical |
| `r_btdm_sched_mem_get_hw_start_time` | `r_sym_bt_sdf6bUMpe1CnARWl962a` | identical |
| `r_btdm_sched_reset_new_item` | `r_sym_bt_RnJIqDW4oA0usCDLVZGy` | identical |
| `r_sched_txn_insertIntoList` | `r_sym_ble_2NzCGxXVVAgscKC4JuXu` | identical |
| `r_sched_txn_insertOne` | `r_sym_ble_jVs2DPPaJL7CDx8SeFuo` | identical |
| `r_sched_txn_getListFromSch` / `getListFromDevice` | `r_sym_ble_oDSWrKSM8tZw8SolRxrc` / `r_sym_ble_9CBCO0GZ7pUHoTfKVbia` | identical |
| `r_sched_txn_resortSwList` | `r_sym_ble_mrucwvpRHVsLLccQveKl` | identical |
| `r_sched_txn_onSchedHwListDone` | `r_sym_ble_rmNuzAO8kQQQXQIpTzGZ` | assertion helper only |
| `r_sched_txn_init` | `r_sym_ble_9VDW4bivmhCHFVAkh57O` | identical |

## Hardware list model

The scheduler exposes sixteen hardware lists. List `i` occupies the two words
at `0x2010_b000 + 0x10 * i`:

- word 0 bits 19:0 hold the compressed head item pointer. Its address is
  `0x2f00_0000 | (field << 2)`; a zero field is an empty list. Writers preserve
  bits 31:20;
- word 1 is the hardware start time of that list.

A list is a singly linked chain through item word `+0x00` bits 19:0, using the
same compression. Walkers OR the shifted field with the value read from
`0x2010_1074` to recover a full address. `SCHEDULER_STATE` at `0x2010_107c`
reports BUSY in bit 31 and the list being executed in bits 23:20.

The BLE stack reserves thirteen lists: `r_sched_txn_init` stores 13 as the
list count and `0x1fff` as the free-index bitmap. Hardware indexes are a
limited resource that the software lends to lists, not a fixed per-role
assignment.

## Scheduler item fields

These offsets are used by the list functions. They extend, and do not
replace, the role-specific item layouts in the role references.

| Offset | Meaning in list code |
| --- | --- |
| `+0x00` bits 19:0 | Hardware next link. `reset_new_item` clears it together with bit 25 |
| `+0x00` bit 25 | Set on every deleted, skipped item, and on a preempted item when lock-modify is enabled |
| `+0x00` bit 24 | Marks the item that the merge reports to its optional first-marked output |
| `+0x00` bit 22 | When clear, a completed item must not start after the current list head |
| `+0x18` bits 3:0 | Priority nibble (not consulted on any reachable conflict path) |
| `+0x30` bit 31 | Selects the device list and enables the late-start check |
| `+0x38` | Execution status. `0xffff_ffff` means not executed; hardware replaces it; preemption writes zero |
| `+0x44` / `+0x48` | Start and end time; lists are ordered by start |
| `+0x4d` | Item kind; kind 2 is a background item handled by a separate path |
| `+0x4e` | Halfword flags. Merge clears the low byte; reset sets bit 0 and clears bits 10:8 |
| `+0x4f` bit 3 | Overlap already resolved; merge inserts by time without removing entries |
| `+0x4f` bits 2:1 | Set to `0b11` when the item is preempted; bit 1 alone marks a deleted item |
| `+0x50` | Previous item in its list, and the caller's insertion hint |
| `+0x54` | Software link for a chain of new items and for the completion queue |

## Software lists

A software list descriptor is `0x1c` bytes:

| Offset | Meaning |
| --- | --- |
| `+0x00` | Earliest item that must be considered current |
| `+0x04` / `+0x08` | Head and tail of the item chain |
| `+0x0c` / `+0x10` | Links in the start-ordered list of software lists |
| `+0x14` | Start time used to order software lists |
| `+0x18` | Signed hardware list index; negative means software-only |
| `+0x19` | List enabled |
| `+0x1a` | Start time comes from the sleep timer instead of the list word |

The stack owns a default list shared by all ordinary roles and a background
list. An item whose `+0x30` bit 31 is set belongs to its device's embedded
list instead (device `+0x2c`, selected by device `+0x04` bit 27). The Direct
Test Mode context embeds such a list with index 0.

`r_sched_txn_resortSwList` keeps software lists ordered by their start time and
lends the free hardware indexes to the earliest lists. When all indexes are
lent, the index of the latest list is moved to the earlier one through
`r_btdm_sched_replace_hw_entry_with_index`. Items of different roles therefore
share one hardware list, in start order, while their software list holds an
index. Arbitration between hardware lists that are live together is performed
by hardware; the criterion it applies is not established by these bodies.

## Insertion

`r_sched_txn_insertOne` first runs `r_btdm_sched_check_overlap_in_list`, then
`r_sched_txn_insertIntoList`. That calls `r_btdm_sched_calc_seq_time` and, for an
enabled list with a hardware index, `r_btdm_sched_insert_with_lock_modify`. A
software-only list uses the merge alone. When the merge makes the item the new
head, the list start time is refreshed and the software lists are resorted.

### Overlap check

`r_btdm_sched_check_overlap_in_list(list, item, hint, priority, argument)`:

1. rejects an item with `+0x30` bit 31 whose start is not later than the
   current time plus the stack margin, returning -2;
2. sets the item's `+0x4f` bit 3;
3. starts after the latest predecessor of the hint whose `+0x4e` byte is zero,
   or at the list's current item;
4. walks the hardware links comparing `[start, end)` intervals. An item that
   ends before an entry starts stops the walk. An entry that ends before the
   item starts becomes the predecessor;
5. for an overlap, calls `priority(item, entry, argument)` when present. A
   negative result is returned as rejection. Result 2 tolerates the overlap.
   Result 1 keeps walking without making the entry the predecessor. Result 0,
   and every overlap without a callback, is tolerated because step 2 already
   set bit 3 on the new item;
6. stores the predecessor in `+0x50`, resets the item's hardware link and
   flags and clears `+0x54`.

It returns 0, 1 or 2 for an admitted item. The priority nibble comparison
after a missing callback is unreachable in this body because of step 2.

### Merge

`r_btdm_sched_merge_list_remove_overlap(chain, list, last, marked)` inserts a
chain of new items linked through `+0x54`. For each item it clears the `+0x4e`
byte, writes `0xffff_ffff` status and then:

- with `+0x4f` bit 3 set, inserts after the latest item whose start is not
  later than its own, beginning at the `+0x50` hint;
- with bit 3 clear, calls `r_btdm_sched_remove_unshareable_entries` from the
  hint and inserts after the predecessor it returns.

Insertion writes the item's and the predecessor's hardware links, sets the
successor's `+0x50`, and updates head, tail and the list's current item.
Before linking a successor it asserts that the successor has not executed. The
function returns 7 when an item became the list head and 0 otherwise. `last`
receives the final item of the chain.

`r_btdm_sched_remove_unshareable_entries` walks from the given entry. Every
overlapping entry without `+0x4f` bit 3 is published to the stack broker,
unlinked, appended to the completion queue through `+0x54` and marked
preempted: `+0x4f` bits 2:1 become `0b11` and an unexecuted status becomes 0.
Preempted items therefore reach their recycle callback through the ordinary
completion path.

### Insertion into a live list

`r_btdm_sched_insert_with_lock_modify(list, item, index)` brackets the merge
with a hardware transaction:

| Begin outcome | Condition | Action |
| --- | --- | --- |
| 3 | sleep policy disabled | No list change |
| 3 | BUSY clear | Hint becomes the list tail; list current item is cleared |
| 4 | BUSY and the execution lock at the hint succeeds | Hardware is locked at the predecessor |
| 5 | BUSY otherwise | Execution modify on the list mask; hint becomes the predecessor of the hardware head; list current item becomes that head |

The sleep-policy predicate is the external function that the
[Direct Test Mode](bluetooth-direct-test-mode.md) reference identifies for
insertion end. The registers are:

- `0x2010_1254` execution lock: bit 31 START, bits 23:20 list index, bits
  19:0 compressed item. Bit 24 reports completion. Bits 29:27 are the result:
  0 locks, 2, 5, 6 and 7 fail, and 1, 3 and 4 are treated as impossible.
  Bit 26 reports the lock engine idle;
- `0x2010_1258` execution modify: bit 31 START, bit 16 a mode flag, bits 15:0
  the list mask. Bit 17 reports completion and bit 19 is treated as impossible.
  Bit 18 reports the modify engine idle;
- `0x2010_1218` lock-modify request: bit 31 START and bits 19:0 compressed
  item, after the list index is written to the low nibble of `0x2010_136c`.
  Bits 30:27 return a result.

Every wait polls only while BUSY is set and asserts after 9999 iterations.
Before a lock or modify, the stack waits until BUSY clears or both engines
report idle.

After the merge, outcome 4 issues a lock-modify request for the last new item
when the environment enables it, and publishes the result to the broker.
Insertion end then:

- for outcome 4 clears lock START;
- for outcome 5 publishes the submitted item as the list head and clears
  modify START;
- when BUSY is clear, publishes the software list head with sleep policy
  disabled or, for an unexecuted submitted item, publishes that item and calls
  `r_btdm_sched_run`. That calls `r_sym_bt_PVKilXLQPu1BjRkm4C6O` (acknowledge
  and enable the dynamic interrupts, broker event 2) and, when it returns
  zero, writes 1 to `0x2010_1000` to start the scheduler.

## Completion

Hardware replaces an item's `0xffff_ffff` status. On a finished-list
interrupt, `r_sched_txn_onSchedHwListDone(mask)` visits every software list
whose hardware index is set in the mask:

1. it reads the list start time;
2. it calls `r_btdm_sched_pick_finished_items(list, index)`, which walks from
   the head, unlinks executed items and appends them to the completion queue.
   The walk skips at most one unexecuted item. A completed item without
   `+0x00` bit 22 must not start later than the current hardware head;
3. an emptied list must have an empty hardware head and is removed;
4. otherwise the list start time is refreshed and the software lists are
   resorted, provided that the hardware head is not empty, no out-of-order
   completion was found and `SCHEDULER_STATE` does not have bit 29 set while
   bits 23:20 name this list.

Queued items are then recycled through their `+0x58` callback, as described in
[Bluetooth interrupts](bluetooth-interrupt-runtime.md).

## Cancellation

`r_btdm_sched_search_deleted_items(start, predicate, argument)` walks the
hardware links from `start`. A positive predicate result adds the item to a
chain linked through `+0x54`, zero skips it and a negative result stops the
walk. The `sched_txn` delete-by-type, by-state-machine and caller-selection
functions build such a chain and pass it to `r_btdm_sched_delete_from_list`.

`r_btdm_sched_delete_from_list(list, index, chain)` first calls an external
sleep-path function. Without a chain it empties the whole list: head, tail and
current item are cleared. With BUSY clear the hardware head is cleared
directly. With BUSY set it issues execution modify on the list mask with the
mode flag set, clears the hardware head and then clears modify START. With a
chain it calls `r_btdm_sched_delete_specified_items`.

`r_btdm_sched_delete_specified_items(list, index, chain)` has two phases:

1. software unlink: every chained item gets `+0x00` bit 25 and `+0x4f` bit 1
   and is removed from the list's links, head, tail and current item. Its own
   hardware next link is kept, so hardware that already holds it can still
   follow the chain;
2. for a list with a hardware index:
   - with BUSY clear, the hardware head is republished from the software head
     unless sleep policy is enabled;
   - with BUSY set, it waits for idle lock and modify engines. When the
     environment enables lock-modify it opens a hold: it waits for
     lock-modify START to clear, writes the list index to `0x2010_136c` bits
     7:4, writes `0x80` to `0x2010_1058`, sets `0x2010_1204` bit 0 and waits
     while BUSY for `0x2010_1324` bit 0, then while BUSY for `0x2010_1208`
     bit 0 to clear;
   - each chained item that has not executed and does not start after the
     current hardware head is passed to `r_btdm_hal_link_skip_specified_tl`.
     The head is read once, before the hold; when it is empty, every
     unexecuted item is skipped. Execution is checked as the loop reaches
     each item. Result 0 is treated as impossible, and results 2 and 4 end
     the loop;
   - the hold is released by clearing `0x2010_1204` bit 0.

   It then notifies the recycle path.

`r_btdm_hal_link_skip_specified_tl(index, item)` writes `0x2010_10ec` with
bit 31 START, the list index in bits 23:20 and the compressed item in bits
19:0. It waits while BUSY and START are both set. It returns bits 29:28 of the
register, or 4 when BUSY cleared first, and then clears the register.

`r_btdm_sched_skip_specified_sch(items, index, count)` sets bit 25 on an array
of items, skips them one by one with the same loop rule and, when
lock-modify is enabled, performs the same hold as a closing pulse.

`r_btdm_sched_stop` returns when BUSY is clear. Otherwise it disables the
dynamic interrupts, publishes broker event 3, waits for idle lock and modify
engines, writes 1 to `0x2010_1004` and asserts if BUSY is still set after
65536 polls.

## Receive routing

The named `ble_lll_mmgmt.c.o` and `ble_lll.c.o` of the role-name archive
route receptions per role:

- `r_ble_lll_mmgmt_update_global_rxlink(item)` enables the link state's
  receive object (link-state `+0x7c`), selects class one for scanner kind
  two and class two otherwise, writes the class to link-state `+0x20` bits
  30:28 and records item `+0x20` bits 11:0 in the object's `+0x18` halfword;
- the item numbers come from the role allocators: advertising instance `k`
  uses `k`, connection `c` uses the extended-advertising limit plus one plus
  `c`, the scanner's items follow the connections, and DTM follows five
  private and four further numbers plus the periodic-synchronization limit;
- `r_ble_lll_get_rxed_buffer(link_state)` walks the chain from the software
  head (link-state `+0x68`), stops at the first incomplete header, skips a
  completed packet whose `+0x18` halfword differs from the recorded number,
  moves a left-behind packetless header into the spare slot (`+0x78`) and
  marks the last completed header as the one hardware may still hold;
- `r_ble_lll_append_rx_buffer` returns a node after the tail (`+0x70`),
  copying a held header's packet into the spare header so that the held
  header stays in the chain without a packet;
- when the object is enabled for a global class, both functions first load
  and afterwards store the class's shared head, tail and spare, so every role
  of a class walks one chain;
- `r_ble_lll_conn_use_rxbuf_from_link_state` clears the class bits, keeps the
  object enabled with class zero and points the private consumer in
  link-state `+0x08` at the software head: a connection receives through its
  own chain.

## Open contracts

The bodies above do not establish:

- the hardware criterion between live hardware lists and whether hardware
  evaluates overlaps across lists;
- the effect of the execution lock, execution modify and lock-modify request
  on item execution, and the meaning of each result code;
- the modify mode flag, the lock-modify result and when the environment
  enables the lock-modify request;
- the meaning of item `+0x00` bit 22, of `SCHEDULER_STATE` bit 29 and of the
  sleep-timer start source;
- why insertion begin skips locking when sleep policy is disabled;
- the effect of the skip request, the hold at `0x2010_1204` and the meaning
  of the skip results 1 to 3;
- whether hardware reads item `+0x00` bit 25 as a skip marker;
- whether hardware records item `+0x20` bits 11:0 in received packet
  `+0x18`, and whether a class-zero link state receives only through its
  `+0x08` consumer;
- the sleep-path functions called by insertion and deletion, and the
  background list.
