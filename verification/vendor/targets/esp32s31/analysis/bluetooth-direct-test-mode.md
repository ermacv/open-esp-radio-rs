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
The list-zero relation from finished-list observation to one affine DTM item,
the hardware status boundary, device fences and the capacity-one RX owner are
now explicit. First and recurring TX/RX preparation retain immutable command
identity and committed phase, and feed the same typed publication, RUN,
completion and recycle chain. The reviewed standalone scheduler margin is now
owned by the Rust scheduler policy. The terminal powered BLE-PHY owner also
combines its retained always-awake selection with a later completed
generation-keyed time request into one opaque non-copyable RF-ready authority.
Initial TX/RX consume current before RF-ready, recurring RX consumes RF-ready
before current, and recurring TX consumes current without an RF-ready phase.
Software unlink and the globally identity-branded capacity-one return mailbox
are now armed atomically;
primary capture, both ordinary publications and mailbox routing share that
serialization boundary, and only the exact first post-arm event can join the
unlinked owner. No-work and command-pending outcomes re-arm the same mailbox
identity and generation before returning.
This still does not provide the sleep-enabled RF wake branch, proven source-124
command-ready causality, unrelated finished-list dispatch, a source-127
expiration consumer or the remaining hardware-consumed descriptor semantics.
The open implementation does now have a concrete affine session loop, deferred
in-flight Test End, exact-once response publication and a target-only command
actor. Production cold start composes those pieces into a Wi-Fi-shaped
`{ hci, runners: { hardware } }` system, and the ESP32-S31 example polls the
standard `bt-hci` read side concurrently with the sole hardware runner. At the
fully recycled CPU-owned boundary, active TX and RX owners enter `TestEnded`,
which retains the role report together with a `Reclaimed` graph that can
reinitialize the same pinned allocation from the configuration retained by its
binding.

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
linked S31 instruction extents and are limited to the locally recovered
effects; a complete extent does not imply complete symbolic semantics. One
explicitly labelled dead-stripped raw-archive body is
retained only as corroboration of a positional transform already present in
the linked DTM recycle callback. The initial public S31 archive retains
descriptive scheduler and DTM symbols; its role names are accepted only where
control flow, call order and instruction extent agree with the current body.
The C61 archive is used under the same stricter role-only rule for BLE
functions whose S31 history is obfuscated. Neither older S31 code nor C61
supplies current register behavior or ABI evidence. Object files and
disassembly products remain temporary review inputs and are not repository
artifacts.

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
| `r_sym_ble_odiPxxFv9QenApESv5qy` | 166 | Validate and start a transmitter test, including channel `0..=39`, payload selector `0..=7` and PHY-dependent parameter checks before creating the TX context. A nonzero active image at environment offset `+0x08` returns standardized `Controller Busy` (`0x3a`) before parameter validation or allocation. |
| `r_sym_ble_x4t8591NiUiinayCMpjZ` | 116 | Construct RX context, store the channel/PHY inputs and repeatedly schedule RX role two until accepted. |
| `r_sym_ble_ssKOV2juzhIVJk3r8x6R` | 112 | Validate and start a receiver test, including channel `0..=39` and the accepted PHY selector domain. A nonzero active image at environment offset `+0x08` returns standardized `Controller Busy` (`0x3a`) before parameter validation or allocation. |
| `r_sym_ble_eTAd...` | 502 | Pop one completed RX buffer from the private link-state chain. Its complete control flow agrees with named `r_ble_lll_get_rxed_buffer` (496 bytes): it follows the full RX-head pointer, checks the volatile completion flag, validates that the packet result and positional auxiliary halfword no longer retain their re-arm sentinels and advances a packetless predecessor only when its successor is complete. |
| `r_sym_ble_9Hls...` | 456 | Append one returned RX buffer. Its complete control flow agrees with named `r_ble_lll_append_rx_buffer` (452 bytes): a terminal tail is copied into the detached reserve, its original header becomes the packetless predecessor, the copied header becomes the armed tail and the two fixed slots alternate on later completions. |
| `r_sym_ble_PptSRbXfefQwMVyO5jxP` | 52 | Dead-stripped raw-archive corroboration only, not a linked effect authority. From third argument offset `+0x04` its complete raw body reconstructs a `0x2f00_0000 | (low20 << 2)` address. A zero compressed pointer takes the vendor fail-stop edge. It returns `-1` when any low-24 bit of the word at returned-buffer offset `+0x0c` is nonzero; otherwise it copies the high byte at `+0x0f` into DTM environment byte `+0x24` and returns zero. The byte meaning remains positional. |
| `r_sym_ble_kdHGLPeGDJlAvxmbjQ6e` | 312 | Current linked effect authority for recycling one completed DTM scheduler item. Its complete linked instruction extent contains the exact local RX-result edge; whole-function symbolic semantics remain incomplete. Its control-flow shape agrees with same-chip named `r_ble_lll_dtm_recycle_sch_item` (324 bytes). The callback returns the item to the private chain first. Status zero enables role accounting: TX role one increments the DTM result count, while RX role two drains returned buffers; it directly rejects a result word whose low 24 bits are nonzero, otherwise updates the positional high byte and increments the wrapping 16-bit receive count. When the environment remains active, both zero and nonzero item status may continue into the next-event reschedule path; nonzero status skips accounting rather than suppressing rescheduling. With DTM capacity one, the first returned tail is terminal and necessarily takes the two-header swap rotation; the detached original remains inside the fixed graph as the packetless predecessor and becomes the next reserve. An unexpected role takes the vendor fail-stop edge. |
| `r_sym_ble_9DFKLYZzjaztWMiPU4NR` | 168 | End the test and return success. The body first serializes the shared 16-bit DTM count as the two-byte Test End result; a zero active image at environment offset `+0x08` then returns zero without entering teardown. When a test is active, it removes queued kind-five items, synchronously stops the common scheduler, clears DTM active/count state, frees the private graph, restores the default length state and also returns zero. Because the recycle callback increments this same count for successful TX events, the current vendor path can expose a nonzero transmitter result. Bluetooth Core requires the packet count ending a transmitter test to be zero, so the open HCI policy deliberately does not copy that vendor behavior. |

