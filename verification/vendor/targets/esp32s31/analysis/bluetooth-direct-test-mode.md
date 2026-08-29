# ESP32-S31 Bluetooth LE Direct Test Mode boundary

Verdict: **INCOMPLETE**. The complete ESP32-S31 DTM command bodies, their path
into the common scheduler, the conditional lock/modify transaction, the
insertion-begin/end outcomes and the later scheduler-item recycle callback are
now identified. The exact
eight-word reset region of its link-state image and the nine-word event update
of its scheduler item are also modeled without the vendor allocator. The
bounded allocation footprints, private RX/TX chain anchors, returned-buffer
accounting and append-decision entry are now recovered as well. The exact forty-channel
permutation/frequency composition and role-dependent PHY-rate images are typed
inputs to that event. Together these prove that DTM is a small Lower Link Layer
role built from controller-SRAM descriptors, a common radio wake operation and
an event-driven scheduler lifecycle. The current hardware-list index is also
now an exact typed field of the interrupt-time scheduler-state observation.
They do not yet prove the internal packet-engine latch/consumer of the private
DTM graph, the relation from that index or a finished-list bit to an affine
item, the hardware status-to-finished-mask boundary, memory visibility fences
or an affine RX buffer owner needed for an on-air open implementation.

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
| `r_sym_ble_VikJlxpO0kioDchKDFeI` | 278 | Reset one DTM controller-SRAM link-state image. It installs the compressed private TX head at `+0x00`, private RX tail at `+0x08`, a complete `0x71764129` access-address word, low 24-bit `0x555555` state and a rounded-power field. |
| `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` | 618 | Construct and schedule one DTM event. It accepts only role values one and two, wakes the common RF owner, writes the reviewed frequency/rate/role/timing scheduler-item images, begins a scheduler transaction and completes that transaction. |
| `r_sym_ble_XaQTq1AjtothrIFLgEeJ` | 362 | Construct TX context and payload. It accepts the reviewed payload-pattern selector domain, stores channel, length and PHY inputs, calculates the interval and repeatedly schedules TX role one until accepted. |
| `r_sym_ble_odiPxxFv9QenApESv5qy` | 166 | Validate and start a transmitter test, including channel `0..=39`, payload selector `0..=7` and PHY-dependent parameter checks before creating the TX context. |
| `r_sym_ble_x4t8591NiUiinayCMpjZ` | 116 | Construct RX context, store the channel/PHY inputs and repeatedly schedule RX role two until accepted. |
| `r_sym_ble_ssKOV2juzhIVJk3r8x6R` | 112 | Validate and start a receiver test, including channel `0..=39` and the accepted PHY selector domain. |
| `r_sym_ble_PptSRbXfefQwMVyO5jxP` | 52 | Process one received DTM buffer. From third argument offset `+0x04` it reconstructs a `0x2f00_0000 | (low20 << 2)` address. A zero compressed pointer takes the vendor fail-stop edge. The body returns `-1` when any low-24 bit of the word at returned-buffer offset `+0x0c` is nonzero; otherwise it copies the high byte at `+0x0f` into DTM link-state byte `+0x24` and returns zero. The byte meaning remains positional. |
| `r_sym_ble_kdHGLPeGDJlAvxmbjQ6e` | 336 | Recycle one completed DTM scheduler item. Complete control flow agrees with same-chip named `r_ble_lll_dtm_recycle_sch_item` (324 bytes). TX role one increments the DTM result count and reschedules only after zero item status. RX role two drains only when item status is zero; every completed returned header enters the append routine, while a result word with zero low 24 bits additionally updates the positional high byte and increments the wrapping 16-bit receive count. The ordinary append path reuses that header; positional bit `+0x10.0` instead substitutes a swap-reserve copy, and ownership of the detached original remains unresolved. An unexpected role takes the vendor fail-stop edge. |
| `r_sym_ble_9DFKLYZzjaztWMiPU4NR` | 168 | End the test, serialize the same 16-bit receive count as the two-byte Test End result, synchronously stop the common scheduler when a test is active, clear DTM active/count state, free the private graph and restore the default length state. |

