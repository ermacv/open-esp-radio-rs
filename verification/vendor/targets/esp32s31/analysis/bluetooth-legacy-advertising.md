# ESP32-S31 legacy advertising admission boundary

This review defines the smallest hardware contract needed after the portable
`ADV_NONCONN_IND` encoder. It does not reproduce the vendor advertising
driver. The first open role is deliberately restricted to LE 1M, standalone
always-awake operation, one non-connectable PDU on primary channels 37, 38 and
39, no receive window, no scan response and no coexistence policy.

## Evidence and role map

Current behavior comes only from the linked ESP32-S31 controller built from
[`esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31).
The public named archive at
[`esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd)
is used only to recover roles. Stable prologues, object offsets, call order and
callee relationships establish these correspondences; old bodies do not
supply current behavior or layout.

| Current linked body | Reviewed role |
| --- | --- |
| `r_sym_ble_MrW1ZaJZqsHzi3wwRhsn` | build a legacy primary-channel PDU |
| `r_sym_ble_c9Zmr2aWmPOsZoITHDe9` | checked legacy-PDU builder wrapper |
| `r_sym_ble_fxKAT8in6cXLv0gLB2W5` | reset the advertising link-state graph |
| `r_sym_ble_fiiv8hEPVagnOAVa7EM2` | reset an advertising role |
| `r_sym_ble_cfILxLFIWftw22I0zQab` | allocate the private advertising graph |
| `r_sym_ble_8UzoZYkzYu9MXbM1vyWN` | start an advertising role |
| `r_sym_ble_grUssKu7oWAkmdueH0Od` | initialize the first-event delay |
| `r_sym_ble_mqh4OXzoN59kvnkKFMA1` | schedule its first primary event |
| `r_sym_ble_GESyjhFJ89FTdFkUqASV` | form one advertising scheduler window |
| `r_sym_ble_77zgK6v8rbStzf0ReBjv` | schedule its next primary event |
| `r_sym_ble_pfcv0QVYrNS6KoQetydw` | recycle a completed scheduler item |

The shared finished-list ingress and scheduler-list consumer are already
reviewed in `bluetooth-interrupt-runtime.md`. The focused Blobray scope
`ble-legacy-nonconnectable-admission` connects the exact packet, link-state and
first-event producers plus recycle to those two asynchronous roots. The wider
`ble-legacy-nonconnectable-advertising` scope retains broad role allocation,
reset, start and recurrence research.

## Existing open implementation

The portable Link Layer validates address kind, advertising data, non-empty
primary channel maps and interval bounds. It encodes an exact bounded
`ADV_NONCONN_IND` and retains generation, event and the complete ordered
channel plan through an affine lifecycle. The selected channels form one
backend event rather than separate executor-driven submissions. The S31
pre-admission owner installs that PDU in the common typed controller TX
allocation used by DTM and advertising. That allocation lives inside a pinned,
physically bounded graph. The graph
binds the common TX header to its packet, installs that header as the sole TX
head/tail, keeps the RX chain absent, retains one separately allocated common
scheduler context, binds up to three scheduler items back to this link state
and installs the first item as the link-state scheduler head. The semantic
channel plan stays at the portable boundary; only the private memory codec
lowers it to S31 frequency fields and private compressed successor links. The
owner supports lossless cancellation of both portable and SRAM owners. The
complete first-event path now joins the descriptor graph
to common scheduler bookkeeping, an independently proven empty list, typed
`HEAD` publication, dynamic interrupt publication, the synchronous scheduler
event and `RUN`. Publication is the sole transition that moves the portable
owner to `InFlight`.

The public named archive proves that legacy primary allocation calls
`r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr`; its allocation prefix and PDU
placement are therefore shared with DTM. The DTM graph typestate itself is not
reused: advertising still has a different private link-state/scheduler graph
and recurrence policy.

The same archive's `r_ble_lll_adv_alloc_sch_items` establishes the separate
common scheduler-context link, the scheduler-item-to-link-state link and the
terminal first-item chain. `r_ble_lll_adv_start` independently stores that
item as link-state scheduler head before calling
`r_ble_lll_adv_sched_first_pri_event`. Current stripped allocation, start and
first-event bodies retain those offsets and ordering. These allocation-time
links are now one private memory-codec operation; no compressed image or SRAM
field escapes to the controller or Link Layer.

