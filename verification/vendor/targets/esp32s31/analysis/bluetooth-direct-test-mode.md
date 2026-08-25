# ESP32-S31 Bluetooth LE Direct Test Mode boundary

Verdict: **INCOMPLETE**. The complete ESP32-S31 DTM command bodies, their path
into the common scheduler, the finite lock/modify head-publication transaction
and the later scheduler-item recycle callback are now identified. The exact
eight-word reset region of its link-state image and the nine-word event update
of its scheduler item are also modeled without the vendor allocator. The exact
forty-channel permutation/frequency composition and role-dependent PHY-rate
images are now typed inputs to that event. Together these prove that DTM is a
small Lower Link Layer role built from
controller-SRAM descriptors, a common radio wake operation and an event-driven
scheduler lifecycle. They do not yet prove the complete descriptor layout, the
hardware status-to-finished-mask boundary, memory fences or RX buffer
reclamation needed for an on-air open implementation.

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
| `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` | 618 | Construct and schedule one DTM event. It accepts only role values one and two, wakes the common RF owner, writes the reviewed frequency/rate/role/timing scheduler-item images, begins a scheduler transaction and completes that transaction. |
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

The reset transaction itself is now reproduced as a non-publishable reviewed
region in the Bluetooth crate. It retains the exact low-twenty-bit compressed
links at `+0x00` and `+0x08`, including the overlapping `+0x02` halfword
transform; five-bit rounded-power image at `+0x04`; high control bits at
`+0x14`; high-byte-preserving `0x555555` image at `+0x2c`; RX-only zero at
`+0x34`; complete `0x71764129` word at `+0x38`; and six-bit configuration image
at `+0x50`. Bounded inputs are rejected before any transform. The type exposes
no controller address, storage publication or hardware-ownership transition,
because the omitted descriptor words and their hardware consumer remain open.

The scheduler-item update is now a second non-publishable reviewed region.
It reproduces the role byte at `+0x02`, bit-31 publication prerequisite at
`+0x04`, four-bit clear at `+0x08`, repeated two-bit rate image at `+0x14`,
seven-bit frequency plus low-nibble image at `+0x18`, RX-only complete
`0x000f0001` at `+0x2c`, two epoch-projected raw-time words at `+0x44/+0x48`
and the low-byte clear at `+0x4c`. The timebase model now retains both exact
wrapping conversion directions; the inverse truncates discarded scheduler
bits toward its anchor, as the complete helper does.

The event's channel and PHY inputs are no longer arbitrary field images. The
current `r_sym_ble_fQsMV3sWyYa3SB0n61bb` and initial
`g_channel_rf_to_index_ro` forty-byte DTM permutations are byte-identical:
channels 0, 12 and 39 select RF indices 37, 38 and 39 while the remaining DTM
channels select RF indices 0 through 36 in order. Current
`r_sym_ble_KlCLZ2Zedz0kVQxLavnT` and initial `g_ble_phy_chan_freq_ro` are also
byte-identical. Their complete composition maps DTM channel `n` to positional
frequency image `2*n` for all `0..=39`. Complete
`r_sym_ble_c4Wk4lIgPXJQLMwbA2Zp` and named `r_ble_phy_chan_to_freq` bodies
confirm the table consumer.

The current TX/RX validators additionally agree instruction-for-instruction
with their named initial S31 roles over the accepted PHY domains. TX accepts
HCI selectors one through four, maps selector four to internal mode zero and
passes the other selectors through. RX accepts only selectors one through
three and maps selector three to mode zero. Complete
`r_sym_ble_7iafDUuOcihxmYHJfSBd` and named `r_ble_phy_mode_to_rate` map modes
one, two, three and zero to positional rate images zero, one, two and three.
Consequently 1M/2M map to zero/one for both roles, selector-three Coded maps to
two for TX and three for RX, and Coded S=2 selector four maps to three for TX
but is unrepresentable for RX. The Rust event now enforces this asymmetry and
cannot accept an odd frequency image or an arbitrary two-bit rate. These
remain descriptor images rather than a packet-engine readiness claim; the
scheduler-item type still cannot insert itself or produce a hardware-owned
token.

The TX payload construction has no allocator requirement. Complete current
`r_sym_ble_XaQTq1AjtothrIFLgEeJ` and initial
`r_ble_lll_dtm_tx_create_ctx` bodies agree on all eight selector branches:
selectors 1, 2, 4, 5, 6 and 7 fill `0x0f`, `0x55`, `0xff`, `0x00`, `0xf0`
and `0xaa`, while selectors 0 and 3 copy PRBS9 and PRBS15 respectively. The
body stores the selector and eight-bit length directly before the payload and
copies exactly that many bytes.