The current linked writers distinguish the CPU-owned DTM environment state
from the private controller-SRAM graph:

| Environment offset | Current linked meaning |
| --- | --- |
| `+0x08` | Active image. TX and RX context construction clear it before setup and store exactly one after setup; command admission and Test End test it for zero/nonzero state. It is not a graph pointer. |
| `+0x14` | Full link-state pointer installed by graph allocation and consumed by event construction. This is the environment's private-graph root. |

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
reservations and reproduces the strict signed wrapping overlap predicate.

The phase split at scheduler-item `+0x2c` is now explicit. Complete initial
body `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` writes `0x000f0001`; the receiver branch
of complete recurring helper
`r_sym_ble_huwoa5WRTRrAierQfN3B.part.1` replaces that word with literal one
after the same item has returned from completion. No other reviewed descriptor
word changes solely because of this phase; the timing words change because the
two phase-specific window policies differ.

The timeline is retained by the powered Controller runtime with an independent
capacity; neither executor nor task endpoints can borrow it mutably. Initial
event admission consumes one fresh Controller-time sample and rejects a start
at or before the guarded current time, matching the deadline gate at the top of
complete `r_btdm_sched_check_overlap_in_list`. Initial insertion preserves
duration while moving a candidate after every occupied interval, treats
touching boundaries as disjoint and applies bounded backpressure without
importing controller-SRAM links. The resolved initial owner then consumes a
second fresh sample and applies the guarded check from
`r_btdm_sched_calc_seq_time`.
Complete recurring helper `r_sym_ble_huwoa5WRTRrAierQfN3B.part.1` instead
enters `r_sched_txn_rmOverlapInsert` directly without the initial
`r_sched_txn_delayIfOverlap` call, so its distinct recurring reservation
consumes only the fresh sequence sample. No admission sample is fabricated for
either recurring role. The open recurring path retains the exact raw window and
rejects an occupied collision without mutation until the vendor removal policy
has a reviewed affine model; it never borrows initial displacement semantics. A
rejected sequence gate returns either exact phase owner for explicit release.
Only the resulting common sequence-ready typestate
can enter a DTM plan, which forms both sequence words from the phase-bound
retained window and retains the reservation through graph and bookkeeping
preparation. The target Controller now drives this split with the same private
generation-keyed latch worker: initial TX/RX publish admission, reserve only
after that exact sample completes, and publish sequence only after reservation;
recurring TX/RX reserve first and publish only sequence. The sample type and
all constructors are crate-private. Cancellation and Drop release an occupied
reservation through its exact scheduler owner before a late latch result can
only enter the common orphan drain. Before
common scheduler bookkeeping,
complete current `r_sym_ble_iHRqSCIgChmgSHj5W8W3` and named same-chip
`r_sched_txn_rmOverlapInsert` copy the link-state five-bit rounded-power image
into scheduler-item bits 24:20 while clearing bits 27:25; the composed Rust
plan now applies this cross-object transform to the same bound CPU-owned graph.
The timebase model now retains both exact wrapping conversion directions; the
inverse truncates discarded scheduler bits toward its anchor, as the complete
helper does. Every completed later scheduler-current observation preserves its
forward scheduler image while moving the raw anchor to that exact sample. This
reanchor is necessary because positive shifting scales have wrapping aliases:
the first and latest epochs can agree in scheduler space while only the latest
raw anchor maps a later scheduler deadline back to the correct raw wrap.

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

The scheduler-margin source is closed for the reviewed standalone profile.
Current member `66.o` function `r_sym_ble_TtdgZbQxHLEPeNfBtjqO` is
instruction-identical to named same-chip `r_ble_lll_sched_init`. It stores the
low byte of `usecs_to_ticks(config[1] + ll_tick_unit +
private_options[0x10] - 1)` in the LLL scheduler environment. Reviewed
`config[1]` is 46, the tick unit and conversion are one, and committed current
data object `r_sym_ble_Me5fSEJ6D19ZRNyzFooG` initially contains halfword 60 at
private offset `+0x10`. Complete `r_ble_controller_init` overwrites that
halfword with 61 before it calls the scheduler initialization chain. The exact
standalone margin is therefore `46 + 1 + 61 - 1 = 107`. The separate scheduler
guard remains 40; neither 40 nor 46 alone is margin.