The fixed `0x71764129` word agrees with the standardized DTM access address,
but its descriptor offset is still recorded positionally. Likewise, the
`0x555555` low-24-bit image is not promoted into a public `CRC_INIT` field
until an independent packet-engine consumer proves that field boundary.

The reset transaction itself is now reproduced as a non-publishable reviewed
region in the Bluetooth crate. It retains the exact low-twenty-bit compressed
TX head at `+0x00` and RX tail at `+0x08`, including the overlapping `+0x02`
halfword transform; five-bit rounded-power image at `+0x04`; high control bits at
`+0x14`; high-byte-preserving `0x555555` image at `+0x2c`; RX-only zero at
`+0x34`; complete `0x71764129` word at `+0x38`; and six-bit configuration image
at `+0x50`. Bounded inputs are rejected before any transform. The type exposes
no controller address, storage publication or hardware-ownership transition,
because the omitted descriptor words and their hardware consumer remain open.

Receiver event construction subsequently replaces link-state word `+0x34`
with `r_sched_timer_convertTimeToTicks(0)`, using the same retained
raw/scheduler epoch that projects the scheduler-item window. The transmitter
event does not write this word. The composed Rust plan now retains that
post-reset distinction; the value remains a CPU-owned positional word rather
than a named packet-engine field.

The scheduler-item update is now a second non-publishable reviewed region.
It reproduces the role byte at `+0x02`, bit-31 publication prerequisite at
`+0x04`, four-bit clear at `+0x08`, repeated two-bit rate image at `+0x14`,
seven-bit frequency plus low-nibble image at `+0x18`, RX-only complete
`0x000f0001` at `+0x2c`, two epoch-projected raw-time words at `+0x44/+0x48`
and the low-byte clear at `+0x4c`. Complete common
`r_btdm_sched_calc_seq_time` additionally stores raw start plus its dynamic
scheduler-environment lead at `+0x0c` and the wrapping raw window length at
`+0x10`. Complete scheduler initialization copies the reviewed two-word policy
to environment `+0x0c/+0x10`; the first word feeds both late-start checks and
the second feeds this sequence lead through `r_btdm_hal_util_us_to_ticks`.
The Rust scheduler derives its typed raw lead from the source-owned scheduler
config and retained Controller time scale instead of accepting an unrelated
raw image. A fixed-capacity timeline owns affine, generation-safe software
reservations and reproduces the strict signed wrapping overlap predicate. The
timeline is retained by the powered Controller runtime with an independent
capacity, and only the matching task endpoint can borrow it mutably. Its
retained timing policy consumes the first fresh Controller-time sample and
rejects a start at or before the guarded current time, matching the initial
deadline gate at the top of complete `r_btdm_sched_check_overlap_in_list`.
The timeline preserves duration while moving a candidate after every occupied
interval, treats touching boundaries as disjoint and applies bounded
backpressure without importing controller-SRAM links. The resolved affine
owner then consumes the second fresh sample and applies the identical guarded
check from `r_btdm_sched_calc_seq_time`; failure returns that owner for
explicit release. Only the resulting sequence-ready typestate can enter a DTM
plan, which forms both sequence words from the resolved window and retains the
reservation through graph and bookkeeping preparation. Before common
scheduler bookkeeping,
complete current `r_sym_ble_iHRqSCIgChmgSHj5W8W3` and named same-chip
`r_sched_txn_rmOverlapInsert` copy the link-state five-bit rounded-power image
into scheduler-item bits 24:20 while clearing bits 27:25; the composed Rust
plan now applies this cross-object transform to the same bound CPU-owned graph.
The timebase model now retains both exact
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