Complete current `r_sym_ble_fxKAT8in6cXLv0gLB2W5` and named
`r_ble_lll_adv_reset_link_state` establish the next transition. The private
memory codec now applies their restricted LE 1M, no-RX, no-CTE and no-privacy
projection from the prepared PDU and a signed dBm request. This includes the
terminal TX-header link, absent RX link, shared rounded-power conversion,
primary-advertising Access Address and CRC preset, public/direct-random address
branches and the reviewed standalone option byte. The option value is not a
guessed bit meaning: same-chip `priv_config_opts_ro` is 46 bytes and its exact
byte at `+0x29` is `3`, matching the current body's six-bit copy. Reset creates
a separate non-publishable typestate; cancellation clears the packet and
rebuilds the allocation graph before returning it to the portable owner.

Complete current `r_sym_ble_mqh4OXzoN59kvnkKFMA1` and named
`r_ble_lll_adv_sched_first_pri_event` close the first-event producer. Named
`r_ble_lll_adv_init`, matching current `r_sym_ble_grUssKu7oWAkmdueH0Od`,
initializes a 2000-microsecond first-event delay. The first event samples the
always-awake radio path and scheduler time, adds the common 107-unit
preparation lead, and forms the LE 1M duration as `payload_length * 8 + 80`.
If the radio observation is later than the nominal start, it shifts start and
end together and preserves duration. Both positions then pass through the
retained scheduler epoch into raw controller time.

Complete current `r_sym_ble_77zgK6v8rbStzf0ReBjv`, named
`r_ble_lll_adv_sched_next_pri_event` and
`r_ble_lll_adv_sched_remaining_pri_after` establish that selected primary
channels are not resubmitted after an IRQ. Before the first `RUN`, the producer
allocates every remaining selected channel, advances each follower start from
the previous item end, adds the same item duration, and links items through the
hardware successor field. The completed-queue link at item `+0x54` is a
different software ownership link, not the execution chain.

The private SRAM codec therefore detaches the allocation-time scheduler head,
lowers the canonical selected channels 37, 38, 39 into one 1--3 item chain,
selects the legacy LE 1M transmitter role, copies the reset link state's
rounded power, installs contiguous accepted raw windows and clears bookkeeping
for every active item. No link image, field mask, rounded-power image or
frequency integer crosses into the controller/LL layer. Before this mutation,
the common Rust timeline reserves the complete chain duration, performs guarded
initial admission and duration-preserving overlap displacement, then applies a
separate fresh sequence-deadline check. Rejection releases the exact slot and
returns the unchanged affine candidate.

## Reviewed response-capable memory profile

The response-capable extension is derived from the same pinned current
`libble_app.a` at commit `7f20740dd66ee774ffce5db0b55507892551aa31`,
SHA-256
`62dbe7216619f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`.
The earlier same-chip archive at commit
`31c30949541a5d3abd4043a1cb66d55aa55577dd`, SHA-256
`ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`,
supplies names only. Complete object-body comparison maps current member
`50.o:r_sym_ble_fxKAT8in6cXLv0gLB2W5` to
`r_ble_lll_adv_reset_link_state`,
`50.o:r_sym_ble_6qbuM1ANzD8Y0VbSNKNu` to
`r_ble_lll_adv_pri_chan_txbuf_alloc_and_make`, and
`50.o:r_sym_ble_nNcQpcbAvd2WhGX0WkCA` to
`r_ble_lll_adv_alloc_rxbuf`. The compared bodies are respectively 840, 442
and 124 bytes and are byte-identical between those revisions. Current
member `15.o:r_sym_ble_GESyjhFJ89FTdFkUqASV` is likewise the byte-identical
180-byte `r_ble_ll_adv_set_sched` body. Current bytes, rather than the old
names, own every layout and arithmetic fact below.

An `ADV_IND` event is a two-transmit-node graph, not the nonconnectable graph
with an optional receive flag. The primary builder always allocates the first
header and packet. Its response-capable branch allocates a second header and
packet, encodes the second PDU as `SCAN_RSP`, links the primary header to that
successor and installs the second header as TX tail. An empty scan-response
data body still retains the second node. The reset body does not branch on
response capability: it copies the graph RX head into the compressed RX
consumer link and retains the full RX head/tail beside the two-node TX
head/tail. Consequently a response-capable readiness predicate must validate
both RX representations; checking only the full RX endpoints accepts an
incomplete hardware graph.

The scheduler-window body adds the halfword at private configuration-table
offset `+0x2c`, whose current and named tables both contain `4`, to the LE 1M
primary-PDU duration when response capability is present. This is retained as
an opaque four-microsecond response-capable scheduler tail reserve. The
evidence does not identify it as an inter-frame space, receive window or scan
response airtime. The receive allocator prepares the non-scanning receive
class and supplies the selected primary-channel count to vendor memory
management, but it does not prove that the open fixed two-node pool safely
covers a three-channel response event. The first open memory typestate
therefore accepts exactly one primary channel and fails closed otherwise.