Current member `64.o` wrapper `r_sym_ble_4QeP6vZAoSzLLHdFgwD0` calls
`r_sym_bt_Ceh2khbCcopEBybBO6Z5` and converts its result through
`r_sym_ble_3ISuZaEAZjklAjtGLFxW`. Named same-chip roles are
`r_btdm_sleep_enable_now` followed by `r_sched_timer_convertTimeToUs`. The null
or disabled sleep-environment branch performs no RF MMIO and obtains a fresh
controller tick; the sleep-enabled branch first crosses still-open wake helpers
and then also obtains a fresh tick. RF-ready is therefore the scheduler-domain
instant returned after synchronous RF-enable-now policy, not a PHY status bit
or caller timestamp. Complete initial TX/RX bodies acquire current before
RF-ready; recurring RX acquires RF-ready before current; recurring TX has no
RF-ready call. The reviewed always-awake branch can be composed without new
register access, while the sleep-enabled wake branch remains outside the
current implementation.

The open standalone composition follows that exact boundary. The affine
always-awake marker remains nested with the settled Bluetooth PHY client and
completed BLE-PHY transaction. A completed request through that combined owner
projects its private sample into the retained scheduler epoch and mints one
opaque non-`Copy` RF-ready token. The token has no public constructor, image or
decomposition edge and is consumed by the same Controller-bound preparation.
It does not reanchor the epoch: only a completed fresh-current observation does
that. Cancellation or Drop abandons the exact keyed request into the existing
sample-discarding orphan drain and cannot turn a late sample into RF-ready.

The source-owned standalone scheduler policy now retains the exact margin 107;
its type, construction and image are private, and public DTM preparation has no
margin input. The timing types reproduce the complete arithmetic and positional
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
The initial TX window and both RX phases now compose directly into the reviewed
DTM scheduler-item transform. Initial RX shares the `440 + 500 + margin`
anchor lead and uses the body's literal 1000-tick end. In the receiver branch
of complete recurring helper `r_sym_ble_huwoa5WRTRrAierQfN3B.part.1`, a fresh
current sample, the scheduler configuration's `+0x2a` guard value, the retained
margin and literal 15 form the nominal anchor; fresh RF-ready wins under the
same signed wrapping comparison, and the end again adds literal 1000. The
window privately retains `Initial` or `Recurring`, and the
memory codec selects the corresponding full-initial or reuse configuration.
Distinct Rust window types prevent either phase from entering the other's
Controller edge. Controller-time RF-ready, admission and sequence samples are
now non-public affine phases. The Controller typestate enforces initial current
then RF-ready, recurring RX RF-ready then current and recurring TX current only
before the matching reservation/sequence edge. The recurrence core consumes
the opaque result and cannot accept a detached caller instant.

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
branch enters the append routine. Complete get/append bodies now show that bit
`+0x10.0` is software terminal/successor state, not an unknown hardware
condition. With capacity one, two bound header slots alternate deterministically
between packet-bearing tail and packetless predecessor/reserve. The lower
memory transaction consumes the fenced exact completion/removal owner and
validates the complete two-slot topology before its first write. The upper LLL
consumes the result into the same non-copyable session before the lower owner
can perform the matching volatile append/re-arm suffix. The returned active RX
owner retains that session, immutable command facts and committed phase;
recurring preparation consumes it losslessly and re-enters the common
publication path.

The allocation footprint is finite and does not require a heap ABI. Private
link-state anchors `+0x68..+0x78` are full SRAM pointers used by the software
get/append routines; only reset-time event words compress the selected TX head
and RX tail. One DTM
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
unproven. Static binding therefore accepts only the three product-owned limits
and retains `5` as a private S31 source fact. Applications cannot override or
silently copy that vendor-build detail. The resulting allocation image is
installed before the CPU owner is returned.
The vendor software kind and callback pointer are not copied into the graph;
the open scheduler must replace them with typed dispatch and affine ownership.