The executable target knowledge provider now binds only the exact current
linked event body and its byte load at caller-owned `arg0+0x0e` to the
reviewed inclusive channel domain `0..=39`. Generic Blobray retains that byte
as a real caller-memory read and uses the provider fact solely to prove that
all resulting accesses remain inside the immutable forty-byte table. Expanding
the domain by one value fails closed. This removes the former table-load
blocker without moving the target-specific channel fact into generic analysis
or duplicating it in project TOML.

The common controller assertion wrapper is also modeled as control flow rather
than writable memory. The exact current BLE and BR/EDR linked bodies return
when argument four is not level three; level three executes the deliberate
null-store/`ebreak` fail-stop sequence and cannot return to its caller. Blobray
now carries that outcome as a divergent platform boundary in generated
references. Only the exact reviewed bodies and linked addresses receive the
summary, and only the memory diagnostic at the explicitly covered fail-stop
site is retired. Other stores and control-flow limitations remain fail-closed.
This removes the former assertion-store blockers from both controller graphs
without pretending that address zero is RAM or flattening non-assert logging
levels into an unconditional trap.

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
steps instead of retaining extracted arrays. Its standalone verifier prepares
at most 255 bytes in caller-owned storage, rejects a short destination before
mutation and returns an affine read-only payload view. Production composition
instead consumes the bound graph owner, copies every declared payload byte and
returns a typed packet-ready graph retaining the validated pattern and length.
Only that state can enter a transmitter event plan; the receiver plan accepts
the ordinary graph. The TX proof survives the positional-event and scheduler-
bookkeeping transitions. All of these states remain CPU-owned: no packet
header publication, fence or hardware ownership is implied.

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
published above the PAC boundary as `BluetoothDtmRxResultProjection`; no
semantic name is assigned to the high byte. The complete recycle body proves
the next layer: an accepted word updates that byte and increments the DTM
environment's 16-bit counter with wrapping arithmetic, while a rejected word
changes neither value. Test End serializes that same counter. Every result
branch enters the append routine; its ordinary branch reuses the returned
header, while bit `+0x10.0` substitutes a swap-reserve copy and detaches the
original. The open LLL transition reproduces that accounting and requires an
unconditional handoff to the separate append decision, but cannot yet
manufacture the affine completed-header owner, decide ownership after the
swap-reserve branch or establish the device-to-CPU visibility edge.

The allocation footprint is finite and does not require a heap ABI. One DTM
graph requests a `0x84`-byte link state, `0x60`-byte scheduler item,
`0x48`-byte scheduler context, three separately allocated `0x18`-byte headers,
a `0x111`-byte TX packet and a `0x11d`-byte RX packet. Four-byte compressed
addressing makes the respective packet allocation footprints `0x114` and
`0x120`; it does not by itself prove cache or DMA alignment. The three headers
are the bound RX head/tail, a software swap reserve and the bound TX head/tail,
not three interchangeable RX descriptors. Link-state offsets `+0x68/+0x70`
hold RX head/tail, `+0x6c/+0x74` hold TX head/tail and `+0x78` holds the swap
reserve. The separate DTM environment grows to `0x28` bytes in the current
revision and initializes its positional byte `+0x24` to `0x7f`.

The complete allocation order is link state, scheduler context, scheduler
item, bound RX packet/header, unbound swap header and bound TX packet/header.
The finite failure results are `-1` for link-state allocation, zero for the
separate context and `2`, `3`, `4`, `5` for scheduler, RX, swap-header and TX
stages. Every partial failure walks the same graph-free path before releasing
the separate context. The open memory crate reserves the resulting 936-byte
aligned graph as one non-movable static allocation retained by a movable CPU
owner; it deliberately does not preserve these allocator result integers as
its API. Target binding derives
every real field address, validates the complete extent against physical
internal SRAM rather than the wider compressed-pointer syntax, and only then
installs the exact allocation headers and private-chain anchors. Native tests
use a separate synthetic typed base, while a failed binding returns the
unchanged static allocation.