The portable Link Layer owns the semantic advertisement, scan-response data
and typed bounded `ADV_IND`/`SCAN_RSP` encodings. A later chip aggregate must
retain that affine portable event while translating it into a short-lived S31
memory input: two allocation-fit PDU borrows, the already selected own-address
behavior and exactly one translated primary channel. The memory crate has no
portable Link Layer dependency, does not retain that event and does not parse
or revalidate Bluetooth wire semantics. It owns only the private two-header TX
chain, the exact affine non-scanning RX pool, the common advertising reset
projection, the compressed RX consumer link and the opaque scheduler duration.
The joined memory owner remains non-publishable and losslessly returns only the
graph and RX pool on cancellation or rejection. Scheduler admission, RX
dispatch, accepted `CONNECT_IND` transfer and multi-channel buffering are
separate later boundaries rather than implied capabilities of this memory
profile.

## Minimum production admission contract

One advertising transmission may enter production only when current-artifact
evidence closes the following connected edges:

1. **Packet producer:** one owned packet/header image contains the encoded PDU,
   declared length, advertising access address `0x8e89bed6`, CRC initialization
   `0x555555`, selected primary-channel frequency and matching whitening seed.
2. **Graph binding:** the link-state and scheduler item point to exactly that
   packet storage, and every hardware-followed pointer has a bounded typed
   encoding, alignment rule and lifetime.
3. **Publication:** a single affine operation orders all SRAM writes before
   publishing one common-scheduler list head and issuing `RUN`. CPU mutation is
   impossible while the owner is hardware-visible.
4. **Terminal observation:** the interrupt/finished-list path identifies the
   same item, fences hardware writes, unlinks it from both hardware and
   software lists and returns CPU ownership exactly once.
5. **Result and recurrence:** every active item must have a non-sentinel
   completion before the exact scheduled event advances its portable identity
   once. Per-item values remain diagnostic rather than a claim of successful
   on-air transmission. The first slice needs no RX or scan-response handling.

Unknown private fields do not block the first driver merely because they lack
vendor names. They do block admission when their value participates in packet
selection, timing, ownership, launch or completion. A reviewed whole producer
image with controlled inputs is sufficient; guessing individual bit names is
not required.

## Current blockers and non-blockers

The encoded PDU now completes one full hardware event without raw SRAM fields
escaping the memory codec. Allocation, restricted reset, first-event timing,
timeline admission, descriptor transform, empty-list merge, typed
`HEAD`/interrupt/event/`RUN` publication, finished-list observation, fresh
head-empty proof, software unlink, post-unlink interrupt join and CPU recycle
are closed. The exact `InFlight` portable owner advances only after that final
recycle. A selected 1--3 channel plan is published as one prelinked hardware
chain, so executor latency cannot open a gap between primary channels.

Current and named recycle bodies both load scheduler item `+0x38`. The all-ones
value remains the in-flight sentinel. For every other value, both zero and
nonzero paths consume the current primary-channel item: the nonzero branch
adds diagnostic/exception handling, while the channel/event counters and later
recurrence path still advance. Consequently `status == 0` is not modeled as
the only protocol success and `status != 0` is not a retryable pre-publication
failure. Rust waits until all active items are non-sentinel and retains a
bounded ordered set of `Zero | NonZero(NonZeroU32)` diagnostic values next to
the advanced LL owner. It makes no stronger RF-success claim.

The first complete primary-channel event contract is therefore closed.
Recycle retains its nominal phase, reusable CPU-owned graph and ordered
diagnostics, and accepts a fresh source-owned 0--10 ms delay into the next
portable event without executor waiting. The S31 recurrence path rebuilds the
same restricted graph, projects phase plus interval/delay into the whole next
chain, takes an exact non-displaced recurring reservation, applies a fresh
sequence deadline and rejoins the existing publication/completion path. The
next driver boundary is production actor composition: provide the live timing
observation and random delay, then retain the returned affine owner across its
async steps. Broad register discovery, diagnostic logging, extended/periodic
advertising, scan responses, RX buffers, sleep wake and coexistence remain
outside this restricted nonconnectable slice.

```console
tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  project research next \
  --scope ble-legacy-nonconnectable-admission \
  --focus all --strategy frontier --limit 30

tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect function ble-controller:r_sym_ble_77zgK6v8rbStzf0ReBjv \
  --full --details

tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect function ble-controller:r_sym_ble_pfcv0QVYrNS6KoQetydw \
  --full --details
```

The first-event gate is closed: the Rust-owned static graph implements all
five edges without exposing raw SRAM images above the private memory codec.
Implementation now returns to the controller/LL recurrence path. Further
vendor research is needed only when recurrence reaches an unresolved hardware
timing or ownership boundary; full vendor-scope completion is neither required
nor desirable.