The current and initial 255-byte PRBS tables are byte-identical. PRBS9 has
SHA-256 `e2a8f5102484eb3bda6e3b5ebb6917bdf31920d3351d68d8b46a645e57356678`;
PRBS15 has SHA-256
`7ba700ed15ee66201f072225222e181a024baee13aca0b08c43a4416c7d4fba9`.
The open implementation regenerates both complete images with bounded LFSR
steps instead of retaining extracted arrays. It prepares at most 255 bytes in
caller-owned storage, rejects a short destination before mutation and returns
an affine read-only payload view. This is CPU-owned preparation only: no packet
header address, descriptor publication, fence or hardware ownership is
implied. Reproducing the vendor allocation and internal object layout remains
unnecessary.

The generic TX-buffer/header allocator is narrow enough to replace as well.
Current `r_sym_ble_4FZFpypyQDtGoyqc084f` is instruction-identical to named
initial `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` apart from symbol names. It
allocates `capacity + 0x12` bytes, zero-initializes a separate 24-byte header,
installs compressed buffer-base and `buffer+0x10` pointers in header words
`+0x04/+0x08`, sets the positional high image to `0x80a`, stores
`capacity << 3` in halfword `+0x10`, then writes packet-buffer bytes `+0x05 =
2` and `+0x06 = 0`. DTM always supplies capacity `0xff`; the resulting complete
header words after zero-initialization are `[0, base, 0x80a00000 | payload, 0,
0x000007f8, 0]`, where both pointers are the reviewed low-twenty-bit compressed
images.

The open packet slot consequently owns `0x12 + 0xff = 0x111` bytes with
four-byte alignment and validates the final aligned word as well as its base
and payload prefix against the controller-SRAM window. Preparation writes the
two allocator bytes, selector at `+0x10`, length at `+0x11` and only the
declared payload prefix at `+0x12`. A second preparation preserves bytes beyond
the new declared packet, matching the complete body's bounded copy/fill rather
than silently clearing unowned state. The slot and header image expose no raw
pointer, dereference or publication transition because the packet-engine
consumer and required device fence remain open.

DTM event duration is also exact through the microsecond-domain frontier.
Current `r_sym_ble_fkQ62juI6VgjMcLGg8XK` maps to named
`r_ble_ll_pdu_tx_time_get`, and current `r_sym_ble_xWK8Hh2AdoTzjH7mpE35`
is byte-identical to initial `g_ble_ll_pdu_header_tx_time_ro`: four little-
endian halfwords `[462, 80, 44, 720]`. Together the complete bodies calculate
`(length + 2) * factor + header` with factors 16, 8, 4 and 64 for internal
modes Coded S=2, 1M, 2M and Coded S=8. Complete
`r_sym_ble_bWydXXPAXzjyon1EdAMg` / `r_ble_lll_dtm_calculate_itvl` then rounds
`packet_duration + 249` upward to a 625-usec quantum and takes the maximum with
the optional 16-bit extended request without rerounding that request.

The complete tick tail is now closed as source-owned S31 behavior. In current
`r_sym_ble_4W45cnSk8tMbpqkpDdkQ` and named initial `r_ble_ll_init`, the value
stored at controller environment offset `+0x24` comes directly from current
`r_sym_ble_E4auD6oVVomYiG2Pm144` / named
`r_ble_lll_calc_us_convert_tick_unit`; both complete leaves return constant
one. Current `r_sym_ble_xHIFihMabllBUXiMYYoN` and
`r_sym_ble_GzLO7QvWzB8FTsdGLaBt`, mapped to the named initial usec-to-tick and
tick-to-usec leaves, are both identity returns. The DTM body consequently
stores `interval_ticks = interval_usecs`, computes a zero one-byte remainder
and never takes its `remainder == unit` correction branch.

The open timing types reproduce the complete arithmetic and positional
tick/remainder images for every HCI length and TX PHY. This still does not
sample a live controller clock, form an absolute deadline or grant scheduler
publication. Those are separate ownership and event-ordering transitions.

The transmitter window above the interval is now exact as pure scheduler
arithmetic. Current `r_sym_ble_pF0fMZSluGybO8KafMKl`, mapped to named
`r_ble_lll_init`, stores identity-converted literal 440 at LLL environment
offset `+0x04`. For an initial event, complete current
`r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` starts with current scheduler time plus that
440, literal 500 and the one-byte scheduler margin, then selects RF-ready time
when it is later under a signed wrapping comparison. The item window begins at
anchor minus the margin. Its end is anchor plus the TX duration calculated for
length `0xff`, deliberately reserving maximum packet capacity independently of
the requested payload length.

Complete recurring helper `r_sym_ble_huwoa5WRTRrAierQfN3B.part.1` first adds
the retained interval to the prior anchor. If the resulting start precedes the
fresh current-time sample, the vendor body repeatedly adds whole intervals
until it catches up. The open transition computes the equivalent ceiling-
division skip count in constant time, preserves the original phase and reports
how many intervals advanced. This removes a potentially long CPU loop from the
future async bottom half without changing the positional start/end images.
The initial and recurring windows compose directly into the reviewed DTM
scheduler-item transform. A live ordered clock sample, RF-ready result,
scheduler-margin owner and publication transition remain intentionally absent.

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