The allocation body also supplies a fixed prefix before any per-event reset.
After zero-initialization it writes `0x1e00` to link-state halfword `+0x30`.
The generic scheduler allocator sets item byte `+0x02` to include `0x30`, and
the DTM body retains that prefix while setting its allocation bit; the resulting
zero-based word `+0x00` is `0x00300000`. DTM additionally installs the
compressed scheduler-context link at item `+0x04`, the compressed link-state
link at `+0x08` and sets the positional low-twenty-bit image `0x0007bdef` at
`+0x24`. The generic allocator also copies the common scheduler default at
module-environment `+0x0c` while clearing bit 21. Complete current
`r_sym_ble_pF0fMZSluGybO8KafMKl`, mapped to named same-chip
`r_ble_lll_init`, initializes that source word to `0xffffffff`; the resulting
item `+0x1c` image is therefore `0xffdfffff`. DTM's later OR with
`0x18000000` does not change that complete image. The open static binding now
installs these unconditional fields before returning the CPU owner. The
configuration-derived low twelve bits at item `+0x20` are
`(ble_multi_adv_instances + 1 + nimble_max_connections +
private_options_halfword_14 + 4 + ble_ll_sync_cnt) & 0x0fff`. The three named
inputs and their exact structure positions are public in the pinned ESP-IDF
`esp_bt_controller_config_t::ble`; the current private `0x2e`-byte table
independently confirms halfword `+0x14` as `5`, but its semantic name is still
unproven. Static binding therefore requires all four source-owned values,
retains the private value as an explicit positional fact and has no implicit
vendor-build default. The resulting allocation image is installed before the
CPU owner is returned.
The vendor software kind and callback pointer are not copied into the graph;
the open scheduler must replace them with typed dispatch and affine ownership.

The RX allocator requests `capacity + 0x1e`, writes the two-byte
`capacity + 2` image at packet offsets `+0x05/+0x06`, and starts its zeroed
header as `[0, packet, 0x80800000, 0, 0, 0]`. Before reuse, the append path ORs
`0x00ffffff` into packet word `+0x0c`, preserving the high byte, writes
`0xffff` at packet halfword `+0x18` and clears the header completion bit. For
capacity `0xff`, the packet bytes are therefore `1, 1`. These exact CPU-owned
transforms still do not reveal the hardware producer of the result word.
If a returned header has positional bit `+0x10.0` set, append copies its full
24-byte image into the swap reserve, detaches the original packet pointer and
uses the copy as the append candidate. The source of that bit and the final
ownership of the original header are not proven for DTM, so the open live path
must quarantine that observation until the special case is closed.

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
to the next pointer setter for selectors one and two, then clears current
pointer flag bit 20 while preserving the published low-twenty-bit head.
Selector three deliberately skips both setters. No other same-chip caller of
either setter exists in the reviewed archive.

The normal `update_global_rxlink` insertion classifier reads scheduler-item
kind byte `+0x4d`:
kind two selects selector one, and every other admitted kind selects selector
two. The scan allocator writes kind two and explicitly calls the direct global
insertion path, proving selector one as the scanner RX chain. Connection,
sync and other admitted non-scan kinds classify to selector two. DTM writes
kind five, but its allocator also enables `direct_allocate` and
`skip_rxbuf_alloc`; the generic precheck then bypasses global insertion, and
the DTM object has no direct-insertion call. Thus DTM owns a private RX graph,
not a selector-two binding. Its software publication path is exact: before
each event, DTM reset copies the private TX head from link-state `+0x6c` into
compressed word `+0x00` and the private RX tail from `+0x70` into compressed
word `+0x08`, while scheduler-item `+0x08` already points at that link state.
The restricted PAC retains positional
selector names, while the new controller-memory layer exposes only the exact
scan/non-scan global routing. Hardware current/next rotation, the internal
engine and latch that consume the DTM link-state pointer, and the point at
which either chain becomes CPU-owned remain unproven.

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
sequence and register transaction. It first calls insertion-begin, then always
calls merge-list removal. Lock/modify is reached only when insertion-begin
returns outcome four and the common scheduler environment enables that path.
Its request pointer comes from the item selected by merge/list state (or the
matching manager-retained current item), not unconditionally from the DTM item
originally submitted for insertion. The current complete body directly uses:

- `0x2010_1218` as `SCHEDULER_LOCK_MODIFY_REQUEST`: bit 31 is START, bits
  19:0 carry the compressed SRAM pointer and bits 30:27 return a positional
  four-bit result;
- `0x2010_107c` bit 31 as `SCHEDULER_STATE.BUSY`;
- `0x2010_136c` through ordered clear/update operations that combine a
  zero-based four-bit hardware-list index with the selected scheduler item;
- a second independent publication/wait when the selected list head changes.

The list index is no longer positional. Named `r_sched_txn_insertIntoList`
loads it from scheduler-manager byte `+0x18` and passes the same value to
`r_btdm_sched_insert_with_lock_modify`, which uses it for insertion begin/end,
the sixteen-entry hardware-list table and the `0x2010_136c` low nibble. DTM's
separately allocated `0x48`-byte scheduler context is zeroed and embeds that
manager at `+0x2c`; its list-index byte is therefore context `+0x44`, value
zero. That proves DTM's target list, but it does **not** prove that the original
DTM scheduler item is the lock/modify request pointer. On an idle scheduler,
insertion-begin returns outcome three, so the lock/modify branch is not taken;
insertion-end owns the later conditional head publication and scheduler RUN.
Outcome four clears command-zero START, while outcome five handles the captured
current-head path by publishing the submitted item before clearing command-one
START. The current insertion-end then observes scheduler BUSY before sleep
policy. A busy scheduler performs no later publication. Once idle, disabled
sleep publishes the manager's software-list head without RUN; enabled sleep
continues to the submitted-item status. It publishes that item and runs only
while the item still carries the typed in-flight status; a status already
changed by hardware needs no further MMIO. The
Bluetooth core now represents this short-circuited semantic plan without raw
result codes or register images. It still exposes no safe DTM-to-lock/modify
admission until merge selection and manager-list ownership are affine.

The later common insertion edge supplies the missing source-program order but
not yet its memory-model contract. It clears item byte `+0x4e`, writes
`0xffff_ffff` to item status `+0x38`, links the item, publishes its compressed
head at `0x2010_b000 + 0x10 * list` and only then conditionally writes one to
`SCHEDULER_CONTROL` at `0x2010_1000` as the kick. No explicit RISC-V fence
appears in the complete DTM, memory-manager or common-scheduler bodies.
Consequently an open owner must prove whether the SRAM window is coherent or
insert a release/cache-clean edge before head/kick and a matching
acquire/cache-invalidate edge after completion before reading item status or
RX data. Absence of a vendor fence instruction is not evidence that ordinary
Rust memory accesses are sufficient.

Before and after request publication the current body waits only while both
BUSY and START are set. Once that conjunction is false, it publishes result
zero if BUSY is clear; otherwise it publishes request bits 30:27. That byte is
sent as broker-type-3 event one. It is not radio completion: the normal BLE
base scheduler subscribes to broker type zero, while the optional named
`gapFinder_stack_enable` subscribes to type three. Its current
`r_sym_ble_YX2xSKJJFZFRxyO5RlZC` callback only loads and logs the result for
event one, changes no scheduler or DTM state and returns zero. The exact
meaning of the sixteen diagnostic values remains unknown.