The RX allocator requests `capacity + 0x1e`, writes the two-byte
`capacity + 2` image at packet offsets `+0x05/+0x06`, and starts its zeroed
header as `[0, packet, 0x80800000, 0, 0, 0]`. Before reuse, the append path ORs
`0x00ffffff` into packet word `+0x0c`, preserving the high byte, writes
`0xffff` at packet halfword `+0x18` and clears the header completion bit. For
capacity `0xff`, the packet bytes are therefore `1, 1`. These exact CPU-owned
transforms still do not reveal the hardware producer of the result word.
For capacity one, get marks a completed tail terminal. Append copies its full
24-byte image into the detached reserve, clears the original packet pointer,
uses the copy as the new armed tail and retains the original as a packetless
predecessor. The next drain moves that predecessor into the reserve and
advances the other header to head before the slots exchange roles again. The
open memory transaction now models this exact bounded rotation; it exposes no
raw header words and returns either no-packet or one typed result projection.

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
The restricted PAC retains positional selector names, while the new
controller-memory layer exposes only the exact scan/non-scan global routing.
Complete current `r_sym_ble_G4zC4UNjJYmyjOsZ3vNq` contains no separate
software write that latches the private DTM graph. Its only publication chain
is the common scheduler head to scheduler-item `+0x08`, then the already bound
link state and its private TX/RX links. The adjacent
`r_sym_ble_4QeP6vZAoSzLLHdFgwD0` wrapper only composes two common BT/RF wake
helpers before scheduler insertion. Thus a separate software latch is not a
publication prerequisite. Hardware current/next interpretation and the
undocumented engine meaning of the graph remain unproven. Completion-side CPU
ownership is now explicit: the graph crosses back only after fresh head
retirement, software unlink, the post-unlink return gate, descriptor recycle
and exact Timeline/list release.

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

Scheduler initialization now closes the initial-list prerequisite without
assuming anything about a vendor container. The PAC returns an affine proof
only after clearing all sixteen hardware heads and completing its trailing
device fence. The Controller consumes that proof while constructing its own
exclusive empty software-list epoch and retains the combined owner through
all later powered states. This makes the first insertion into that pristine
epoch a distinct, bounded merge case. It does not authorize insertion yet:
the exact empty-list descriptor/link writes and release visibility edge must
consume the same item and list owner before a head-published state can be
formed. The later interrupt-preparation and RUN suffix must consume that exact
head before the graph can enter its distinct running state.

The controller-memory layer now implements the item half of that first merge
as a separate cancellable typestate. It clears the submitted item's compressed
hardware-next link while preserving the allocation image and clears the
source software-next link. The Rust scheduler epoch intentionally replaces the
three vendor manager pointers instead of materializing their private ABI. At
this controller-memory boundary alone, the state performs neither a visibility
fence nor an MMIO publication.

The initialized scheduler now supplies that consuming join. It advances its
exclusive list from `Empty` to the exact prepared item identity at the same
time that the controller-memory state applies the empty-list links. A second
item is rejected unchanged, and pre-publication cancellation requires the
same identity before restoring both the list epoch and descriptor state. The
joined state still remains CPU-owned and cannot call a register accessor.