The two register slots in each selector pair are no longer directionless.
Current `r_sym_ble_LboRu27EaU8MV8Q7UUfZ` and
`r_sym_ble_ZzrExMrn8EDiTFI7PENK` are each 122 bytes and instruction-identical
over all three selector branches to named same-chip
`r_ble_phy_global_curr_rxptr_set` and
`r_ble_phy_global_next_rxptr_set`. Complete current
`r_sym_ble_HL6xpyhopnPTnSDqTURd`, mapped to the named memory-manager RX-link
reset, passes its selected chain to the current pointer setter and literal zero
to the next pointer setter. The restricted PAC therefore exposes current/next
RX slot roles while retaining selectors one through three and their element
contents as positional. This does not yet prove which selector DTM uses at a
given event, when hardware rotates the pair or when either chain is safe to
reclaim.

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
`SLEEP_TIMER_LATCHED_TIME_0` at `0x2010_10ac`. Exact same-chip historical-body
comparison names that function `r_btdm_sleep_timer_ticks_get`; it also maps
the two conversion helpers below without importing a historical address or
ABI claim. Complete
`r_sym_ble_3ISuZaEAZjklAjtGLFxW` converts the delta from an owned raw-time
anchor into the BLE scheduler epoch, handling either side of the anchor and
rounding the negative side by one when a discarded remainder is nonzero. For
the reviewed standalone HAL profile, the signed scale image is three: the
complete conversion helper shifts a positive raw-time delta left by two and
the inverse helper shifts a scheduler delta right by two. This proves the
arithmetic used by DTM, but not a public time unit or the wrap width of the raw
counter. The restricted PAC now exposes only fresh-read OR publication,
pending-bit observation and the complete latched word. The Bluetooth layer
owns affine `publication -> in flight -> read ready -> sample` phases and a
pure wrapping scheduler-epoch projection. Every pending observation returns
control immediately. A live owner, registered wake or bounded timer recheck,
and the MMIO ordering fence remain absent, so this is not yet a production
time source.

## What belongs in each open layer

| Layer | DTM responsibility | Current publication gate |
| --- | --- | --- |
| SVD / restricted PAC | Typed controller-window fields, complete publication/ack order, controller-time reads and the SRAM compression domain | Lock/modify and always-awake time-latch fields, exact images and wait predicates are finite. Live cross-owner MMIO and the remaining scheduler commands are absent. |
| HAL | Powered controller epoch, common RF wake, cache/device fences, timer conversion, same-core IRQ routing and bounded stop/quiesce | Clock/PHY/BTBB components exist separately; the time-scale transform and pure affine latch phases exist, but no reachable composed epoch, live latch owner or powered rollback exists. |
| Scheduler core | Affine event item, ordered deadline queue, insert/abort/complete states, hardware-head replacement and consistency check | One head lock/modify operation has affine event-driven phases; nine DTM scheduler-item words and their typed channel/role/PHY inputs have exact pre-insert transforms. The finished-item/recycle call chain and DTM callback edge are mapped; the open queue, raw status source, fences and abort path are absent. |
| Packet memory | Static aligned TX/RX/link-state slots with `CPU -> prepared -> hardware -> completed -> CPU` ownership | A statically sized DTM TX packet slot, its complete allocation-time 24-byte header image, caller-owned bounded payload preparation, compressed extent validation, positional list pairs, the exact eight-word link-state reset and nine-word scheduler-item event regions, software object kinds and the narrow RX result-word projection exist. Packet-engine header consumption, the publication edge and RX reclamation are incomplete. |
| LLL DTM | Parameter validation, channel/PHY/pattern image, TX/RX event state machine and receive counter | The complete channel domain, composed frequency lookup, role-dependent PHY/rate mapping, all eight bounded TX payload patterns and packet-duration/minimum-interval arithmetic are typed. Exact command roles, allocation rollback, substantial descriptor writes and the positional RX result split are mapped, but tick/remainder conversion, remaining field boundaries and result meaning are incomplete. |
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
2. **Compose the controller timebase owner.** The latch address/order,
   standalone scale arithmetic, affine nonblocking request phases and pure
   wrapping epoch projection now exist. Bind them to the sole task-side MMIO
   owner and a lost-wake-safe registered event or bounded timer recheck; prove
   the effective counter width, physical unit and required ordering fence
   before scheduler deadlines become live. Compose the now-exact DTM
   microsecond interval with the recovered environment remainder only after
   that value has a source-owned initialization path.
3. **Freeze the minimum descriptor.** Eight link-state reset words and nine
   scheduler-item event words, including their preservation masks, typed
   channel/PHY images and epoch projection, are now exact. Trace every
   additional word read by hardware for one TX-role event and prove the
   complete field masks for access address, CRC, whitening, power and next
   link; connect the now-exact TX packet/header image to its packet-engine
   consumer and publication fence. For RX, connect the now-exact
   low-24/high-byte result projection to validity, receive-count and
   reclamation semantics.
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