The restricted PAC now validates the pointer and typed hardware-list index,
performs the two ordered fresh-read field updates and the complete request
publication through generated accessors, and represents the two-word decision
observation without joining their physical owners. The Bluetooth crate adds
affine `awaiting publication` and `in flight` phases. Raw PAC/HAL publication
is unsafe because a syntactically valid controller-SRAM address does not prove
descriptor initialization or lifetime. The free Bluetooth-level phase
constructors are no longer public. Instead, the task runtime can admit only a
consumed `BluetoothDtmSchedulerBookkeepingPrepared`; its pending state retains
the pinned graph while the sole worker owns the matching request.
Each `observe` call evaluates exactly one fresh event and returns `Waiting` to
the executor when the conjunction remains active; it contains no polling loop,
allocator, waker or RTOS dependency. Its terminal value is consequently named
a publication result, not a completion. A prepared DTM graph can consume that
result only when both the scheduler-item address and typed hardware-list index
match. One bounded runtime attempt either returns the unchanged pending owner
or consumes the exact result and performs that identity join. Success enters a
non-cancellable CPU-owned join state: it still grants no hardware-head, RUN,
descriptor-visibility or radio-completion authority.
The powered task endpoint contains the finite live MMIO step and the ISR side
publishes value-only observations through the durable handoff. A controller
task that drives admission, waits and result consumption as one operation is
still absent, so this does not yet expose a live DTM command.

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

The complete current 12-byte leaf `r_sym_bt_iFvwGI2tL5M1WM3fIkHq` is
instruction-identical to same-chip named
`r_btdm_sched_get_current_link_index`: one `SCHEDULER_STATE` read returns
exactly bits 23:20 as a zero-based current hardware-list index. The restricted
PAC retains that semantic field in the later source-124 scheduler snapshot,
and the Controller event carries it without exposing a register image. This
proves active-list identity only. It does not prove which affine item is on
that list, that the item has completed, or that its SRAM is CPU-visible.

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

The scheduler-item status word also has a finite ownership meaning. Before
hardware insertion, the common transaction clears the software completed-link
at item `+0x54` and writes `0xffff_ffff` to status `+0x38` as its in-flight
sentinel. The finished-item picker skips exactly that sentinel and queues any
other observed status. DTM treats only status zero as successful role work.
Its callback first returns the scheduler item to the private chain at
link-state `+0x64`, then performs TX/RX accounting and optional rescheduling.

Normal Test End deletes queued kind-five items, frees the deleted chain,
synchronously stops the common scheduler, clears active/count state and only
then frees TX packet/header, RX packet/header, the swap reserve, the private
scheduler chain, link state and context. Scheduler stop requests shutdown and
waits for `SCHEDULER_STATE.BUSY` to clear, but that predicate alone does not
prove an empty software completed queue, absence of an already-entered
callback or return of the sole item token. The open shutdown therefore needs a
`Stopping` state, callback reschedule gate, async busy-clear observation and a
bottom-half join before graph reclamation.

This is the reference software edge at which a finished DTM item is handed
back to its role. It is the correct boundary for an affine
`HardwareOwned -> Completed -> CpuOwned` open lifecycle. It still does not by
itself prove which raw interrupt/status bits produce each hardware-list bit,
or the memory fence required before callback reads. The per-buffer drain,
append-entry and ordinary re-arm rules are now exact; the swap ownership,
visibility and real ownership transition remain prerequisites for a live path.

The later current-revision software-list removal return gate is now exact as
well. The historical same-chip body names the caller
`r_sched_txn_removeSwList`; the current body replaces its old busy-state
assertion tail with `r_sym_bt_FCfM3hAXphsk1qERleGZ`. One attempt returns only
after an idle scheduler observation, positional command-zero status 26 and
positional command-one status 18, with the two command reads short-circuited in
that order. The vendor repeats this attempt and diagnoses every 10,000 misses.
The restricted PAC and HAL instead expose one split-owner finite observation:
`Pending` returns control to the executor and `Ready` permits the future
software-list removal state machine to advance. This is not yet a descriptor
ownership return; no current controller worker composes the predicate with
the finished-list-to-item mapping or an acquire fence.

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
control immediately. The same powered Controller owner now exposes request,
generation-scoped abandon and one-observation recheck operations; PAC
publication includes its device fence. A registered wake or proven bounded
recheck source, effective counter width and physical unit remain absent, so
this is not yet a deadline-ready production time source.