The later common insertion edge supplies the source-program order. It clears
item byte `+0x4e`, writes
`0xffff_ffff` to item status `+0x38`, links the item, publishes its compressed
head at `0x2010_b000 + 0x10 * list` and only then conditionally writes one to
`SCHEDULER_CONTROL` at `0x2010_1000` as the kick. No explicit RISC-V fence
appears in the complete DTM, memory-manager or common-scheduler bodies.
The open PAC deliberately strengthens both ownership edges with device fences:
one before and after head publication, and one after the finished-status/report
transfer. Official ESP-IDF commit `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
defines `0x2f00_0000..0x2f08_0000` as the same directly mapped internal
DRAM/DIRAM/DMA window and places external memory in a separate aperture.
Therefore this graph needs no cache clean or invalidate operation, but its
post-completion status and RX loads must still be volatile and inaccessible
until the affine fenced-transfer result is consumed.

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
publishes value-only observations through the durable handoff. The production
command actor drives admission, wait, result consumption, recurrence, Test End
and backpressured HCI publication as one operation. The remaining runtime gaps
are outside that list-zero DTM loop: unrelated-list routing, modem-timer
expiration ownership, sleep-enabled RF wake and powered teardown.

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
link-state `+0x64`, performs TX/RX accounting only for status zero, and may
reschedule either status outcome while the DTM environment remains active.

Normal vendor Test End snapshots the shared count, deletes queued kind-five
items, synchronously stops the common scheduler, clears active/count state and
only then frees TX packet/header, RX packet/header, the swap reserve, the
private scheduler chain, link state and context. Scheduler stop requests
shutdown and waits for `SCHEDULER_STATE.BUSY` to clear, but that predicate alone
does not prove an empty software completed queue, absence of an already-entered
callback or return of the sole item token.

The open async design deliberately does not reproduce that global scheduler
teardown. Its stopping runner first closes recurrence. A pre-HEAD event is
cancelled and recycled; a post-HEAD event is driven through RUN, finished-list
capture, empty-head retirement, unlink and recycle exactly once. Only the
returned affine graph may enter `TestEnded`, and response publication retains
that graph across HCI backpressure before restoring `Idle`. This is a stronger
local ownership join than vendor `BUSY=0`, while unrelated-list dispatch and a
future powered all-role shutdown remain separate outer-runtime work.

There is one intentional HCI difference. The current vendor callback increments
its shared count for a successful TX event and Test End serializes that value.
Bluetooth Core 6.3, Vol 6 Part F, requires the packet report ending a
transmitter test to contain zero
([official RFPHY Test Modes](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core_v6.3/out/en/low-energy-controller/rfphy-test-modes.html)).
The open Controller therefore reports zero for TX and retains the vendor count
only as a reviewed implementation anomaly, not as driver policy.

This is the reference software edge at which a finished DTM item is handed
back to its role. It is the correct boundary for an affine
`HardwareOwned -> Completed -> CpuOwned` open lifecycle. It still does not by
itself prove which raw interrupt/status bits produce each hardware-list bit,
but completion visibility is no longer a cache-operation unknown. Official
ESP-IDF describes `0x2f00_0000..0x2f08_0000` as directly mapped internal DMA
RAM, separate from the cached external apertures. The current PAC transfer
orders `FINISHED_LIST_STATUS` read, `FINISHED_LIST_REPORT` write and a trailing
`fence iorw, iorw`; status and RX reads must remain inaccessible until an
affine result of that transaction is consumed. The memory layer now models
scheduler items, link-state, both RX headers and the hardware-written RX packet
words as volatile shared SRAM. Its list-zero-specialized
transition consumes one affine list token, retains ownership on the sentinel
and classifies zero versus nonzero completion without granting CPU ownership.
The capacity-one per-buffer drain, swap rotation and re-arm rules are now exact.
Hardware-head retirement, abstract software-list unlink and the post-unlink
return gate are composed for every DTM completion, including exact timeline
release and reviewed descriptor-link cleanup. The RX-success transaction also
proves the full-pointer two-header topology, clears the retired scheduler links,
accounts the bound semantic result and only then commits the corresponding
append/re-arm rotation. Source-list release precedes public CPU ownership.

The later current-revision software-list removal return gate is now exact as
well. The historical same-chip body names the caller
`r_sched_txn_removeSwList`; the current body replaces its old busy-state
assertion tail with `r_sym_bt_FCfM3hAXphsk1qERleGZ`. One attempt returns only
after an idle scheduler observation, positional command-zero status 26 and
positional command-one status 18, with the two command reads short-circuited in
that order. The current caller mutates its software list and then conditionally
tail-calls this helper; neither function consumes a primary-interrupt event or
proves that either command-status transition raises source 124. The vendor
repeats this direct observation and diagnoses every 10,000 misses.
The restricted PAC and HAL instead expose one ordered split-owner finite
transaction. The Controller consumes the exact empty-head graph into the
distinct source-owned `SoftwareListUnlinked` state and arms its capacity-one
mailbox atomically under one critical section, intentionally replacing rather
than recreating the vendor intrusive list. Every public primary service uses
that same boundary for capture/acknowledgement, both ordinary durable cell
publications and mailbox routing. The armed slot retains exactly the first
post-arm disposition; pre-arm events remain on the general path, and a full
slot returns a later event without overwriting the retained one. The internal
mailbox identity is allocated globally on first arm and every arm adds a
checked generation; either exhaustion rejects before unlink. The internal
graph/event pairing has no public constructor, standalone unlink or primary
service bypass. Consuming it performs no second interrupt capture,
acknowledgement or cell publication. Ordinary scheduler and lock/modify wake
dispositions belong only to the immediate primary-service result and are not
repeated by this late consumer.

Its affine scheduler observation makes BUSY return `Pending` before the task
owner can read either command register. Only the idle token permits command-zero
and then conditional command-one reads. `NoSchedulerWork` and command-pending
outcomes re-arm the same mailbox identity and generation before leaving the
critical section; a foreign Controller mailbox cannot take, cancel or re-arm
the owner even when its numeric generation matches. Ready advances the same
identity without returning descriptor ownership. This closes
temporal pairing and owner retention, not retry liveness. A later command-ready
edge can be returned by a full mailbox while the first retained event is still
pending, then precede the re-arm and fail to become the next DTM event. Selector-6
recovery still blocks this path fail-closed, and neither vendor evidence nor the
current open runtime proves command-ready-to-source-124 causality or a
guaranteed retry wake.

The reviewed register model consequently promotes `0x2010_125c` to
`SCHEDULER_FINISHED_LIST_STATUS.FINISHED_LIST_MASK` and retains `0x2010_1260`
as the positional `SCHEDULER_FINISHED_LIST_REPORT`; it does not invent W1C
semantics for that second word. The restricted PAC preserves the exact
read/report/fence order as one affine task-owned observation. Its mask and pop
result cannot be copied, and each selected list becomes a nonforgeable affine
token instead of a freely constructible positional index. The Bluetooth layer
drains one lowest-numbered list per finite step, allowing the future async
bottom half to yield between lists without a loop or RTOS. The generic token
proves only one selection from that captured transfer. The DTM Controller
supplies the next layer for list zero and can continue any retained mask one
affine observation per call without recapturing hardware; other-role mapping
and dispatch remain outside this closure.

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

The scheduler's first live reference update is exact as well. Its initializer
zeroes both raw and scheduler reference images, and its task-enable leaf is a
no-op. The first task-run tail-calls the reference update, which samples raw
time, converts the wrapping delta from the prior raw reference, stores sampled
raw time minus the conversion remainder and advances the scheduler reference
by the converted quotient. Every accepted ESP32-S31 scale image is positive,
so this first update has zero remainder. It therefore establishes
`raw_anchor = sample` and
`scheduler_anchor = sample << (shift_image - 1)` with 32-bit wrapping; the
standalone shift image three produces `scheduler_anchor = sample << 2`.
Every later completed Rust scheduler-current observation applies the same
reference-update geometry to the retained epoch: it projects the fresh raw
sample through the prior epoch, stores that projected scheduler image beside
the fresh raw anchor and binds the same non-copyable sample to one DTM
preparation. Cancellation, Drop and orphan drain do not update the epoch. This
models the arithmetic of the vendor task-run reference update; without a live
vendor task callback it does not claim that the observation occurred at an
equivalent execution point or that its software recheck latency is bounded.

## What belongs in each open layer

| Layer | DTM responsibility | Current publication gate |
| --- | --- | --- |
| SVD / restricted PAC | Typed controller-window fields, complete publication/ack order, controller-time reads and the SRAM compression domain | Lock/modify, insertion execution-lock/modify, always-awake time-latch and software-list-removal fields, exact publications, wait predicates and finite live MMIO are present. Command-zero accepts only a typed item/list request; command-one accepts a typed list and constructs its one-hot field inside the PAC. The Controller-owned mailbox retains the exact first primary disposition after atomic unlink-and-arm; its globally unique opaque identity and checked generation prevent cross-Controller affinity, while exhaustion rejects before unlink. Its internal graph/event pair has no public constructor and consumption repeats no ISR operation or wake disposition. The stored scheduler observation conservatively drives each finite PAC task step: BUSY skips all task-side command reads, idle permits command zero, a clear ready field skips command one and only a ready command one exposes its result. This preserves split ownership but does not establish vendor source-124 causality or guarantee that a later ready edge survives while the slot is full. Terminal values reduce to pending/retained/reconcile/rejected dispositions without exposing register images. The later interrupt-time scheduler snapshot also returns the typed current hardware-list index. That snapshot is affine; its removal projection yields either Pending or a single-use idle capability required by the HAL task owner. The task owner then preserves command-zero before conditional command-one without a polling loop. At Controller composition, no-work and command-pending retain the exact unlinked awaiting owner and re-arm its identity and generation before returning; Ready binds that same owner to the complete removal predicate, so another mailbox epoch cannot be cross-wired at the memory boundary. Scheduler-head publication now orders every prior CPU descriptor write before its generated MMIO field update and retains the trailing device fence. BTMAC source 14 is modeled as one generated W1C clear field plus one interrupt-enable field; the affine run suffix consumes the published head and dynamic-interrupt proof, performs the exact synchronous clear/enable/fence transaction and admits RUN only by consuming that result. A distinct affine post-completion transaction consumes the RUN provenance into a fresh fenced hardware-head observation and advances only on empty. Per-event recycle and the lower `CpuOwned -> Reclaimed -> reinitialize` edge are present; quiescing an in-flight Test End request remains an upper session responsibility. |
| HAL | Powered controller epoch, common RF wake, cache/device fences, timer conversion, same-core IRQ routing and bounded stop/quiesce | The HAL forwards the finite insertion execution publications and observations without exposing raw PAC access. It also forwards the safe typed scheduler run-event and RUN chain; no upper layer passes an address, mask or register image. The powered Controller retains the unique repeatable live latch owner and exposes finite generation-keyed request/recheck operations with cancellation drain. The first completed observation initializes the scheduler epoch; later borrowed current observations reanchor the same epoch. At the terminal powered BLE-PHY boundary, the retained always-awake selection plus a completed later request projects one opaque RF-ready token without RF MMIO; that token is consumed in the exact role/phase order and never reanchors the epoch. The sleep-enabled wake branch, a proven wake/recheck source, physical counter contract, live-route lifecycle, autonomous completion dispatcher and powered rollback remain absent. |
| Scheduler core | Affine event item, ordered deadline queue, insert/abort/complete states, hardware-head replacement and consistency check | DTM list zero now has an exclusive empty epoch, exact first-item merge and terminal pre-route publication transition. The Controller rejects a merge from another epoch before head MMIO and retains the unchanged graph on failure. Success immediately consumes every memory rollback image against the exact affine PAC head token and records both scheduler and memory epochs as head-published. That state has identity only and cannot observe completion. The exact head is then consumed through stable-owner dynamic interrupt preparation, synchronous source-14 event publication and RUN as one typed suffix, moving both epochs to their distinct running phases. Storage rejection returns the unchanged head-published owner before suffix MMIO; success returns an affine running graph and no CPU mutation surface. Only that running graph can join a fresh finished-list transfer, read volatile status and advance a non-sentinel list-zero result without granting CPU ownership. Retained remainders use opaque bounded continuations without recapture or MMIO. After fenced empty-head retirement, atomic unlink-and-arm returns one mailbox-identity-and-generation-bound awaiting owner. Public primary service serializes capture, both ordinary publications and mailbox routing; no standalone unlink, bypass service or pair constructor remains. Wake dispositions are emitted only by that immediate service result. No-work and command-pending consumption re-arm before unlock, while Ready advances the same graph without duplicating those wakes. Temporal order and cross-mailbox affinity are closed, but vendor evidence does not connect command readiness to source 124 and a full slot can pass a later ready edge before re-arm, so retry liveness remains open. Ready TX and RX nonzero-status paths consume the exact reservation and return the exclusive list to Empty only after memory recycle succeeds. Zero-status RX enters a specialized fail-closed drain/account/re-arm transaction and returns the same non-copyable session only after memory, timeline and source-list release. Recurring TX and RX consume their active owners, derive the next phase, prepare a new affine merge and feed the same head/RUN path; every rejection returns the prior owner without committing the candidate phase. The production actor owns this list-zero session driver; unrelated hardware lists remain outside its dispatcher. |
| Packet memory | Static aligned TX/RX/link-state slots with `CPU -> prepared -> head-published -> running -> completion-observed -> CPU -> reclaimed` ownership | A no-heap memory crate owns the complete 936-byte DTM graph, validates physical placement and installs full private software-list pointers separately from compressed hardware links. TX readiness and role-specific event/bookkeeping transforms remain affine through first and recurring merges. The exact list-zero PAC publication token consumes CPU rollback authority into `HeadPublished`, which exposes identity only. The matching RUN token alone advances it to `Running`; only that state accepts a fenced finished-list observation. Scheduler items, link-state, RX headers and hardware-written RX packet words use volatile semantic accessors. The direct internal DMA-RAM aperture and trailing PAC device fence establish visibility. The lower recycle transition consumes exact empty-head/removal proofs; zero-status RX validates the deterministic two-header topology, accounts the typed result and commits append/re-arm before returning an active owner that recurrence can consume. Only an ordinary CPU owner can enter `Reclaimed`; it retains the same pinned graph and its inseparable allocation configuration, exposes no event preparation and can reinitialize a fresh CPU-owned epoch without caller-supplied configuration. No equivalent edge exists on an intermediate owner. |
| LLL DTM | Parameter validation, channel/PHY/pattern image, TX/RX event state machine and receive counter | The complete channel domain, composed frequency lookup, role-dependent PHY/rate mapping, all eight bounded TX payload patterns, packet-duration/minimum-interval/tick arithmetic, constant-time event catch-up and RX accounting transition are typed. Active TX/RX owners retain immutable command identity, committed phase and, for RX, the non-copyable accounting session across recurrence and its lossless failure paths. First and repeated scheduler-current acquisition is Controller-bound and affine. The source-owned standalone RF-ready producer and private phase states enforce initial current-before-RF, recurring-RX RF-before-current and recurring-TX current-only order. Initial admission and sequence are private generation-keyed requests separated by reservation; recurring paths reserve before their sole sequence request. The reviewed standalone margin 107 is held only by the source-owned scheduler policy. The active-session and stopping runners preserve continuity through prepared and in-flight ownership, close recurrence, finish or cancel the accepted event and retain the reclaimed graph through response backpressure. Sleep-enabled RF wake, remaining hardware field meanings, unrelated-list dispatch and powered all-role abort remain incomplete. |
| HCI | LE Receiver/Transmitter Test v1 and v2 plus LE Test End command/event semantics | A closed generic codec validates exact versioned bodies and normalizes them into one receiver token and one transmitter token. The token retains its exact ingress opcode, typed channel, PHY, pattern and receiver modulation-index assumption through deferred hardware ownership, RUN/failure, busy rejection and response backpressure. Reserved PHY selects return `Unsupported Feature or Parameter Value`; other malformed parameters fail closed as `Invalid HCI Command Parameters`. The S31 projection opens the reviewed 1M/2M/Coded rate modes without version-specific runners; both valid modulation-index assumptions share the reviewed channel/PHY-only RX context because Bluetooth permits, but does not require, a receiver optimization. The source-owned session policy retains idle starts and active Test End for the hardware owner, completes idle Test End with success and zero packets, and rejects a repeated active start with `Controller Busy`; no caller supplies those statuses. The target command actor publishes exact-once responses through the standard `bt-hci::ExternalController` transport. The pinned `bt-hci` release still lacks correct typed Host-side Receiver/Transmitter Test coverage for the board example, so no raw Host command path is added. |

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

## Implemented runtime and next executable increments

The production command actor, first-event runner, recurring-session runner and
Test End/Reset quiescence path now compose the DTM graph end to end. The list
below records both completed foundations and the remaining narrow increments;
it must not be read as claiming on-air validation.

1. **Drive ordered timing through a concrete session pump.** First and recurring preparation
   no longer accept caller-built windows, and their active owners retain the
   immutable command identity and committed phase. The private affine
   standalone always-awake marker is now retained from Controller-HAL profile
   selection through the pre-route BLE-PHY owner, but performs no RF MMIO and
   gates a terminal affine first-live time request. Its completed sample
   initializes the persistent scheduler epoch, and repeated borrowed requests
   reanchor that epoch before one role/phase preparation consumes the private
   current. The terminal powered BLE-PHY owner now composes the retained
   always-awake selection and a completed later time request into a private
   RF-ready token. Initial TX/RX consume current before that token, recurring RX
   consumes the token before fresh current, and recurring TX remains
   current-only. Initial admission and later sequence acquisition are distinct
   private requests separated by reservation; recurring preparation reserves
   before its sequence request. The concrete session/HCI pump now retains these
   owners through response backpressure. General LL role dispatch remains
   outside this DTM-only path.
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
   link. The complete event body proves there is no separate software latch;
   retain the published `item -> link state -> private links` chain and trace
   only the undocumented hardware interpretation still needed for completion.
4. **Maintain the concrete executor-neutral DTM pump and Test End.** Completion and
   recycle already return active TX/RX identity only after exact list and
   Timeline release, and a rejected recurrence preserves the committed phase.
   The lower role edge now consumes either active owner into `TestEnded`,
   retaining a reclaimed graph with zero for TX or the accumulated RX count;
   that graph can reinitialize the same pinned allocation using its bound
   configuration. Honest `Idle -> GraphReady` and
   `TestEnded -> Stopping -> Idle` bookends now retain the graph and terminal
   report across response backpressure. The concrete owner-preserving pump now
   suppresses recurrence after Test End and either cancels before publication
   or joins the accepted event through recycle. BUSY-clear alone remains
   insufficient evidence for whole-session quiescence.
5. **Complete the open scheduler wake model.** The fixed-capacity Timeline,
   explicit prepared/running/completed/recycled owners and globally
   identity-branded atomic unlink-and-arm mailbox now exist. The exact first
   post-arm event is retained
   without a public constructor or service bypass, and no-work/command-pending
   consumption re-arms the same identity and generation before returning. ISR
   wake dispositions are delivered once by primary service, not repeated by
   late consumption. Add a proven command-ready wake and a
   bounded policy that cannot miss a later ready edge while that capacity-one
   slot holds an earlier event. The vendor selector-6 callback validates only
   its private intrusive transaction container and has no open-driver runtime
   equivalent; keep scan resume outside the DTM feature graph.
6. **Preserve the composed ISR epoch and session owner.** The level-3 hard handler
   captures/acknowledges a bounded snapshot and publishes a lost-wake-safe
   token. The executor-neutral session consumes those finite events and drives
   recurrence or stopping; neither path blocks, allocates or calls an RTOS.
7. **Validate DTM without target HIL first.** Host/model tests now cover typed TX
   and RX requests, recurrence, RX count, deferred in-flight stop and idle graph
   reuse. Add target HIL channel/frequency and payload-pattern checks only when
   exclusive ESP32-S31 hardware becomes available.
8. **Keep the Controller-side HCI surface DTM-only.** The generic layer parses
   Receiver/Transmitter Test v1 and v2 plus Test End into normalized typed
   commands and builds their staged Command Complete events. The same concrete
   session worker consumes both command versions; no version-specific runner or
   compatibility alias exists. Publication
   retains the typed command/result through backpressure. The pinned `bt-hci`
   release does not provide correct typed Host-side Receiver/Transmitter Test
   commands, so the target example intentionally does not invent a raw Host
   command path. Capability bits and supported-command images remain
   conservative.

Only after this slice passes deterministic virtual-time tests, compiled
production trace comparison, fault/cancellation tests and dated HIL should the
driver report DTM as live. Legacy non-connectable advertising is the next
vertical slice; scanning follows it and introduces the typed successor to the
now-classified scan-resume path.

## Remaining hard unknowns

- scheduler-head diagnostic result-code meanings, if they are operationally
  relevant at all;
- raw interrupt/status origin of the finished hardware-list mask and its
  acknowledge/re-arm order; the device fence and DTM list-zero affine join are
  already explicit;
- exact multi-item ordering within a finished hardware list; DTM's capacity-one
  list-zero relation does not establish that rule for other roles;
- exact controller-time raw width, wrap and physical unit;
- complete S31 descriptor field boundaries, all hardware-read words and the
  hardware interpretation of DTM's published private link-state RX pointer;
- exact hardware current/next rotation for the normal scan/non-scan global
  lists; DTM deliberately bypasses those selector pairs;
- exact primary/NRT bits for TX done, RX done, timeout and abort in the DTM
  feature configuration, including their mapping to finished-list indices;
- whether any command-ready transition in the post-unlink return predicate
  raises source 124 or another hardware wake at all;
- meaning of the validated RX result high byte, plus length/CRC/RSSI extraction;
- bounded abort plus powered quiescence when an event is scheduled or running.

The operational blockers are the physical controller-time width/unit contract,
guaranteed post-unlink progress when the mailbox is full, unrelated finished-list
routing, source-127 expiration ownership, remaining hardware-consumed descriptor
semantics, sleep-enabled RF wake and target on-air evidence. The DTM session loop,
in-flight Test End join and backpressured Controller-side HCI publication exist.
Trouble integration and an RTOS abstraction are not prerequisites.