## What belongs in each open layer

| Layer | DTM responsibility | Current publication gate |
| --- | --- | --- |
| SVD / restricted PAC | Typed controller-window fields, complete publication/ack order, controller-time reads and the SRAM compression domain | Lock/modify, always-awake time-latch and software-list-removal fields, exact images, wait predicates and finite live MMIO are present. The later interrupt-time scheduler snapshot also returns the typed current hardware-list index. The removal gate preserves the current short-circuit read order without a polling loop. The remaining scheduler commands are absent. |
| HAL | Powered controller epoch, common RF wake, cache/device fences, timer conversion, same-core IRQ routing and bounded stop/quiesce | The powered Controller retains the unique live latch owner and exposes finite request/recheck operations with generation-safe cancellation drain. A proven wake/recheck source, physical counter contract and powered rollback remain absent. |
| Scheduler core | Affine event item, ordered deadline queue, insert/abort/complete states, hardware-head replacement and consistency check | The conditional lock/modify MMIO operation has affine event-driven phases and a typed item/list request, but admission is explicitly unsafe until the common scheduler owns the merge-selected item for the complete transaction. DTM's zeroed private scheduler context proves target list zero; it does not prove request-pointer identity. The current insertion-begin outcomes are exposed as semantic `Unlocked`, `ExecutionLockRetained` and `CurrentHeadReconciled` states, and a staged pure insertion-end plan preserves command-clear/head ordering plus the current BUSY, sleep-policy and typed item-status short circuits without raw result or register images. Eleven DTM scheduler-item words and their typed channel/role/PHY/sequence-lead inputs have exact pre-insert transforms. A bounded source-owned timeline resolves strict wrapping overlaps, retains affine generations and applies fixed-capacity backpressure without exposing the vendor list ABI. The powered runtime retains that timeline for the whole epoch, and its powered task endpoint joins the sole mutable software workers to the matching task-side HAL owner. There is deliberately no safe DTM admission: manager software-list ownership, merge-selected ownership and execution of the insertion-end plan must be composed first. The overlap-resolved reservation requires the second fresh deadline sample before the DTM event typestate can form sequence words and survives graph/bookkeeping preparation and cancellation. The current hardware-list index and bounded finished-list drain share the exact list domain, but neither selects an affine item. Broker notification, hardware-head publication, raw finished-status source, fences and abort/completion composition remain absent. |
| Packet memory | Static aligned TX/RX/link-state slots with `CPU -> prepared -> hardware -> completed -> CPU` ownership | A separate no-heap controller-memory crate reserves every reviewed per-event DTM link-graph allocation with four-byte alignment: link state, scheduler item/context, three role-specific headers and complete TX/RX packet slots. The separate `0x28`-byte DTM environment remains LLL state. Target binding gives a movable CPU owner one non-movable static allocation, validates the complete physical SRAM extent before mutation and installs the bound headers, five private-chain anchors and scheduler-item link. TX preparation consumes that owner and yields packet readiness only after a total declared-byte copy; mutable TX-slot access is unavailable after binding. TX/RX re-arm sentinels, list routing and positional result parsing are also exact CPU-owned components. Production placement ownership, the packet-engine latch/consumer, cacheability/fences and affine hardware completion/reclaim states are absent. |
| LLL DTM | Parameter validation, channel/PHY/pattern image, TX/RX event state machine and receive counter | The complete channel domain, composed frequency lookup, role-dependent PHY/rate mapping, all eight bounded TX payload patterns, packet-duration/minimum-interval/tick arithmetic, constant-time event catch-up and one-word RX accounting transition are typed. TX and RX event plans are distinct states: TX requires a prepared graph and retains its exact pattern/length through scheduler bookkeeping, while RX accepts an ordinary bound graph. Exact command roles, allocation rollback, descriptor chain anchors, wrapping received-packet count and unconditional handoff to the append decision are mapped. The ordinary re-arm is fail-closed on the swap bit; remaining field meanings, swap ownership, live item/buffer ownership, abort and quiescence are incomplete. |
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

1. **Own the complete scheduler insertion transaction.** The current
   insertion-begin outcomes and insertion-end short-circuit plan are now
   semantic Rust states. Add the affine manager software list and merge
   selection that retain both the submitted item and any distinct selected
   item. Only `ExecutionLockRetained` may feed the existing lock/modify worker,
   and only after the environment gate with that exact selected pointer;
   `Unlocked` must reach insertion-end without fabricating a request. Execute
   the typed end plan only after proving descriptor visibility and the
   head/broker/RUN prerequisites. Do not couple the lock/modify result to radio
   completion or create a task-side alias for `SCHEDULER_STATE`.
2. **Close the controller timebase source.** The latch address/order,
   standalone scale arithmetic, affine nonblocking request phases, pure
   wrapping epoch projection and sole powered task-side MMIO owner now exist.
   Add a lost-wake-safe registered event or proven bounded timer recheck and
   establish the effective counter width and physical unit before scheduler
   deadlines become live. Compose the now-exact DTM
   microsecond interval with the recovered environment remainder only after
   that value has a source-owned initialization path.
3. **Close the hardware-consumed descriptor.** Eight link-state reset words,
   eleven scheduler-item event words, every allocation footprint, chain anchor,
   TX/RX header image and RX re-arm transform are now exact. Trace every
   additional word read by hardware for one TX-role event and prove the
   complete field masks for access address, CRC, whitening, power and next
   link. Bind the private `item -> link state -> RX tail` path to its internal
   packet-engine latch and the required publication fence.
4. **Advance the bound graph into hardware ownership.** The no-heap aligned
   capacity, non-forgeable physical-SRAM binding, exact allocation-time links,
   consuming TX packet readiness and role-specific event preparation now
   exist. Add `HardwareOwned/Completed/Recycled` only after the private packet-
   engine latch and release/acquire rules are proven; prevent mutation after
   publication and reuse before callback, abort or quiescence. The exact
   wrapping receive count and append-decision outcome may be consumed only by
   the completed owner; the swap-reserve observation must remain quarantined
   until its detached-header ownership is proven.
5. **Implement the open scheduler model.** Use a fixed-capacity ordered queue,
   explicit `Prepared/Scheduled/Running/Completed/Recycled/Aborted` states and
   a single hardware-head owner. Retain each request's typed list index beside
   its affine item; DTM uses the now-proven list zero. Feed a finished-list bit
   into an item-selection/recycle transition only after proving ordering within
   that list. The interrupt-time current index is not a completion token. Model
   selector-6 as an internal invariant. Keep scan resume outside the DTM
   feature graph.
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
- exact item ordering/selection within a finished hardware list and the affine
  owner returned by that selection; DTM's hardware-list assignment is zero,
  but a list index alone is not an item completion;
- exact controller-time raw width, wrap and physical unit;
- complete S31 descriptor field boundaries, all hardware-read words and the
  internal latch that consumes DTM's private link-state RX pointer;
- exact hardware current/next rotation for the normal scan/non-scan global
  lists; DTM deliberately bypasses those selector pairs;
- exact primary/NRT bits for TX done, RX done, timeout and abort in the DTM
  feature configuration, including their mapping to finished-list indices;
- meaning of the validated RX result high byte, plus length/CRC/RSSI extraction
  and the acquire fence preceding the now-mapped buffer-return order;
- source and ownership semantics of the rare returned-header swap bit
  `+0x10.0`;
- bounded abort plus powered quiescence when an event is scheduled or running.

These are the next blockers. HCI packet syntax, Trouble integration and an
RTOS abstraction are not.
