# ESP32-S31 Wi-Fi Rust ownership migration

This document records the method and the evidence used while replacing hidden
vendor Wi-Fi state with explicit Rust ownership. It is intentionally separate
from the generated [linked-state audit](esp32s31-linked-state-audit.md): the
audit answers what is linked and reachable, while this document answers why a
boundary exists and how it may be removed.

## Target architecture

The final dependency direction is:

```text
WPA2/WPA3, association, scan policy
                 |
       async MAC/radio runtime
                 |
        ESP32-S31 radio HAL
                 |
          PAC/MMIO registers
```

The HAL boundary must expose finite register operations. It must not own
credentials, timers, queues, cryptography, allocation, or an executor. Unknown
register fields remain typed opaque values with an evidence comment until
their meaning is demonstrated. An SVD/PAC description is useful after stable
register groups have been recovered; it is not a prerequisite for moving
state out of the blob.

## Completion contract

The primary goal is ownership, not merely link-size reduction. The migration
is complete only when all radio state controlled by this driver is reachable
through an explicit Rust composition root and no C or ROM function can retain
or mutate that state implicitly.

The final profile therefore requires all of the following:

1. No mutable radio backing, node, queue, timer, rate-control, key, channel,
   PHY, MAC, or power-management object is owned by a vendor archive, a ROM ABI
   pointer cell, or an untyped C global.
2. Protocol and runtime code do not obtain Rust-owned state through an
   `unsafe` C call. Safe Rust owns the state machine and its storage; the
   target adapter may use a narrowly scoped `unsafe` block only to validate an
   interrupt payload, cross a temporary ABI, or access MMIO.
3. No vendor archive contributes executable radio functionality to the final
   ELF. Removing the last archive reference is a consequence of transferring
   ownership, not a substitute for it.
4. A temporary ROM call is admitted only after evidence classifies its whole
   reachable behavior as either a pure input-to-output transform or a finite
   register operation. A ROM function which dereferences caller state,
   consults a callback table, follows a pointer from a ROM ABI cell, or has
   unknown side effects is ownership debt and must be rewritten.
5. Pure and register-only ROM leaves are temporary differential oracles. They
   remain on the removal ledger and ultimately move to safe Rust or the
   ESP32-S31 radio HAL/PAC.
6. Interrupts do not own protocol state. They acknowledge hardware and
   transfer bounded data or readiness into Rust-owned channels; the single
   radio owner performs all state transitions.

Consequently, `0 mutable blob globals reachable from strict roots` is a useful
regression gate but not a completion claim. The separate completion counters
are: remaining vendor runtime roots, cold vendor initializers, ROM ABI
bindings, unproven/stateful ROM calls, and temporary pure/MMIO ROM calls. All
five must eventually reach zero.

## Migration rules

Each migrated object follows one vertical slice:

1. Identify every archive and ROM reader/writer, callback, timer, and pointer
   cell.
2. Recover only the layout and transitions needed by the current AP/STA
   profile.
3. Put the state machine in a safe, host-testable Rust module.
4. Keep raw pointers, MMIO, and ABI callbacks in one target-specific adapter.
5. Copy cold state once, before strict handoff, and reject an in-flight
   operation rather than transferring ambiguous ownership.
6. Remove runtime vendor getters/setters from the strict root graph.
7. Prove the final ELF with the no-wait/no-heap audit and update the linked
   state report.
8. Remove the vendor backing only after its cold initializer and deinitializer
   have also moved to Rust.

The single radio owner serializes state-machine transitions. Interrupts only
publish edges or data into bounded static channels. Cross-context scalar
snapshots use atomics; they do not disable interrupts or enter a vendor
critical section.

`adapter::RadioResources` is now the first explicit composition root. Its
address remains static because the retained C ABI callbacks have no context
parameter, but static placement is no longer treated as ownership. A one-way
`RadioOwnerClaim` permits exactly one `RadioFuture` or `WifiRuntimeFuture` to
consume the queues and timers. Dropping that future does not release the
claim: reuse will be added only with a complete async stop transition that
proves interrupts, descriptors, timers, and cold publications quiescent.

## Deferred atomic retry debt

A diagnostic scan of the qualified stress ELF found compiler-generated
`lr.w`/`sc.w` retry backedges in generic Rust atomics outside the externally
reachable command-claim boundary. ESP32-S31 has the RISC-V `A` extension but
not Zacas, so LLVM implements compare-exchange with an LR/SC retry loop even
for weak operations.

This is recorded as architectural debt, not expanded into hand-written
assembly now. The command and radio-queue admission leaves that must return
after one failed claim already use isolated single-attempt LR/SC adapters.
The remaining sites will be re-audited after each ownership slice: removing
shared global state and assigning one radio owner should make most atomics
unnecessary. Any site still reachable from an interrupt or run-to-completion
boundary will then be replaced locally, with its ownership contract known.

## Completed slice: channel manager

### Evidence

The reference is the pinned `libnet80211.a[wl_chm.o]` object. Its disassembly
establishes:

- `g_chm` is a ROM-ABI pointer cell whose backing is the 592-byte `gChmCxt`;
- home and current selectors are at offsets `0x50` and `0x52`;
- the 2.4 GHz table starts at `0x54` and contains 14 records of 12 bytes;
- record byte zero is the primary channel and the little-endian frequency is
  at byte two;
- the two vendor timer objects start at offsets `0x24` and `0x38`;
- the operation envelope contains channel, dwell durations, context, start
  callback, and end callback;
- `chm_init` produces channels 1 through 13 at 2412 MHz plus 5 MHz per channel,
  channel 14 at 2484 MHz, and the observed opaque record word `0x83`.

Fields without a demonstrated meaning remain opaque bytes in
`channel_state.rs`. This preserves evidence without turning guesses into an
API.

### Rust ownership

`ChannelState` owns the validated channel table. Home/current selectors use
atomic publication, so WPA2 AP checks and diagnostic snapshots do not need a
critical section. `ChannelResources` owns:

- the channel state;
- the channel-switch operation state machine;
- the start/end callback envelope;
- two stable-address `RawOsiTimer` objects registered in the fixed Rust timer
  pool.

`prepare_strict_runtime_before_handoff` performs the only `g_chm` read. It
requires an idle vendor operation, validates the selectors and all 14 records,
then publishes the Rust state. After handoff:

- scan dwell and MAC-settle delays are Rust async timer events;
- `chm_get_chan_info`, `chm_get_home_channel`, and
  `chm_is_at_home_channel` are not strict runtime dependencies;
- returning home and promoting the associated channel are Rust state
  transitions;
- no channel-state transition allocates, polls, waits, or masks interrupts.

The current Rust section is 256 bytes. The 592-byte vendor backing remains
linked for cold initialization, so this step temporarily adds 256 bytes. Once
`chm_init/chm_deinit` are replaced and the ROM pointer binding is unnecessary,
removing `gChmCxt` yields a net 336-byte SRAM reduction.

### Verification

The primary `wifi-primary` STA final link (implemented by
`wifi-rust-static-cold-init-hil`) passes with zero no-wait/no-heap audit
violations. In the generated linked-state report,
`gChmCxt` and `g_chm` are absent from mutable state reachable by strict vendor
leaves. They remain listed only as cold-init linked state.

The same image was verified on ESP32-S31 hardware against a WPA2 AP after the
Rust channel-state handoff. Passive scan, open authentication, association,
the WPA2 four-way handshake, DHCP, gateway ping, DNS, TCP, and post-link data
all completed. The allocation counters remained zero through association and
post-link operation.

## Current qualified baseline

The 2026-07-26 primary ELF retains one explicit RX fallback rooted at
`wDev_ProcessRxSucData`. After fixing the linked-state auditor to preserve the
current function across local `.L*` labels and to recognize absolute ROM code
aliases, its complete direct archive frontier contains 11 definitions /
3,236 bytes. The direct ROM frontier contains `memcmp`, `memcpy`, and
`roundup2`, 302 bytes in the pinned ROM ELF.

That fallback reaches four mutable blob objects: `TxRxCxt` (1,044 bytes),
`wDevCtrl` (72), `g_wifi_menuconfig` (104), and `g_lmac_cnt` (192), for
1,412 bytes total. It also reaches three four-byte ROM-ABI cells:
`wifi_sta_rx_probe_req`, `g_osi_funcs_p`, and `pTxRx`. Earlier entries in this
chronological document which reported exact zero runtime blob state were
generated by the incomplete relocation parser and are superseded by this
corrected inventory. This did not change the qualified executable; it changed
which already-linked dependencies the auditor can see.

The complete direct archive graph rooted at `register_chipv7_phy` contains 80
definitions / 21,596 bytes. Its direct external frontier resolves 119
functions / 22,816 bytes against the pinned ROM ELF; only
`__esp_radio_printf` and `rtc_clk_xtal_freq_get` are unresolved by that ELF.
Calls internal to those ROM bodies are a separate transitive graph and are not
yet counted. The cold graph reaches one mutable blob-owned object,
`phy_param` (508 bytes). `g_phyFuns` is a final-link alias to a four-byte
Rust-owned binding and is audited separately.

Outside the strict and cold-PHY graphs, the final ELF contains 167 linked
mutable blob symbols / 19,864 bytes. The generated linked-state audit contains
the full name, size, owner, and referrer inventory; “outside” means not proved
reachable by these roots, not safe to delete.

Rust-owned strict sections total 287,288 bytes. The largest storage remains in
the RX path: the 59,008-byte runtime ESF pool, 56,320-byte cold ESF pool, and
54,784-byte WDEV payload pool. These are not assumed redundant merely because
their capacities are similar; their simultaneous lifetimes and transfer of
descriptor ownership must be proved before storage is overlaid or removed.

The former state auditor used the corrected numbers as improvement-friendly
upper bounds. It rejected growth in runtime blob objects, ROM-ABI cells,
strict static storage, the vendor call graph, cold PHY state, or other linked
mutable blob state; every ownership transfer could reduce those bounds. The
generated report remains in this repository, while the analyzer source is
preserved by Git history.

## Focused radio-only porting set

The complete source graph is intentionally not the porting backlog. The
primary profile performs a full cold calibration, has no NVS dependency, and
does not retain vendor diagnostic output. Stopping traversal before
`phy_printf`, `syslog`, `phy_get_rf_cal_version`,
`phy_rfcal_data_check_new`, `phy_rf_cal_data_backup_new`, and
`phy_rf_cal_data_recovery_new` removes 15 archive definitions / 4,974 bytes
and 17 direct ROM definitions / 11,238 bytes. The removed archive closure is
the printf formatter plus calibration-record check/copy helpers. The removed
ROM closure is floating-point formatting plus `phy_byte_to_word` and
`phy_set_mac_data`. There is no NVS function in the resulting graph.

The resulting raw Wi-Fi full-calibration graph has 65 archive definitions /
16,622 bytes and 102 direct ROM definitions / 11,578 bytes. These are still
source-oracle counts, not “functions left to rewrite”:

- `memcpy`, `memset`, and `__divdi3` become ordinary Rust/core operations;
- `ets_delay_us` becomes a Rust async timer edge, never a copied delay body;
- a hardware status poll remains only when no completion interrupt can be
  evidenced; every read is a one-shot MMIO completion and the Rust owner
  supplies a finite attempt count or deadline, so no CPU spin loop is copied;
- `rtc_clk_xtal_freq_get` is an explicit clock input supplied by the HAL;
- `phy_get_romfuncs`, `phy_param_addr`, and archive
  `phy_get_romfunc_addr` are ABI plumbing to delete;
- `phy_i2c_enter_critical` and `phy_i2c_exit_critical` disappear under the
  unique radio owner;
- many remaining radio leaves already have Rust transitions or finite MMIO
  implementations and therefore are retained only as differential oracles.

For the active runtime, none of the 11 fallback archive bodies should be
ported wholesale. Completing the missing RX descriptor/control/optional
metadata cases in the existing Rust dispatcher removes the whole fallback
frontier and simultaneously releases `TxRxCxt`, `wDevCtrl`,
`g_wifi_menuconfig`, `g_lmac_cnt`, `wifi_sta_rx_probe_req`, `g_osi_funcs_p`,
and `pTxRx` from the runtime ownership graph.

Within cold PHY, the immediate unresolved `phy_bb_init`/channel frontier is
nine unique child roots with 2,926 bytes of direct reference bodies:

| child root | reference bytes | source | current decision |
|---|---:|---|---|
| `phy_txdc_cal_init` | 272 | archive | port calibration transition |
| `phy_tx_cap_init` | 230 | archive | port calibration transition |
| `phy_tx_pwctrl_init` | 154 | archive | port calibration transition |
| `phy_txdc_cal_pwdet_init` | 520 | archive | port calibration transition |
| `phy_txiq_cal_init` | 332 | archive | port calibration transition |
| `phy_bt_tx_gain_init` | 90 | archive | retain as conditional shared/coex evidence until omission is proved |
| `phy_rxiq_cal_init` | 408 | archive | port calibration transition |
| `phy_set_rx_gain_table` | 650 | archive | port RX gain transition |
| `phy_chip_set_chan` | 270 | archive | port cold channel transition |

For the current Wi-Fi-only AP/STA target, eight roots / 2,836 direct reference
bytes are mandatory. The remaining 90-byte `phy_bt_tx_gain_init` root is not
on the immediate Wi-Fi implementation path; it is retained only as
BT/coexistence evidence until a later coex profile proves whether the shared
register programming is required.

There is no vendor global to reproduce as a global in this cold porting set.
The only blob-owned mutable source object reached by the cold graph is the
508-byte `phy_param`; its required fields move into `PhyColdState` and typed
child inputs/outcomes. The four-byte `g_phyFuns` binding, ROM parameter pointer
and critical-section callbacks are compatibility plumbing to delete when the
parent is activated, not state to port. Likewise, the active RX fallback's
`TxRxCxt` (1,044 bytes), `wDevCtrl` (72), `g_wifi_menuconfig` (104) and
`g_lmac_cnt` (192), plus three ROM-ABI pointer cells, disappear when the
remaining Rust RX cases are complete; their vendor layouts are not port
targets.

`phy_set_pbus_mem` is no longer in that code backlog. Its complete 384-byte
ROM parent, 362-byte `phy_write_pbus_mem` child and 50-byte
`phy_save_pbus_reg` child are represented by one Rust-owned transition. The
ROM stack construction and four `memcpy` calls became twelve constant tables;
the only varying inputs are explicit former parameter bytes `0x002` and
`0x014`. Sixty separately completed publications reproduce the exact table,
control-field and address sequence. Six fixed final MMIO reads are returned
to `PhyColdState`, which alone commits them to former parameter offsets
`0x030..=0x047`. The ROM global pointer cell is not part of the port.

`phy_tsens_temp_read` is no longer in that code backlog either. Its 50-byte
indirect-call wrapper is deleted rather than copied. Rust owns the required
94-byte local measurement graph as three explicit operations: one PHY-I2C DAC
read, one `0x2081_8000` code sample, and a conditional PHY-I2C range write.
The complete five-entry 30-byte sensor attribute table and ROM integer
conversion are constant Rust data/code. The result and sensor index are
committed only by `PhyColdState`; `g_phyFuns` and the global `phy_param`
pointer are absent. The ROM default for an unknown DAC reads beyond the
attribute object, so Rust turns that corrupt state into a typed failure.

`phy_dcode_cal_init` is complete as a Rust-owned nested calibration
transition. The recovered four-byte ROM table is the explicit sequence
`[115, 116, 117, 118]`. Each entry runs the existing async RFPLL transition,
one finite NRX register transform, four identity-bound CKGEN PHY-I2C writes
and two six-bit PHY-I2C reads. The eight results are returned as an owned
value and committed only to `PhyColdState[0x1a1..=0x1a8]`; no ROM parameter
pointer is published.

`phy_check_rx_sat` is also no longer in that code backlog: its Rust transition,
one-shot target MMIO sampler and owned `phy_param` mutation are complete. No
dedicated completion interrupt is evidenced, so the exact 100-register-read
policy remains as 100 independently completed samples. The executor may yield
or use an async timer between samples; the MMIO leaf cannot spin. After the 10
roots, the work is to compose `phy_bb_init` (362 bytes of reference parent),
port the remaining outer `register_chipv7_phy` sequencing (486 bytes), and
activate the complete graph without publishing `phy_param` or `g_phyFuns`.

## Completed slice: typed large-RX ownership

The kind-7 ESF receive path now has an explicit ownership state independent of
its C ABI pointer:

```text
Free -> Radio -> Network -> Free
```

The safe `rx_ownership` module encodes these states in two native-word bitmaps
and host-tests every transition. `esf.rs` remains the target adapter: it
validates the exact fixed-pool object and packet range, then creates one
`OwnedLargeRxNetworkFrame`. A duplicate or stale callback cannot create a
second safe token. The network channel stores that token rather than three raw
pointer/length fields, and safe immutable/mutable packet views are available
only through the token. Its destructor is the sole Network-to-Free transition.
The generic radio recycler accepts only Radio-owned objects, so it cannot free
storage still held by the network executor.

The ISR still publishes only the intrusive lower-MAC packet pointer into the
bounded RX queue; it never sees the network ownership token. This preserves the
minimal ISR view while moving the cross-context lifetime into safe Rust. The
new second ownership bitmap costs one native word in the default profile. A
redundant cumulative data-RX claim counter was removed and is now derived from
the mutually exclusive admission outcomes, keeping the primary static SRAM
budget from growing. The qualified final ELF is 244 bytes below the preceding
311,745-byte baseline.

This slice does not claim that the WDEV payload, cold ESF, and runtime ESF pools
have disjoint lifetimes. It makes that lifetime measurable and enforceable
before any overlay is attempted.

## Completed slice: unique executor-side RX authority

The intrusive lower-MAC FIFO remains global because its address is part of the
interrupt ABI, but global placement no longer grants consumer authority.
`RadioOwnerClaim::try_take_executor` now creates one non-`Copy`, non-`Clone`
`RxExecutorCapability` only after its one-way compare-exchange succeeds.
The zero-sized capability is moved into the sole runtime
`VendorPpDispatcher`; both the event-17 arm and the synthetic continuation
must mutably borrow it before they can dequeue or recycle a descriptor.

The cold initialization dispatcher deliberately has no RX capability. If an
RX event appears before handoff, it fails immediately with
`RxExecutorUnavailable` instead of silently creating a second consumer or
entering the vendor RX pump. The ISR has no reference to the capability: it
can append a descriptor and publish a wake edge, but cannot process protocol
state or recycle storage.

`pending_continuation` remains a read-only readiness view used by
`RadioFuture`. It cannot remove a descriptor and therefore does not constitute
a second consumer. This change adds no static storage, allocation, wait,
delay, retry loop, or RTOS primitive; the capability has a host-tested size of
zero.

## Completed slice: RX discard ownership transition

The pinned `libpp.a[wdev.o]::wDev_DiscardFrame` reference body is exactly
0x20 bytes. It contains no protocol work or hardware wait: it retains
`wDevCtrl.head`, reads `tail.next`, clears `tail.next`, publishes that next
descriptor as the new software head, and tail-calls
`wDev_AppendRxBlocks(old_head, tail, count)`.

Strict Rust now performs that state transform under one finite local Wi-Fi
interrupt mask. Detaching the prefix creates a non-`Copy`
`DetachedRxPrefix`; consuming that token is the only path into the already
qualified fixed descriptor recycler. This makes the ownership transition
explicit without adding a queue, allocation, polling loop, delay, task
handoff, or static storage.

`wDev_DiscardFrame` is an absolute ESP32-S31 ROM export at `0x2f8010c8`.
GNU `--wrap` cannot interpose it because LLD also rewrites the ROM linker
assignment and captures the generated wrapper name. The late
`esp32s31-rom-wrap-overrides.x` fragment therefore retains the address only
as `__real_wDev_DiscardFrame`, aliases the public name to the unique SRAM
symbol `wifi_strict_wdev_discard_frame`, and asserts that equality at final
link. The strict auditor additionally rejects any call to the old public
leaf.

Hardware qualification completed taskless cold init, passive scan, WPA2
association and four-way handshake, DHCP and the post-link network checks.
The stress phase completed 4,096/4,096 UDP datagrams and 4/4 HTTP transfers
at 23.778 Mbit/s. TX ownership balanced at 4,786/4,786, RX at 692/692, and PP
publication at 20,691/20,691; ESF rejection and all allocation counters
remained zero. The full final-ELF audit, including static binding and PM init,
reports 6,407 functions and zero violations.

## Completed slice: exact completed-RX-unit identity

The first Rust outer RX walk incorrectly named and forwarded the first
descriptor seen after `wDevCtrl.head` as the argument to
`wDev_ProcessRxSucData`. That interpretation is valid only for a
single-descriptor unit. The pinned 0x150-byte
`libpp.a[wdev.o]::wdevProcessRxSucDataAll` body proves the actual ABI:
`+0xc2` tests bit 30 on the current descriptor, `+0xfe` moves the accumulated
count into `a1`, `+0x100` moves that same current descriptor into `a0`, and
`+0x102` calls `wDev_ProcessRxSucData`. The argument is therefore the
descriptor carrying the completion marker: the unit tail.

The inner routine obtains the unit head independently from `wDevCtrl.head` and
retains the argument as the exact tail later supplied to indication or
discard/recycle. Rust now represents that pair as a non-`Copy`
`CompletedRxUnit { tail, count }`; consuming it is the only way the outer walk
can dispatch the unit. There is no duplicable head-shaped pointer at that
boundary. The pinned symbol-size audit now fixes all three relevant reference
bodies: outer walk 0x150, inner aggregate 0x6a0, and discard leaf 0x20.

The corrected image completed scan, WPA2, DHCP, 4,096/4,096 UDP datagrams and
4/4 HTTP transfers at 26.829 Mbit/s. It balanced 4,786/4,786 TX,
690/690 network RX, and all fixed-pool/recycler ownership with zero allocation
or rejection. The WDEV probe validated 702/702 completed units and the
asynchronous recycler completed 702/702 chains. This traffic contained only
single-descriptor units (`max_descriptors=1`), so the hardware run qualifies
the ordinary path; the multi-descriptor tail identity is currently established
by the pinned instruction sequence, not by a separate multi-descriptor HIL
case.

## Completed slice: safe RX metadata layout boundary

The first variable-offset operation inside the remaining 0x6a0-byte
`wDev_ProcessRxSucData` aggregate is the pinned 0x146-byte
`get_sublen_offset`. Its functional result is now reproduced by
`decode_rx_metadata_layout`, a safe Rust function over a fixed 44-byte prefix:
the base payload offset is 0x38; an optional seven-bit sublength plus the
boolean high bits of byte 0x2a is rounded to four bytes; and, when MAC register
`0x2010_4098` bit 23 is set, the ten-bit field in bytes 0x26..0x27 is rounded
and added when nonzero or explicitly present. The vendor log and PPDU-dump
side branches are intentionally absent under the already verified
`WIFI_LOG_NONE` profile.

As with the adjacent ROM leaves, GNU `--wrap` cannot safely interpose this
absolute export. The late linker fragment retains `0x2f8010f4` only as
`__real_wDev_ProcessRxSucData`, publishes
`wifi_strict_wdev_process_rx_success_data` under the public name, and asserts
the alias. The SRAM Rust boundary copies only the fixed metadata prefix and
validates the computed status offset against the descriptor length. Its first
qualified protocol route is now Rust-owned: status-zero, base-offset STA data
with promiscuous/error-dump/CSI modes disabled publishes the pinned `wDevCtrl`
metadata and frame-pointer fields, derives the exact copy and aggregate flags,
and enters the Rust-owned single-descriptor indication leaf. The same route now
owns ordinary STA association-response, beacon, and authentication management
frames. In the STA-only profile, Probe Request frames are also Rust-owned: the
pinned body rewrites their route from STA to AP, its optional observation
callback is proven null during strict preparation, and an absent AP interface
makes the final operation an immediate discard. Rust performs that exact
ownership transfer through the already qualified asynchronous recycler.
Action frames are now qualified as well. Exact disassembly proves that
`ic_interface_enabled(2)` reads only bit two of `wDevCtrl+0x31`, followed by
the FTM bit `0x04` in the word at `g_wifi_menuconfig+0x40`. Strict preparation
samples both while initialization is quiescent, rejects either enabled state,
and publishes one byte of immutable Rust policy. The hot RX path therefore
enters the common Action indication join without reading either hidden C
global. Control, AP/NAN, optional-metadata, error-status and unclassified
inputs still enter an explicit
`__real_wDev_ProcessRxSucData` fallback. This is not yet a claim that the
complete multi-descriptor aggregate or optional-metadata frame-indication
paths have been replaced.

Host tests cover the base layout, both rounded optional fields, a truncated
prefix, all three branches of the recovered aggregate-flag decoder, and the
data, management, exact Probe Request, and exact Action classifiers; the
runtime suite now passes 272 tests. The complete metadata/route probe owns 112
bytes of explicit internal-SRAM state and the immutable Action policy owns one
byte. Strict Rust static storage is 311,618 bytes and remains below the
qualified baseline. The counters are diagnostic migration state and can be
removed when the aggregate routes are fully Rust-owned.

The management measurement observed subtype bitmap `0x2912`: association
response (1), probe request (4), beacon (8), authentication (11), and action
(13). The Action post-port hardware run decoded 708/708 status-zero,
base-offset STA units: 694 data and 13 management aggregates entered the Rust
indication route, including 2 Action frames, while one Probe Request entered
the Rust discard route. No observed unit entered the vendor fallback. It
completed scan, authentication, association, the WPA2 four-way handshake,
DHCP, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers at 24.798 Mbit/s. TX
ownership balanced at 4,786/4,786 and network RX at 690/690, with zero
allocation and rejection counts. The exact decoder reported only aggregate
flag value zero in that run. Optional sniffer, CSI, NAN, error-status and
extended-metadata classes remain unqualified.

## Completed slice: single-descriptor RX indication

The common basic STA route no longer calls the ROM `wDev_IndicateFrame`.
Pinned ROM disassembly establishes its five-argument ABI, kind-7/kind-8 ESF
selection, two adjacent copies, descriptor stores, discard-before-publish
ownership order, and final `lmacRxDone` handoff. For one base-layout
descriptor with zero CSI length, the split copy at byte `0x38` is exactly one
bounded contiguous copy. Rust now claims either the fixed 32-object kind-7
SRAM pool or the initialized finite kind-8 small-RX free list, fills the
recovered ESF/RX descriptor layout, returns the hardware descriptor through
the existing Rust recycler, and publishes the new frame directly to the
Rust-owned RX queue. Pool exhaustion or malformed output fails immediately
and consumes the input unit; it never waits or enters a dynamic fallback.

The descriptor word contains two independent fourteen-bit values. Bits 0..13
are the backing segment capacity while bits 14..27 are the actual received
byte count. The first HIL attempt exposed this distinction: treating the
1700-byte capacity as the received length exhausted the kind-8 guard during
scan. `descriptor_received_length` now decodes the high field, checks it
against the low-field capacity, and every subsequent bound and copy uses only
the received length.

The qualifying HIL run completed passive scan, open authentication,
association, the Rust WPA2 four-way handshake, DHCP, 4,096/4,096 UDP
datagrams, and 4/4 HTTP transfers. It validated 715/715 successful RX units:
699 data and 13 management frames were published through the Rust indication
leaf, including three Action frames; three STA Probe Requests took the
qualified discard route. `rust_indicate_routes=712`, both indication reject
counters were zero, and both vendor indication and aggregate fallbacks were
zero. TX ownership balanced at 4,795/4,795 and network RX at 695/695.
Removing unsupported vendor benchmark-statistics calls from `wifi-primary`
also made the whole runtime allocation snapshot exactly zero, including
attempted/failed allocations, frees, and reallocations.

## Completed slice: singleton optional-sublength indication

The singleton Rust indication path now owns the second finite layout admitted
by the recovered `get_sublen_offset` contract: zero CSI/extended metadata with
an optional rounded sublength. `SingleRxCopyPlan` performs all variable-offset
arithmetic in safe, host-tested Rust. The SRAM ABI leaf copies the fixed
0x38-byte RX-control prefix, skips the rounded sublength, copies the remaining
MPDU bytes, and publishes `descriptor_length - rounded_sublength`, matching the
pinned ROM stores. CSI/extended metadata remains fail-closed because its raw
length and four-byte-aligned source offset are not yet one qualified
published-length contract.

The allocation selector now also matches the pinned ROM order. Copy mode zero
uses the fixed kind-7 pool. Copy mode one uses kind 8 only for inputs no larger
than 500 bytes, then immediately falls back to fixed kind 7 for a larger input
or an exhausted small pool. A successful preferred-pool fallback is not
reported as an allocation rejection; both pools are finite and neither path
waits or enters the public allocator wrapper.

Fat LTO temporarily removed the individual symbols for the already Rust-owned
`ppRxProtoProc`, `rc_get_trc`, and `rcUpdateRxDone` leaves. They are now
`inline(never)` and the final application link retains all three explicitly.
This is a proof boundary rather than a behavioral dependency: the strict audit
can inspect their independent call graphs and internal-SRAM placement under
every optimized link.

The updated primary ELF passed the 25-root, 6,407-function no-wait/no-heap
audit with zero violations. Its strict vendor-root graph reaches zero mutable
blob symbols/bytes, and strict Rust static storage remains 311,618 bytes. The
hardware regression completed scan, WPA2, DHCP, 4,096/4,096 UDP datagrams and
4/4 HTTP transfers at 24.382 Mbit/s. WDEV validated 707/707 units, Rust
indicated 706 and discarded one qualified STA Probe Request; both vendor
fallback counters and both indication reject counters were zero. TX ownership
balanced at 4,786/4,786, network RX at 691/691, and the complete allocator
snapshot remained zero. That workload observed 707 base layouts and no
optional-sublength layout, so it qualifies the base-path regression while the
new optional branch currently rests on pinned disassembly plus host tests.

## Next slices

Priority is now based on ownership leverage and total SRAM, rather than only
on mutable blob bytes:

1. Continue replacing the remaining raw Radio-owned packet transitions in
   `wDev_ProcessRxSucData` one vertical boundary at a time.
   The measured status-zero/base-offset STA data route and ordinary
   association-response, beacon, and authentication management routes are now
   Rust-owned. The STA-only Probe Request route rewrite/discard decision and
   the guarded Action route with NAN/FTM disabled are Rust-owned as well. The
   complete observed basic STA RX workload now reaches neither the vendor
   aggregate nor the ROM indication leaf. Retain fail-closed fallback for
   control, AP/NAN, CSI/extended metadata, error-status, and multi-descriptor
   classes while porting those indication variants. Singleton rounded
   sublength metadata with zero CSI is now Rust-owned. Remove the ROM leaf from
   the strict root graph only after every admitted mode has an explicit Rust
   owner or an intentional fail-closed policy.
   `ppRxProtoProc`, `rc_get_trc`, `rcUpdateRxDone`, `ppRecycleRxPkt`, and the
   public `esp_wifi_internal_free_rx_buffer` release boundary are now
   Rust-owned. The adjacent `wDev_DiscardFrame` head publication and transfer
   into the recycler are Rust-owned as a non-duplicable token as well; the
   remaining target is the aggregate frame-indication/dispatch body rather
   than this list transition.
   The 22 peer records, three route bitmaps, all schedule selection and all
   nine schedule arenas are now Rust-owned. `rcUpdateAckSnr`,
   `rcTxUpdatePer`, `rcUpdatePhyMode`, `rcAttach`, both public schedule getters
   and the default-interface selector no longer contribute rate-control
   ownership debt. Keep extending the typed `RateControlState` only when a
   newly admitted PHY mode exposes a state transition not covered by the
   current STA/AP qualification.
2. Use the separate Radio/Network ownership counts and existing high-water
   marks to overlay or remove only storage whose
   lifetimes are proven disjoint. Do not reduce the 32-entry TX pool without a
   new throughput qualification because it has reached full occupancy.
3. Replace NVS-shaped cold configuration storage with typed Rust
   configuration, then remove channel/function-table/interface cold ABI
   publishers one group at a time.
4. Move finite register leaves (TSF, TXQ state, CCA, CSI bandwidth, key table,
   and descriptor ownership) behind an experimental ESP32-S31 radio HAL.
5. Port full PHY calibration and `register_chipv7_phy` last, keeping the
   vendor image as a differential oracle until every adopted field and
   register sequence is qualified.

The strict-runtime descriptor head/tail ownership in `wDevCtrl` is explicit,
but the object is not yet fully retired: the delegated RX aggregate still
reads and updates metadata, mode and routing fields in it. Those accesses must
move into typed Rust state together with the common RX route before the
72-byte vendor object can be classified as cold-only.

For every slice, record coexistence-related fields even when Wi-Fi-only policy
does not use them. BT/BLE/802.15.4 support should be able to add a coordinator
above the radio HAL without rediscovering discarded ownership or register
evidence.

Public `ieee80211` and supplicant crates should remain behind adapters for now.
Adopting them before the hardware/state boundaries are stable would combine a
protocol migration with an ownership migration and make regressions harder to
localize.

## Completed runtime slice: ACK-SNR ownership

The pinned `libpp.a[trc.o]::rcUpdateAckSnr` body has no MMIO or global reads,
but it mutates the first two bytes of a caller-owned rate-control record
through an untyped pointer. Under the completion contract this is not an
admissible ROM leaf: Rust owns the record, so a ROM function must not mutate
it.

The recovered transform is now a safe value function over `[i8; 2]`:

- `0x7f` input leaves the filter unchanged;
- byte zero stores the latest signed sample;
- the first midpoint is zero when the previous sample is `0x7f`, otherwise it
  is the arithmetic half of previous plus current;
- byte one stores that midpoint when uninitialized, then applies the exact
  `(3 * old + midpoint) / 4` signed filter.

Host tests cover sentinel, initialization, negative rounding, steady-state and
positive samples. The exported `rcUpdateAckSnr` name is linked to a narrow Rust
ABI adapter which copies exactly two bytes into and out of the safe transform.
The original ROM address remains only as `__real_rcUpdateAckSnr` for
differential inspection. `rcUpdateTxDone` calls the Rust adapter directly, so
the strict runtime root has been removed without moving record ownership into
the adapter.

The final AP ELF binds the public symbol to Rust at `0x400c33ac` and retains
the old `0x2f801064` entry only as `__real_rcUpdateAckSnr`; final disassembly
contains no transfer to the old address. The linked-state graph decreased from
23 to 22 runtime vendor roots and from 36 to 35 reachable vendor functions,
while mutable blob state reachable from strict leaves remained zero. The
no-wait/no-heap audit covered 6,407 functions with zero violations. On
ESP32-S31 hardware an Android client completed WPA2 M1/M3, became authorized,
and reached the ready AP after the Rust replacement was flashed.

## Completed runtime slice: TX-PER and schedule lowering

The public `rcTxUpdatePer` symbol is now bound to
`wifi_strict_rc_update_tx_per`. Its ABI adapter accepts only one of the three
fixed default records or a currently claimed record from the 16-entry
Rust-owned peer pool. A schedule pointer is read only after its base,
12-byte alignment and arena bounds have been checked.

The state transition itself is safe Rust over `RateControlState`. It reproduces
the recovered retry penalty, joint counter rescaling at `0x0200_0000`, the four
retry-pressure bands, the threshold at seven, and the complete scalar clear
performed before lowering a schedule. Six host tests cover the boundaries,
wrapping byte behavior, counter rescaling, legacy fallback and all three HE
beamforming policy branches.

The exact ESP32-S31 ROM image used for the ROM-only leaf proof is
`esp32s31_rev0_rom.elf`, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
It places the 0x7e-byte `rcTxUpdatePer` body at `0x2f8375aa` and the
0x1a-byte `phy_read_hw_noisefloor` body at `0x2f827d72`. The latter is only a
volatile read of `0x2010708c`, conversion of its low 12-bit signed encoding and
an arithmetic divide by four, so the runtime now performs that MMIO leaf
directly. The three writes formerly performed by
`hal_he_set_bf_report_rate` and four byte-field writes from
`hal_he_set_ersu_ack_rate` are likewise direct, finite Rust MMIO sequences.

At this checkpoint the strict final-ELF audit reported 21 vendor roots, 34
reachable vendor functions, zero mutable blob globals reachable from strict
leaves and zero violations. Ownership debt was
`1 fallback + 10 stateful/unproven + 10 temporary MMIO`. Linking all currently
admissible schedule arenas retains one additional
12-byte compatibility object (`BAROFDMSched`), while the Rust transition adds
320 bytes of internal executable/read-only storage. Both costs are explicit in
the primary state baseline and are temporary until schedule contents move into
typed Rust storage.

Hardware qualification used the heap-free primary STA image: passive scan,
association, WPA2 four-way handshake, DHCP, gateway ping, DNS and HTTP all
completed. An 8 MiB device-to-host TCP transfer completed at approximately
20 Mbit/s in the ordinary non-throughput profile. TX ownership balanced at
5,946/5,946, the 32-credit pool reached its full qualified high-water mark, and
all invalid/full/contended/credit/peer rejection counters remained zero.

### Rust-owned schedule-bank checkpoint

The nine 12-byte schedule arenas are now represented by one exact 852-byte
Rust bank containing 71 typed `RateScheduleRef` records. Its initial bytes were
recovered from the pinned `libpp.a[trc.o]`; the sole `rcAttach` mutation,
writing each arena-local index to byte `0x0a`, is materialized at compile time.
Host tests enumerate all 71 references, prove pointer round trips and check the
recovered boundary records.

`RateControlState` now carries the current and legacy schedule references as
values. `wifi_strict_rc_update_tx_per` converts both ABI pointers at entry and
publishes a pointer only after the safe transition has selected another valid
reference. It no longer enumerates or reads the nine vendor globals. The
three fixed default contexts and four ROM ABI schedule cells likewise publish
only addresses derived from the Rust bank.

The complete default-interface `trc_update_ifx_phy_mode` selector and the
per-peer `rcUpdatePhyMode` transition are Rust-owned. They preserve the
recovered LoRa `[1, 0, 1, 0]`, dot11b record 3, P2P-dot11g record 7 and the
remaining HT/HE mode-to-schedule mappings while rejecting absent, foreign or
unclaimed records. `rcAttach` now initializes the Rust schedule bank and
records without calling the archive body; both public schedule getters return
only validated Rust-bank addresses.

The qualified final ELF therefore contains the one 852-byte Rust schedule bank
and none of the nine vendor schedule definitions. Four obsolete ROM-ABI
schedule cells are not live, fixed cold-init bindings decrease from 43 to 39,
and linked mutable blob state outside the strict runtime graph decreases by
nine symbols and 852 bytes, from 185/22,212 to 176/21,360. Internal sparse
schedule-kind tags also avoid a 160-byte SRAM pointer jump table. The
recursive final-ELF cold-init audit proves that `__wrap_trc_init` reaches
exactly the three fixed context initializers and their Rust schedule
publications, without a cycle or hidden memory access at the arena-base leaf.

On ESP32-S31 hardware the heap-free image completed passive scan, association,
WPA2 M1-M4, DHCP, ping, DNS and HTTP without entering `ppTask`. Allocation,
reallocation and free counts remained zero, other-core stalls remained zero,
and all TX/RX queues balanced without rejection. A default UDP receive run
accepted 27,162,000 bytes in 10.268 seconds (about 21.2 Mbit/s). The
`idf-iperf` queue profile accepted 30,126,600 bytes in 10.256 seconds (about
23.5 Mbit/s); the sender emitted 83.9 Mbit/s. The application observed one
peer RX BlockAck request but intentionally declined it, so this measurement is
a non-AMPDU RX baseline rather than a rate-control ceiling.

The qualified default image leaves 16,432 bytes for the CPU0 stack, only
48 bytes above the 16 KiB gate. Enabling the smaller RX-BlockAck profile
currently leaves 15,840 bytes (544 bytes short); the 40-descriptor/48-buffer
saturation profile overflows SRAM by 14,016 bytes. Reducing or overlaying
fixed RX storage is therefore the next prerequisite for a qualified aggregate
throughput measurement. These link-time failures do not indicate a
rate-control regression.

## Completed runtime slice: TXOP queue ownership

The pinned `libpp.a[lmac.o]` implementation uses a three-byte availability
array initialized to `[1, 1, 1]`. `lmacRequestTxopQueue` takes the first
non-zero byte, clears it, and writes the selected class `0..=2` to byte
`0x1d` of one 0x38-byte hardware-queue record. If no class is free it returns
zero without mutation. `lmacReleaseTxopQueue` restores that byte and writes
the sentinel class `3` back to the queue.

`TxopQueueState` now owns this finite transform in safe Rust. The persistent
three bytes are a single internal-SRAM object, published directly through
`g_txop_queue_status_ptr`; there is no C shadow or duplicated synchronization
state. Quiescent handoff proves all four hardware queues contain sentinel
class `3` before resetting the pool. The two narrow ABI adapters additionally
validate the queue index, `our_instances_ptr`, and old class, and trap on an
ownership invariant violation rather than indexing arbitrary memory.

Late linker aliases redirect both request and release, including the function
addresses installed in the WDEV callback table. The heap-free primary profile
now enables the already-qualified Rust implementation of all 43 static pointer
publications, which is required to publish the Rust TXOP object. The resulting
ELF contains `wifi_strict_txop_queue_status` (three bytes) and neither the
vendor request/release bodies nor private `g_txop_queue_status`.

The full strict audit covers 6,407 functions with zero violations. Runtime
vendor debt decreases to 20 roots and
`1 fallback + 9 stateful/unproven + 10 temporary MMIO`; reachable vendor
functions decrease to 33. The linked-state audit reports all 43/43 fixed
bindings, zero mutable blob globals reachable from strict leaves, and one
fewer outside blob object (185 objects / 22,212 bytes in the credentialed
primary image).

Enabling the Rust pointer publishers changes linker relaxation immediately
before the two fixed function-table allocations. Their qualified OSI-calloc
return offsets are therefore `wdev_funcs_init + 0x36` and
`net80211_funcs_init + 0x32`, two bytes beyond the vendor-publication profile.
The static allocator classifier selects the offset by profile; it still
requires the exact caller, allocation source, and sizes 1,560/332 bytes.

Hardware qualification of this exact image completed passive scan, open
authentication, association, the WPA2 four-way handshake, DHCP, gateway ping,
DNS, TCP and HTTP without entering `ppTask`. The post-link snapshot remained
at zero allocations, reallocations, frees and failures. All 19 submitted data
frames returned their static TX slots, all 16 received frames returned their
static RX slots, the 32-credit TX pool was balanced, and no other-core stall
was observed.

## ROM ELF as a deblob oracle and the first direct radio-HAL leaves

The rev0 ROM ELF is now treated as the primary oracle for replacing temporary
ROM calls. The qualified artifact is `esp32s31_rev0_rom.elf`, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
It is an unstripped 32-bit little-endian RISC-V ELF with 128 sections, 4,192
defined symbols and 3,488 text functions. It has no DWARF type information,
but its symbol sizes, complete machine code and named ROM-owned RAM sections
are sufficient to distinguish finite stateless/MMIO leaves from functions
that access hidden ROM ABI state. The artifact is reference material only and
is not linked into the firmware.

The first completed leaf is `hal_get_tsf_time` at ROM address `0x2f82b9f8`,
size `0x3e`. Its complete body has no calls, cycles, wait, allocation or
ROM-owned RAM access. It sets latch bit one for interface zero or bit two for
any non-zero interface in `0x2010_d814`, reads the high and low TSF words from
`0x2010_d824` and `0x2010_d820`, then clears the same latch bit.

`wifi_strict_hal_get_tsf_time` reproduces this transaction with volatile MMIO
in a 44-byte internal-SRAM Rust leaf. The final linker aliases the public
`hal_get_tsf_time` symbol to this Rust address while retaining
`__real_hal_get_tsf_time = 0x2f82b9f8` only as a differential oracle. The
generated RV32 body preserves the ROM high-then-low read order and the `u64`
return ABI, and contains no call or backward edge.

The next completed leaf is `hal_mac_rx_get_last_dscr` at ROM address
`0x2f8386a2`, size `0x1e`. Its entire state is two register reads: the low
20 address bits from `0x2010_408c` and the high 12 address bits from
`0x2010_4c70`. `wifi_strict_hal_mac_rx_get_last_dscr` preserves the ROM
low-register-then-high-register read order and joins only those disjoint
fields. The generated internal-SRAM RV32 implementation is also exactly
`0x1e` bytes, has no call or backward edge, and the original ROM address is
retained only as `__real_hal_mac_rx_get_last_dscr`.

The exact credentialed STA ELF passed the complete strict audit over 6,407
functions with zero violations. Runtime vendor debt decreased to 18 roots and
`1 fallback + 9 stateful/unproven + 8 temporary MMIO`; reachable vendor
functions decreased to 31. Mutable blob state reachable from strict leaves
remained zero, all 43 fixed cold-init bindings remained active, and the
strict-static baseline remained 82 sections / 312,441 bytes.

Hardware qualification completed passive scan, WPA2 association and four-way
handshake, DHCP, gateway ping, DNS, TCP and HTTP 200 without entering
`ppTask`. Allocation, reallocation, free and failure counters all remained
zero. TX ownership balanced at 18/18, RX ownership at 16/16, the 32-credit TX
pool returned to zero use, and no other-core stall was observed.

The next qualified group removes five complete MAC-control bodies from the
pinned `libpp.a` runtime graph:

- `hal_mac_tx_set_cca` is a single read-modify-write of `0x2010_4c5c`. It
  replaces bits 31:30 with the low two bits implied by the RV32
  `cca << 30` operation and returns zero.
- `hal_mac_is_txq_valid`, `hal_mac_set_txq_invalid` and
  `hal_mac_txq_disable` access queue control word
  `0x2010_4d70 - 0x10 * queue`. This is the exact result of the vendor
  `(0x0201_04d7 - queue) << 4` sequence. The first reads bit 30, the second
  clears bit 30, and the third clears bits 31:30.
- `hal_mac_set_csi_cbw` is an evidenced two-byte `ret`; the pinned S31
  archive ignores its argument and performs no state mutation.

All five public symbols now alias internal-SRAM Rust leaves. Their generated
RV32 bodies occupy `0x2f00_8f4e..0x2f00_8fca`, contain no call, indirect
branch or control-flow cycle, and reproduce the archive masks and access
addresses mechanically. They are treated as replaced vendor roots rather
than allow-listed MMIO debt.

The exact credentialed STA image passed the strict audit over 6,407 functions
with zero violations. Runtime vendor debt is now 13 roots and
`1 fallback + 9 stateful/unproven + 3 temporary MMIO`; reachable vendor
functions decreased to 26. Strict-leaf blob state remains zero, all 43 fixed
cold-init bindings remain active, and the strict-static baseline remains
82 sections / 312,441 bytes.

Hardware qualification observed six APs, completed WPA2 association and the
four-way handshake, obtained `192.168.178.138` by DHCP, and passed gateway
ping, DNS, TCP and HTTP 200. Static TX ownership balanced at 18/18 and RX at
15/15; all allocation counters, failures and other-core stalls remained zero.
`ppTask` was never entered.

A later hardware run exposed and corrected an error in the original address
translation for the three queue-control leaves. The archive forms
`0x0201_04d7 - queue` and then shifts the complete value left by four; the
register is therefore `0x2010_4d70 - 0x10 * queue`. The earlier Rust
translation accidentally dropped the upper nibble and tried to read
`0x0100_4d70`. The strict exception handler stopped at
`wifi_strict_hal_mac_is_txq_valid` with `mtval=0x01004d70`, before silently
continuing with invalid state. After correction, final-ELF disassembly
materializes `0x2010_4d70`, and the same post-link path completed without a
trap.

Two complete `libphy.a[phy_reg.o]` leaves are now Rust-owned as well.
`phy_set_rx_comp_new` replaces the low byte of `0x2010_702c` and the high byte
of `0x2010_70a0` with `0xed`, preserving the vendor access order.
`phy_dc_mem_clr` pulses bit 20 of `0x2010_703c`; its Rust body deliberately
performs the vendor's fresh volatile read between the set and clear writes.
The exact meaning of the compensation fields and pulse is left undocumented
beyond those observed transactions rather than guessed from the symbol names.

Both public archive symbols resolve directly to SRAM Rust code.
`wifi_strict_phy_dc_mem_clr` is the same `0x1c` bytes as the reference body;
`wifi_strict_phy_set_rx_comp_new` is `0x24` bytes versus the vendor `0x28`
while preserving both RMW operations. Neither generated body contains a call,
indirect branch or cycle.

The strict runtime graph is now 11 vendor roots with
`1 fallback + 9 stateful/unproven + 1 temporary MMIO`; 24 vendor functions
remain reachable. The exact ELF again passed all 6,407-function control-flow,
heap and wait checks with zero violations. Hardware qualification switched
through the passive-scan channel sequence, observed seven APs, completed
WPA2/DHCP/ping/DNS/TCP/HTTP, and balanced TX at 19/19 and RX at 17/17.
Allocation and other-core-stall counters remained zero, and `ppTask` was not
entered.

## Completed strict-runtime slice: `wDevCtrl`

The pinned `libpp.a[wdev.o]` defines a 72-byte initialized object. Its byte
`0x2e` is `0x60`; archive-wide relocation inspection finds four readers and no
writer. In the currently qualified strict graph only `rcUpdateTxDone` reads
that byte. It converts the descriptor's encoded ACK-SNR byte to the signed
sample consumed by the otherwise stateless `rcUpdateAckSnr` leaf.

The Rust `rcUpdateTxDone` boundary now performs the finite validation and
field selection itself, uses the evidenced `0x60` encoding constant, and
delegates only to `rcUpdateAckSnr` and `rcTxUpdatePer`. The mesh-only retry
clamp is deliberately outside the basic AP/STA profile and is documented at
the adapter. This removes `wDevCtrl` from ordinary TX completion without
copying the opaque C object into Rust.

The other former strict referrer, `esp_test_set_rx_error_occurs`, only increments
external diagnostic counters when test byte `wDevCtrl[0x44]` is nonzero. The
strict profile replaces it with its successful no-op result, consistently
with the existing optional TX/RX diagnostic wrappers.

Both public ESP32-S31 names are absolute ROM exports, so the late linker
fragment binds them directly to uniquely named Rust functions and retains the
pinned ROM addresses only as `__real_*` aliases. The final ELF proves those
addresses. The corrected relocation audit stops before the replaced vendor
bodies and now reports one remaining strict mutable blob object:
`phy_param` (508 bytes); `wDevCtrl` is present only outside the strict graph.

Hardware verification exercised the Rust rate-completion path under WPA2 STA
load: scan, association, four-way handshake, DHCP, ping, DNS, TCP/HTTP, 4096
UDP datagrams and four HTTP transfers completed. All 4786 TX credits and 691
RX credits were returned, no PP publication was rejected, and allocation
counters did not change after handoff.

The static cold-init path is now the default `wifi-primary` STA profile. Its
former 8 KiB bootstrap allocator arena has been removed together with the
`esp-alloc` dependency. The 2026-07-25 final image leaves 22,648 bytes of CPU0
stack against the unchanged 16,384-byte minimum, so the earlier stack-budget
debt is resolved rather than hidden by weakening the gate. Allocator-shaped C
and Rust ABI entries are fail-closed `ebreak` sentinels with no backing
storage; the final-ELF audit rejects an allocator implementation or heap
section.

## In-progress slice: `phy_param`

The first strict PHY step no longer calls `phy_change_channel`,
`phy_set_chanfreq`, or `phy_chip_set_chan`. The Rust radio owner directly
executes the recovered finite channel-programming sequence. Before handoff it
adopts only the instruction-evidenced fields:

- frequency offset at `0x20`;
- channel-14 MIC gate at `0x26`;
- 802.11p policy bytes at `0x28..=0x29`;
- crystal selector at `0x4f`;
- TX-gain skip, seed, configuration, calibration curve, correction, base, and
  delta fields at `0x07`, `0xa8..=0xbf`, `0xd0..=0xd1`, `0xf1..=0xf7`,
  `0x123`, and `0x1b2`;
- current channel/init/CBW at `0x11c..=0x11f`.

The qualified `phy_i2c_enter_critical` and `phy_i2c_exit_critical` bindings
are each a single `ret`; the Rust path omits them because the sequence belongs
to one radio owner. The `g_phyFuns+0x14` indirect call is the cold-published
`phy_set_rx_comp_new` leaf and is now direct. `phy_11p_set` was also removed
from runtime: its complete body only writes the same two policy bytes back to
`phy_param`.

Absolute ROM leaves have no bytes in the final ELF, so the no-wait auditor
keeps the old `phy_change_channel` graph as a reference-only control-flow
oracle. The linked-state auditor uses only the real Rust runtime roots. This
distinction prevents the removed `phy_chip_set_chan` body from being reported
as live while retaining conservative checking of its lower calls.

The normal TX-gain path is now Rust-owned as well. The three pinned
`phy_tx_gain.o` tables are represented as typed aligned halfword arrays.
Rust calls the absolute-ROM `phy_wifi_get_tx_gain` oracle with the adopted
calibration profile and stack-owned fixed output arrays, then calls the finite
TX-gain register encoder directly. This removes
`phy_wifi_set_tx_gain_new` and the cold-published `g_phyFuns+0x24` callback
from the strict runtime graph. It adds 42 bytes to the explicit PHY state
(the aligned section grows from 6 to 48 bytes) instead of retaining an opaque
508-byte owner.

The encoder itself is now Rust-owned too. Its reference is the complete
`0x130`-byte `libphy.a[phy_tx_gain.o]::phy_set_tx_gain_mem_new` body together
with the complete ROM bodies `phy_txbbgain_to_index` and
`phy_write_gain_mem`. The former is a five-value pure mapping; the latter
writes the three gain words at `0x2010_0848..=0x2010_0850` and then updates
the index field at `0x2010_0844`. The replacement accepts only the evidenced
16- or 32-entry bounds, traps null inputs before any dereference, has no
allocation, wait, indirect call, hidden state, or hardware-dependent exit,
and is emitted as a `0x170`-byte SRAM leaf with no calls.

The vendor ABI requires one contiguous 192-byte scratch layout rather than
four unrelated arrays. `TxGainScratch` now makes that contract explicit:
six seed words at offset 0, eight 32-bit output words at 24, sixteen 64-path
words at 56, and eighteen 72-path words at 120. Compile-time offset and size
assertions prevent a Rust layout change from silently changing the oracle
inputs. This also documents the recovered overlap by which baseband-gain
indices three and four select halfwords in the 32-bit output region.

JTAG inspection after the qualified DE cold initialization measured
`phy_param[0x26] == 0`: the optional channel-14 MIC/power mode was disabled.
Strict handoff now checks that byte and returns
`PhyChannelStateAdoptionError::Channel14MicEnabled` instead of adopting an
unsupported profile. The runtime therefore supports the qualified channel
range 1 through 13 and does not call `phy_chan14_mic_cfg_new`. This is
deliberately fail-closed: channel 14 can be added later as a separate typed
power profile after the complete ROM calibration contract is known.

The final linked-state audit consequently reports zero mutable blob globals
reachable from strict runtime leaves. This does not yet remove `phy_param`
from the image: Rust still reads its evidenced fields once during cold
handoff, and the remaining cold vendor PHY initialization still owns and
populates the object.

The audit now resolves member-local RISC-V data relocations such as
`.LANCHOR0` back to their unique global symbol and filters referrers against
the final ELF. In the direct cold call graph rooted at
`register_chipv7_phy`, only one mutable blob object remains: `phy_param`
(508 bytes). Its direct cold referrers are narrowed to
`register_chipv7_phy` and `phy_get_romfunc_addr`; indirect callbacks
published to ROM remain a separately stated limitation. This makes those
functions the next concrete cold-PHY ownership frontier instead of treating
all linked PHY helpers as equally live during initialization.

The first cold-PHY parameter-transfer group is now interposed in Rust.
`register_chipv7_phy_init_param` copies exactly 71 bytes from the 128-byte
init profile into six disjoint `phy_param` ranges. The mapping is kept as
offsets because the meaning of most fields is not published. The Rust
`phy_rfcal_data_sub_new` transform copies all 508 parameter bytes to or from
`cal_data[12..520]`; its backup and recovery entry points preserve the
vendor return contracts. The reference bodies are the complete pinned
`phy_init.o` functions and the complete rev0 ROM `phy_byte_to_word` body at
`0x2f826034`. All bounds are compile-time constants, null inputs trap before
dereferencing, and no allocation, wait, MMIO, callback, or hidden state is
introduced.

Final-ELF disassembly proves that `register_chipv7_phy` calls the uniquely
named Rust init and calibration-transfer boundaries. The transfer loops
currently lower to the stateless ROM `memcpy` leaf; this is an admissible
temporary input/output helper, not a ROM state owner, and is simple enough to
replace when the remaining cold object is removed. `phy_param` remains
vendor-defined because the other live functions from the same `phy_init.o`
member have not yet all been ported. We deliberately do not patch or weaken
the archive: the remaining object will switch ownership only after the whole
member can stop being extracted.

The ROM ABI publication performed by `phy_get_romfunc_addr` is now Rust-owned
as well. Its primary reference is the unstripped rev0 ROM ELF
`esp32s31_rev0_rom.elf`, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`.
That ELF proves that `phy_get_romfuncs` at `0x2f824a82` is only a load from
the pointer cell at `0x2f07fc3c`, while `phy_param_addr` at `0x2f824a8c` is
only a store to the parameter cell at `0x2f07fc40`. The cell selects the
52-byte, 13-entry `g_phyFuns_instance` table at `0x2f07f944`.

Rust models all 13 entries with a compile-time checked `repr(C)` layout,
validates the table address and the two callbacks which the pinned vendor
body intentionally preserves, publishes `phy_param`, and replaces the
remaining 11 entries in the exact vendor store order. The preserved entries
are `phy_txcal_debuge_mode_` at `0x2f8244fe` and
`phy_get_tone_sar_dout_` at `0x2f8266da`. The two no-op I2C critical
callbacks reproduce the pinned two-byte ROM/vendor leaves and reside in
internal SRAM because ROM may call them while cached execution is
unavailable. Final-ELF disassembly proves that public
`phy_get_romfunc_addr` resolves to Rust code whose body contains only bounded
loads, validation branches, and stores: it has no call to either ROM
accessor, no indirect call, allocation, wait, or loop.

The four-byte `g_phyFuns` storage is now physically Rust-owned without
patching the archive. The linker publishes the C name as an exact alias of
`wifi_strict_phy_rom_function_table_binding`, whose initialized value is the
fixed rev0 table address `0x2f07f944`. The binding uses `UnsafeCell<u32>` only
to retain writable ELF section flags required by the C ABI; Rust exposes no
runtime mutation API. `phy_change_channel` no longer reads the alias at
runtime and instead uses the compile-time fixed table address.

The final-link state auditor refuses to classify this as a transfer merely by
name. It proves that both names have the same nonzero address, that the Rust
backing is exactly four bytes, mutable, present in a real ELF section, and
placed in internal SRAM. Only then is `g_phyFuns` excluded from the
blob-owned inventory. The ten still-linked vendor readers are
`phy_bt_set_tx_gain_new`, `phy_bt_tx_pwctrl_init`, `phy_cal_param_track`,
`phy_chip_set_chan`, `phy_start_tx_tone_step_new`, `phy_tx_cap_init`,
`phy_tx_gain_print`, `phy_tx_pwctrl_init`, `phy_txdc_cal_pwdet_new`, and
`phy_wifi_set_tx_gain_new`; they now load the Rust-owned ABI binding. The
fixed 52-byte callback table itself remains a rev0 ROM-ABI RAM object and is
temporary debt until those readers and all ROM callbacks have moved.

This removes `g_phyFuns` from hidden C state but does not yet claim physical
ownership of `phy_param`. Once every remaining live function from
`phy_init.o` has been ported, that archive member can stop being extracted
and Rust can define the final 508-byte cold object explicitly.

The migrated sequence passed the strict hardware workload: passive
scan, WPA2 association, four-way handshake, DHCP, ping, DNS, TCP/HTTP, 4096
UDP datagrams and four HTTP transfers. All 4786 TX credits and 690 RX credits
were returned, no PP post was rejected, and post-handoff allocation counters
were unchanged. The first channel-state qualification measured 25.024 Mbit/s.
The subsequent Rust-owned TX-gain qualification returned all 4786 TX and 691
RX credits, rejected no PP publication, and measured 26.309 Mbit/s. The final
channel-14-invariant build repeated the complete workload, returned all 4786
TX and 691 RX credits, rejected no PP publication, and measured
28.278 Mbit/s. After replacing the final TX-gain MMIO leaf and correcting the
queue-register address, a focused regression again completed passive scan,
WPA2, DHCP, gateway ping, DNS, TCP and HTTP 200. It returned 19/19 TX and
17/17 RX owners, reported zero allocation operations and other-core stalls,
and remained running for a further 30 seconds without a trap or `ppTask`
entry. The final strict debt is `1 fallback + 9 stateful/unproven + 0
temporary MMIO`; 10 vendor roots and 23 reachable vendor functions remain.
The subsequent cold-parameter-transfer image exercised full calibration,
including Rust init mapping and backup, then completed scan, WPA2, DHCP,
ping, DNS, TCP and HTTP 200. It returned 18/18 TX and 15/15 RX owners with
zero allocation operations, other-core stalls, or `ppTask` entries. Recovery
has exact host coverage over all 508 bytes; a warm/no-calibration hardware
cycle remains a separate qualification item.

The subsequent Rust ROM-ABI-publication image repeated a cold full-calibration
boot, passive scan, WPA2 association and four-way handshake, DHCP, gateway
ping, DNS, TCP and HTTP 200. The callback table cell contained the expected
`0x2f07f944`; post-link traffic returned all 18/18 TX and 15/15 RX owners
with no rejection. Allocation, reallocation and free counters remained zero,
`ppTask` was never entered, other-core stalls remained zero, and a further
30-second interrupt-active run produced no trap or reset.

The subsequent Rust-owned `g_phyFuns` binding image repeated cold
full-calibration, passive scan of seven BSS records, WPA2 association and the
four-way handshake, DHCP, gateway ping, DNS, TCP and HTTP 200. Post-link data
returned all 19/19 TX and 16/16 RX owners. Allocation, reallocation and free
counters remained zero, `ppTask` was never entered, and other-core stalls
remained zero. The qualified final ELF reports one cold PHY blob symbol /
508 bytes, 175 other linked blob symbols / 21,356 bytes, and 84 Rust strict
sections / 313,297 bytes. Its CPU0 stack span remains 16,432 bytes, exactly
the same as the preceding non-stress profile.

The bounded calibration-record check/write transform is now Rust-owned too.
The complete pinned `phy_init.o::phy_rfcal_data_check_new` body and the rev0
ROM ELF leaves `phy_set_mac_data`, `phy_get_mac_addr`, and
`phy_byte_to_word` establish the exact contract: refresh the four-byte RF
calibration version and eight-byte identity prefix, sum 130 little-endian
words over bytes `0..520`, and either write or compare the one's-complement
checksum at `0x208..0x20c`. The identity permutation comes from the public
`EFUSE_RD_MAC_SYS0/1` registers at `0x20715050` and `0x20715054`; the third
ABI argument is instruction-proven unused. Rust exposes only a fixed
524-byte view, uses wrapping arithmetic and compile-time loop bounds, and
traps a null record before MMIO or dereference.

In the qualified final ELF, both the validation and full-calibration branches
inside `register_chipv7_phy` call the Rust boundary at `0x400d1836`. Its
release body contains direct eFuse loads, bounded byte loads/stores and one
fixed 130-iteration loop, with no `jal`, `jalr`, allocation, wait, callback,
or access to hidden mutable state. The three former ROM helpers remain only
as absolute exports and are not called by this path. The cold hardware run
completed full calibration, passive scan, WPA2, DHCP, gateway ping, DNS,
TCP and HTTP 200; it returned 18/18 TX and 15/15 RX owners, kept all
allocation counters and other-core stalls at zero, never entered `ppTask`,
and remained stable for a further 15 seconds. The strict audit still reports
zero violations and unchanged debt of `1 fallback + 9 stateful/unproven +
0 temporary MMIO`.

Two adjacent cold clock leaves are now Rust-owned. The complete pinned
`phy_get_xtal_freq` body previously called `rtc_clk_xtal_freq_get`, but both
ESP-IDF and the S31 HAL define the chip's crystal as fixed at 40 MHz. Rust
therefore writes the evidenced zero profile code to `phy_param[0x4f]` and
replaces bits 5:0 of `0x2010_f028` with 39 directly; there is no remaining
clock-query call or implicit state source. The complete
`phy_close_fe_bb_clk` body is reproduced as its exact three-register
transaction: zero `0x2010_0400`, clear bits 1:0 of `0x2010_0800`, then zero
`0x2010_7c80`. Unknown field meanings are deliberately not guessed.

Final-ELF disassembly places both functions in internal SRAM at
`0x2f00919c` and `0x2f008fca`. The crystal function is 26 bytes and the clock
close function is 32 bytes; neither contains a call, loop, wait, allocation,
or non-evidenced state access. `register_chipv7_phy` calls the former and
`phy_xpd_rf_new` tail-calls the latter directly. A cold full-calibration HIL
then completed scan, WPA2, DHCP, ping, DNS, TCP and HTTP 200, returned all
18/18 TX and 15/15 RX owners, preserved zero allocation and other-core-stall
counters, never entered `ppTask`, and ran for a further 10 seconds without a
trap or reset.

The complete post-initialization register update is now Rust-owned as well.
The reference is the pinned `libphy.a[phy_init.o]::phy_reg_update_new` body
plus the complete rev0 ROM `phy_wifi_agc_sat_gain` body at `0x2f827db0` and
the pinned `libphy.a[phy_reg.o]::phy_set_ftm_en` body. Rust preserves their
instruction-proven ordering: set bit 26 at `0x2010_705c`; write
`0x0818_212d` to `0x2010_7064` and `0x2010_7114`; replace bits 8:0 at
`0x2010_7104` with `0x1c0`; perform the two separately read
read/modify/write transactions on `0x2010_78c8`; and set bit 0 at
`0x2010_7d4c`. Names describe the vendor symbol or the observed operation;
unknown register-field semantics are not inferred.

The two private leaves are incorporated into the Rust parent rather than
published as additional interposition symbols because archive relocation
inspection found no other caller. In the qualified final ELF,
`phy_reg_update_new == wifi_strict_phy_reg_update_new == 0x400d17cc`; the
body is 98 bytes and contains only finite MMIO loads/stores and `ret`, with
no `jal`, `jalr`, loop, allocation, or wait. Both
`register_chipv7_phy` and the caller-task `phy_wakeup_init` resolve their
calls to that address. The function remains flash-mapped: call-graph
inspection shows only cold initialization and the normal
`esp-phy::increase_ref_count` wakeup path, not an interrupt context.

An initial attempt to place this 98-byte caller-task leaf in the
interrupt-only SRAM section was rejected by the existing post-link memory
gate because it left 16,304 bytes for the CPU0 stack instead of the required
16,384. Moving only this proven caller-task leaf back to flash restored the
stack reserve without weakening the gate or moving any ISR handler/data out
of SRAM.

The identical qualified image completed a cold full-calibration boot,
passive scan, WPA2 four-way handshake, DHCP, gateway ping, DNS, TCP and HTTP
200. It returned all 19/19 TX and 16/16 RX owners, negotiated a 32-frame
TX ADDBA window, retained zero allocation operations and other-core stalls,
never entered `ppTask`, and remained stable for a further 10 seconds. A
preceding reset of the same image observed one failed M4 TX completion after
successful calibration, scan, association and M3 verification; this
intermittent TX-completion event remains tracked separately and is not hidden
as PHY qualification evidence. The strict final-ELF graph still reports
zero violations and unchanged debt of `1 fallback + 9 stateful/unproven +
0 temporary MMIO`.

## Completed PHY-I2C command RAM leaf and in-progress async RF init

The next cold-PHY boundary is deliberately wider than
`libphy.a[phy_init.o]::phy_rc_cal_init`. That vendor wrapper merely supplies
fixed tables to ROM `phy_rc_cal`; interposing the wrapper alone would retain
both the hidden `phy_param` mutation and the synchronous PHY-I2C/delay
implementation.

The unstripped rev0 ROM ELF identified the complete relevant bodies:

- `phy_get_data_sat` at `0x2f826024`, size `0x10`;
- `phy_get_rc_dout` at `0x2f8261ac`, size `0x96`;
- `phy_rc_cal` at `0x2f826242`, size `0x108`;
- `phy_chip_i2c_readReg_org` at `0x2f829ffa`, size `0x38`;
- `phy_chip_i2c_readReg` at `0x2f82a032`, size `0x50`;
- `phy_chip_i2c_writeReg` at `0x2f82a30e`, size `0x6a`.

The ROM read leaf publishes a command and then repeatedly reads busy bit 25.
The write leaf repeatedly reads that bit both before and after publication.
`phy_get_rc_dout` performs four masked writes, a synchronous
`ets_delay_us(100)`, one masked read, and two cleanup writes. These cycles and
the delay are not admissible in the strict async runtime.

The preparatory Rust module therefore owns only stateless command encoding,
single-observation start/finish leaves, and a finite RC-calibration transition
plan. It models the 13 recovered block read masks, the `0x0647` host-selection
bitmap, command registers `0x2010f800/0x2010f804`, read-mask register
`0x2010f81c`, host configuration at `0x2010f820`, and busy bit 25. A read
start adds a deliberate fail-fast pre-command busy check which is absent from
the ROM read body; it prevents an owner-contract violation from overwriting
an active transaction.

The transition plan exposes the 100-microsecond interval as an async timer
edge. A completion observer is called once only after an independently
delivered hardware or timer edge; a still-busy result is an incomplete or
timeout error, never permission to self-wake and poll again. The arithmetic
half of `phy_rc_cal` now mutates an explicit fixed-size Rust parameter image
and is host-tested on both sides of the ROM result-45 threshold.

One independent cold leaf from that frontier is now active:
`libphy.a[phy_i2c.o]::phy_i2c_master_cmd_mem_init`. Its complete `0x5be`-byte
reference body does not start an I2C transaction. It encodes exactly 45
three-byte commands, substitutes 19 values derived from the fixed
`phy_param` image, and writes the words to
`0x2010_fc00..=0x2010_fcb0`. Its only ROM callees are the pure
`phy_encode_i2c_master` at `0x2f82a81a`, size `0x0a`, and the one-store
`phy_i2c_master_fill` at `0x2f82a824`, size `0x0e`.

Rust now owns the full command template, the exact parameter substitutions,
the recovered saturation arithmetic, and the 45 finite volatile stores. The
active loop uses a monotonically increasing cursor over the sorted dynamic
indices rather than a `match` jump table. Both unchecked table reads are
locally preceded by the same explicit `cursor != 19` proof; this is narrowly
scoped target-adapter `unsafe`, not protocol-state ownership.

In the credentialed HIL ELF,
`phy_i2c_master_cmd_mem_init == wifi_strict_phy_i2c_master_cmd_mem_init ==
0x400d120c`. The Rust body is `0x10a` bytes. Final-ELF disassembly contains no
`jal`, `jalr`, `jr`, panic edge, allocation, delay, hardware-dependent exit,
or loop other than the statically bounded 45-command traversal. The two ROM
helpers remain absolute reference exports and are not called. The
non-credential primary image is 892,624 bytes and still passes the complete
6,407-function strict audit with zero violations.

Hardware qualification exercised this replacement during a cold
full-calibration boot. Passive scan observed the target AP, then open
authentication, HT20 association, the Rust WPA2 four-way handshake, DHCP,
gateway ping, DNS, TCP and HTTP 200 all completed. The post-link snapshot
reported zero allocations, reallocations, frees and failures; all 19 TX and
16 RX owners returned to their static pools, `ppTask` was never entered, and
no other-core stall occurred.

The parent `libphy.a[phy_init.o]::phy_rf_init` remains the correct activation
boundary for the actual asynchronous calibration runtime. Its complete
`0x122`-byte body sequences 26 direct operations. Inspection of every
reachable calibration leaf found more synchronous behavior than the RC
wrapper alone exposes:

- `phy_open_i2c_xpd_new`, size `0xac`, performs a 100-microsecond ROM delay
  and then enters `phy_wait_i2c_sdm_stable`;
- ROM `phy_wait_i2c_sdm_stable` at `0x2f823e76`, size `0x4a`, repeatedly
  compares the cycle counter and PHY-I2C result against `0x5b` until success
  or timeout;
- ROM `phy_rfpll_chgp_cal` at `0x2f825cd4`, size `0xf4`, can perform up to
  100 iterations, each containing a 20-microsecond delay and a masked
  PHY-I2C read;
- `phy_xtal_duty_cal`, size `0x392`, contains a delay and several bounded
  measurement/calibration loops;
- `phy_get_rc_dout` contains its already identified 100-microsecond delay and
  completion read.

Therefore these functions will not be interposed one at a time while leaving
their synchronous parent active. The next slice is an explicit Rust
`PhyRfInit` state machine: every command publication transfers ownership to
one in-flight token, every time interval is an async Rust timer edge, and
every completion is observed once after a hardware/timer wake. A deadline
failure terminates the calibration transition; it never becomes a
self-waking poll loop. Until that parent is active, cold boot still executes
the remaining vendor/ROM `phy_rf_init` sequence and the strict ownership debt
does not decrease merely because its command-RAM child is already Rust-owned.

The first child transition of that parent is now modeled but deliberately not
activated. Complete disassembly of
`libphy.a[phy_reg.o]::phy_open_i2c_xpd_new` establishes two paths. A nonzero
argument clears the upper halfword at `0x2070_4184`, clears bit 28 at
`0x2070_40f0`, and then delays for 100 microseconds. Both paths subsequently
set those fields, preserve the instruction-evidenced bit-31 clear/set pulse
when bit 30 of `0x2070_4208` was initially clear, ensure bit 31 is set, and
tail-call ROM `phy_wait_i2c_sdm_stable`.

The complete ROM wait body at `0x2f823e76` records the cycle counter at
`0x2010_d800`, uses an inclusive `9,999`-cycle bound, and repeatedly reads
PHY-I2C block `0x63`, register zero until the result is `0x5b` or the bound is
exceeded. Rust now separates this into:

- two finite, no-call MMIO adapters for the pre-delay and common register
  sequences;
- an optional `DelayMicros(100)` action completed only by the async timer;
- a `CheckSdmDeadline { maximum_cycles: 9_999 }` action;
- one non-blocking PHY-I2C read action per SDM sample;
- explicit `Stable` and `TimedOut` terminal outcomes.

A mismatching sample returns to the deadline-check state only after that
sample's I2C completion edge. The transition contains no future and has no
waker, so it cannot schedule or poll itself. Three host tests cover the
delayed and immediate paths, reject completions delivered out of order, prove
that every retry crosses both deadline and I2C edges, and prove timeout is
terminal. The complete 320-test suite passes serially. The target
`wifi-primary` build and both strict final-ELF audits also pass unchanged;
because the parent is not active, the new preparatory state machine is dead
stripped and the qualified runtime/cold-state metrics do not change.

The complete parent order is now recovered from pinned
`libphy.a[phy_init.o]::phy_rf_init` rather than inferred from individual
symbols. Its 26 operations are:

1. `phy_open_fe_bb_clk`;
2. `phy_bbpll_cal(1)`;
3. `phy_bias_reg_set(1)`;
4. `phy_open_i2c_xpd_new(1)`;
5. `ets_delay_us(10)`;
6. `phy_pbus_clear_reg`;
7. `phy_i2c_clk_sel(8)`;
8. `phy_i2c_bbpll_set(1)`;
9. `phy_adc_rate_set(1)`;
10. `phy_i2cmst_reg_init`;
11. `phy_pwdet_reg_init`;
12. `phy_fe_reg_init`;
13. `phy_tsens_read_init(1, phy_param[0x16])`;
14. `phy_tx_pwctrl_bg_init`;
15. `phy_i2c_rc_cal_set(3, 1, 9)`;
16. `phy_rc_cal_init`;
17. `phy_filter_dcap_set`;
18. `phy_i2c_readReg(0x62, 1, 0x0f)` into `phy_param[0x18e]`;
19. `phy_i2c_init1`;
20. `phy_rfpll_chgp_cal`;
21. `phy_i2c_master_cmd_mem_init`;
22. `phy_i2c_readReg_Mask(0x69, 0, 4, 3, 0)`;
23. conditional `phy_i2c_sar2_init_code(0x578)`;
24. `phy_xtal_duty_cal_init(0)`;
25. `phy_fe_reg_update`;
26. `phy_set_chan_freq_hw_init(2, 4)`.

This sequence is the activation ledger for the Rust parent state machine.
Finite MMIO operations can become direct actions; each delay becomes an
executor timer deadline; each PHY-I2C command becomes a uniquely owned
in-flight transaction completed by an external edge. The ledger prevents a
partially ported child from being mistaken for removal of the synchronous
vendor parent.

The first two parent leaves are now active Rust code. Complete rev0 ROM ELF
disassembly identifies `phy_open_fe_bb_clk` at `0x2f823ec0`, size `0x38`.
Rust reproduces its exact finite transaction: write `0x1e7` to
`0x2010_0400`, set bits 1:0 at `0x2010_0800`, write `0xffff_ffff` to
`0x2010_7c80`, and set `0x0040_000f` at `0x2070_401c`. The function is cold
only and remains flash-mapped.

The complete `phy_bbpll_cal` body at `0x2f827dbc`, size `0x1c`, clears bits
3:2 at `0x2010_f818`, then selects bit 2 for argument zero or bit 3
otherwise. It is also called by runtime channel switching, so the Rust
implementation is placed in internal SRAM. In the qualified final ELF,
`phy_open_fe_bb_clk == wifi_strict_phy_open_fe_bb_clk == 0x400d0cac` and
`phy_bbpll_cal == wifi_strict_phy_bbpll_cal == 0x2f008fcc`; their bodies are
56 and 26 bytes respectively and contain no call, indirect branch, loop,
allocation, delay, or hidden mutable-state access.

The hardware image exercised both replacements through cold full
calibration, a six-record passive scan, HT20/WMM association, WPA2 M1-M4,
DHCP, gateway ping, DNS, TCP and HTTP 200. It returned all 18/18 TX and 15/15
RX owners, recorded zero allocation/reallocation/free calls and zero
other-core stalls, and never entered `ppTask`. The strict 6,407-function
audit reports zero violations and unchanged runtime debt of one explicit RX
fallback. The linked cold-PHY state remains exactly one blob symbol,
`phy_param`, of 508 bytes.

The third parent operation is now modeled as an event-driven child too.
Complete `libphy.a[phy_i2c.o]::phy_bias_reg_set` disassembly proves that its
48-byte body ignores its argument and makes exactly two synchronous
`phy_i2c_writeReg` calls: block `0x6a`, register zero, value `0xaf`, followed
by block `0x6a`, register one, value `0x7f`. Both select PHY-I2C host one
under the recovered block table.

`BiasRegTransition` preserves those two commands and their order, but each
write advances only when the outer radio owner returns a completion carrying
the expected address. An out-of-order or duplicate completion fails closed.
It has no MMIO of its own, future, waker, timer, allocation, callback, or
hidden state; the executor will drive each action through the existing
single-command `try_start_write`/`try_finish_write` adapter. Host tests prove
the exact values, ordered completion contract, terminal state, and
instruction-proven argument invariance. The serial runtime suite now passes
323 tests. This child remains intentionally dead-stripped until the parent
`PhyRfInit` transition can own the whole sequence.

The first six parent operations are now composed by
`PhyRfInitPrefixTransition`. It exposes the active clock and BBPLL leaves as
finite MMIO actions, delegates the two bias writes to `BiasRegTransition`,
delegates the power-up, 100-microsecond timer and SDM deadline/read sequence
to `OpenI2cXpdTransition`, and finally emits the separate
`DelayMicros(10)` present in the parent body. It then delegates operation six
to `PhyPbusClearTransition`. No nested child completion is observable as an
intermediate terminal state.

The complete rev0 ROM `phy_pbus_clear_reg` body at `0x2f824572`, size `0x90`,
is not a finite no-wait leaf. It enters debug mode, performs twelve
`phy_pbus_force_test` transactions in a fixed order, and returns through
`phy_pbus_workmode`. Every force-test body publishes its encoded command at
`0x2010_0884` and busy-waits on sign bit 31 at `0x2010_0890`. The work-mode
tail samples bit one at `0x2010_9c18`; when set, it synchronously delays one
microsecond, applies a two-write pulse at `0x2010_702c`, delays another two
microseconds, and clears the pulse bit.

Rust separates that graph into finite radio-HAL leaves and explicit
ownership edges. `try_start_phy_pbus_force_test` takes one readiness sample
before publication and fails fast if another transaction owns the block.
`try_finish_phy_pbus_force_test` takes exactly one post-edge sample and either
clears the command bit or returns `Busy`; it never loops or wakes itself.
Debug/work-mode and pulse setup/clear are finite ordered MMIO operations.
The one- and two-microsecond waits are distinct executor timer actions.

`PhyPbusClearTransition` owns the exact twelve-command cursor. A completion
must carry the current command identity; stale or reordered completions are
rejected. A command still busy at its externally supplied deadline becomes
the terminal `ForceTestTimedOut` outcome rather than another poll. Both
conditional work-mode paths and both timer edges have host coverage.

The complete rev0 ROM `phy_i2c_clk_sel` body at `0x2f829f1c`, size `0x68`,
is a finite MMIO leaf. It performs two ordered read/modify/write operations
on each of `0x2010f824`, `0x2010f828`, and `0x2010f82c`. The first preserves
all bits outside mask `0x7c0` and publishes `(selection << 4) & 0x7c0`; the
second preserves all bits outside mask `0x3f` and publishes
`(selection >> 1) & 0x3f`. The parent supplies selection `8`, producing
field contributions `0x80` and `0x04`. There is no call, branch, delay, loop,
or mutable software state in the leaf.

Rust preserves the six-write ordering in
`configure_phy_i2c_clock_selection`; the parent exposes it as the finite
`ConfigureI2cClockSelection { selection: 8 }` action.

The next cold-parent operation is `phy_i2c_bbpll_set(1)`, not
`phy_fe_txrx_reset(1)`. This is pinned by the relocation at offset `0x4a` in
`libphy.a[phy_init.o]::phy_rf_init`. The reset leaf occurs in
`phy_wakeup_init`; an earlier slice accidentally composed that wakeup
operation into the cold prefix after reading the archive-wide relocation
table without preserving its section owner. The standalone finite reset HAL
leaf remains valid for the later wakeup port, but it is no longer part of the
cold transition.

The complete rev0 ROM `phy_i2c_bbpll_set` body at `0x2f82a67e`, size `0x54`,
contains three blocking PHY-I2C transactions on `(0x66, 4)` when enabling:
masked read/modify/write clears bits three and two, then a second read captures
the resulting byte. ROM stores that byte through the mutable `phy_param`
indirection at offset `0x4a`. Its disable branch reads the same hidden byte
and writes it back.

`I2cBbpllTransition` makes both directions explicit. Enable owns the masked
read, write, and snapshot-read completions and returns
`Enabled { register_snapshot }`. Restore requires that snapshot as a Rust
input. The cold parent carries the byte across later steps and returns it in
`ReadyForPowerDetectorRegisterInit`; it no longer needs to mutate or inspect
`phy_param[0x4a]`.

The complete rev0 ROM `phy_adc_rate_set` body at `0x2f82a6d2`, size `0x4a`,
contains one blocking subgraph followed by a finite MMIO suffix. Its
`phy_i2c_writeReg_Mask(0x66, 0, 4, 3, 2, !rate * 2)` call first reads the
PHY-I2C byte, replaces bits two and three, then writes the byte back; both ROM
transactions busy-wait. For the parent rate `1`, that field is cleared.
The suffix uses two fresh reads of `0x20100448` to publish rate bit zero into
bits one and zero separately.

`AdcRateTransition` separates the masked operation into identity-bound
`ReadI2c`, `WriteI2c`, and finite `ConfigureMmio` actions. The read result is
owned by the transition, so no hidden byte or mutable C state crosses the
async edges. It never retries or samples from `poll`.

The complete rev0 ROM `phy_i2cmst_reg_init` body at `0x2f8276c4`, size
`0x22`, is finite MMIO-only code. It uses two fresh reads of `0x2010f818`:
the first replaces field `0x600` with `0x400`, and the second sets `0x40`.
Rust preserves both writes as `ConfigureI2cMasterRegisters`.

The complete rev0 ROM `phy_pwdet_reg_init` body at `0x2f82634a`, size `0x5c`,
is six finite stores. Rust preserves constants `0x0f0f0fff`,
`0x00ff0f64`, and `0x0000aaaa`, both separately sampled field updates at
`0x20100808`, and the final mode field at `0x20701068`. There is no branch,
call, delay, loop, or software-state access.

The complete rev0 ROM `phy_fe_reg_init` body at `0x2f827740`, size `0xf6`,
contains seventeen MMIO writes across ten registers. Rust uses explicitly
unrolled set/clear/replace operations and preserves every fresh read,
including the repeated bit-one/bit-zero writes at `0x20100448`. It contains
no wait, delay, loop, callback, or software-state access.

The complete pinned `libphy.a[phy_tsens.o]::phy_tsens_read_init` body is
`0x36` bytes. Instruction inspection proves that it ignores both ABI
arguments: it performs four MMIO writes, loads constant one into `a0`, and
tail-calls ROM `phy_set_tsens_power_` at `0x2f825dc8`, size `0x1c`.
Consequently the parent load of `phy_param[0x16]` is dead at the callee
boundary and Rust does not carry or publish that byte.

Complete ROM `phy_tx_pwctrl_bg_init` at `0x2f8267f6`, together with
`phy_en_pwdet` and `phy_pwdet_sar2_init`, is another finite MMIO chain.
Rust preserves three separate power-detector bit clears, both SAR2 field
updates, the `0x16a` store, the auxiliary-mode update, and the final
background-control bit.

Complete ROM `phy_i2c_rc_cal_set` at `0x2f82a634`, size `0x4a`, performs
three blocking `phy_i2c_writeReg_Mask` calls: `(0x6b, 0x11, bits 5:4, 3)`,
`(0x6b, 0x0f, bits 7:3, 1)`, and
`(0x6b, 0x13, bits 5:2, 9)`. Rust now has a reusable
`MaskedI2cWriteTransition` which explicitly owns the read byte, pure field
transform, and later write completion. `RcCalibrationSetTransition` composes
the three operations without a synchronous sub-call or hidden wait.

The complete pinned `libphy.a[phy_init.o]::phy_rc_cal_init` wrapper supplies
only three fixed byte tables to ROM `phy_rc_cal` at `0x2f826242`, size
`0x108`. Rust now owns the complete operation rather than retaining that ROM
parent. It first exposes the calibration-complete flag (bit 23 of parameter
word `0xa4`) as an explicit owner observation. If the bit is clear,
`RcCalibrationTransition` performs the exact four masked writes, an async
100-microsecond timer edge, one masked read, and two cleanup writes recovered
from ROM `phy_get_rc_dout`. The final `ApplyResult` action invokes the
already-Rust-owned arithmetic transform on the explicit parameter image,
including bytes `0xe8..=0xf0` and the completion flag. If the flag was
already set, no I2C action or delay is scheduled.

Complete ROM `phy_filter_dcap_set` at `0x2f82a476`, size `0x1be`, reads only
parameter offsets `0xe9`, `0xea`, `0xed`, `0xee`, and `0xf0`, applies the
finite `phy_get_data_sat` transform, then performs 18 blocking full-byte
PHY-I2C writes to block `0x67`. Rust captures those five bytes into
`FilterDcapParameters` after RC calibration and owns the exact write order in
`FilterDcapTransition`. Every write requires a matching non-blocking I2C
completion; the transition has no ROM call, global lookup, delay, retry, or
self-wake.

Operation eighteen is the full-byte
`phy_i2c_readReg(0x62, 1, 0x0f)`. The archive parent immediately stores its
result into hidden `phy_param[0x18e]`. Rust instead publishes one
identity-bound `ReadParameter18e` action through the existing non-blocking
start/finish primitive. Its completion inserts the byte into
`PhyRfInitParameterSnapshot`; neither the transition nor its result touches
the C global.

The complete pinned `libphy.a[phy_i2c.o]::phy_i2c_init1` body is a fixed
sequence of 26 blocking full-byte writes. Twenty-four values are constants;
the remaining two inputs are the newly read parameter byte `0x18e` and
`phy_param[0xee].wrapping_add(2)`. `PhyRfInitParameterSnapshot` now owns both
dynamic inputs together with the five-byte filter snapshot.
`I2cInit1Transition` publishes the exact 26-write order and advances only on
an address-matching completion.

Complete ROM `phy_rfpll_chgp_cal` at `0x2f825cd4`, size `0xf4`, begins with
three masked writes, then performs up to 100 synchronous
`delay(20 microseconds) + masked lock read` iterations. On the final miss it
calls blocking `ets_printf`, continues with a second calibration read, two
masked writes, and refreshes parameter byte `0x18e`.
`RfpllChargePumpTransition` exposes every delay and I2C operation as a
separate external edge. The final miss is retained as
`rfpll_lock_observed = false` instead of invoking a print path, and the
refreshed byte replaces the value in the owned parameter snapshot.

Operation twenty-one reuses the already recovered 45-word command-RAM
template, but no longer requires its temporary `phy_param` ABI. A second
finite adapter accepts `PhyRfInitParameterSnapshot`; host tests prove that
all 45 encoded words are identical to the global-image path. The cold parent
publishes that owned snapshot in `ConfigureI2cMasterCommandMemory`.

Operations twenty-two and twenty-three are now explicit as well. A
non-blocking masked read observes `(0x69, reg4, bits3:0)`. A nonzero value
skips initialization; zero expands ROM `phy_i2c_sar2_init_code(0x578)` at
`0x2f82a444`, size `0x32`, into one masked write of value `5` and one
full-byte write of `0x78`. Both branches preserve
`sar2_reinitialized` in the terminal state.

The prefix advances into operation twenty-four only after the first
twenty-three cold-parent operations. SDM and PBus timeouts terminate
separately and cannot run later hardware steps.

The complete `libphy.a[phy_rx_cal.o]::phy_xtal_duty_cal_init` wrapper is
`0x74` bytes. It reads `(0x61, reg9, bits5:0)`, clears bit five of
`(0x61, reg7)`, then performs calibration passes at frequency codes `0x988`
and `0x9b0`. The complete `phy_xtal_duty_cal` body is `0x392` bytes. Each
pass tests all 31 duty candidates `0x20..=0x3e`; every candidate requires an
external 20-microsecond timer edge and four signal-power measurements. A
sample outside `2/3..=3/2` of the initial mean is replaced at most twice.
The first candidate with the smallest signed filtered mean wins, so equal
values preserve the earlier candidate.

`XtalDutyCalibrationTransition` and `XtalDutySearchTransition` own that
wrapper, both passes, all samples and both results. They contain no future,
waker, allocation, print, delay implementation or self-progressing poll.
The parent captures `phy_param[0x4f]` and byte two of the image published
through ROM `phy_param_rom` as typed inputs before entering the transition.
The latter byte is forwarded by `phy_pbus_xpd_rx_on` to PBus selector zero,
path two; its electrical meaning is deliberately left unnamed until evidence
identifies it. `phy_set_txclk_en` and `phy_set_rxclk_en` are now exact Rust
MMIO leaves for bits 17:16 and 15:14 respectively at `0x2010_0890`.

Operation twenty-four no longer has aggregate `PrepareHardware` or
`RestoreHardware` completions. `XtalDutyPrepareTransition` exposes RF
frequency programming, tone setup, both clock gates, PBus debug mode, all ten
pre-search PBus commands, the Rust-owned RX-DCO transition, and exact
save/clear/restore ownership of bits 23:22 at `0x2010_0434`. The PBus list
includes the seven commands from ROM `phy_pbus_xpd_rx_on` followed by the
three direct commands in `phy_xtal_duty_cal`; every completion is bound to
the exact transaction. `XtalDutyRestoreTransition` exposes tone stop, both
clock gates, all three `phy_pbus_xpd_rx_off` commands and the conditional
work-mode tail. That tail can progress through its one- and two-microsecond
states only from identity-bound external timer completions. A PBus timeout is
a typed terminal failure, never a retry or poll.

Tone setup is now a complete Rust-owned S31 MMIO leaf. The primary references
are pinned `libphy.a[phy_reg.o]::phy_start_tx_tone_step_new`, size `0xc2`,
and its former `g_phyFuns + 0x30` target
`phy_txgain_comp_pacfg_new`, size `0x54`. Rust preserves the initial two
zero writes at `0x2010_0410` and `0x2010_0414`, both selector-field writes at
`0x2010_0428`, the two path images at `0x2010_041c` and `0x2010_0420`, and
all four final compensation writes at `0x2010_0410`. The start request is
exactly `(1, 0x80, 0, 0, 0, 0)` and the restoration request is exactly
`(0, 0x80, 0x28, 0, 0, 0)`. No callback-table load, indirect call, hidden
software state, allocation, loop, delay, or status wait remains in this
operation.

RFPLL frequency programming is now a complete Rust-owned transition as well.
The exact rev0 ROM graph is rooted at `phy_set_rf_freq_offset` at
`0x2f82_5c10` and includes `phy_set_rfpll_freq`, `phy_rfpll_set_freq`,
`phy_write_rfpll_sdm`, `phy_restart_cal`, `phy_wait_rfpll_cal_end`,
`phy_read_pll_cap`, `phy_write_pll_cap`, and
`phy_rfpll_cap_init_cal`. `RfpllFrequencyTransition` owns the five-byte SDM
image, all 13 fixed prefix writes, the 100 externally delivered
`20-microsecond + lock-read` attempts, both capacitor reads, every candidate
write, and each external five-microsecond edge. The missed-lock print is
replaced by `lock_observed = false`.

The ROM capacitor search normally shares its offset, sum, and sample count
between downward and upward phases. Rust preserves that order and arithmetic.
If the upward phase misses the ROM's `offset == 10` equality exit, the ROM
can remain hardware-dependent without a software bound. Rust permits ten
additional externally completed upward observations and then reports
`CapacitorSearchDeadlineExceeded`; it never creates an unbounded poll or
self-wake loop. Operation twenty-four therefore has no remaining vendor/ROM
child boundary.

`esp32s31_rev0_rom.elf` supplies the exact symbolized RX-DCO reference:
`phy_pbus_rx_dco_cal` is at `0x2f82_8f44`, size `0x228`.
`PhyRxDcoTransition` now owns its PBus read and four-command setup, each
per-iteration I/Q force, externally completed 10-microsecond interval,
population-dependent threshold, complete `phy_get_dco_comp(1, 0, ...)`
arithmetic, twelve-iteration bound, result words, and success/failure register
restoration. The fixed `phy_pbus_rd(1, 2)` jump-table path is one Rust MMIO
read of the low nine bits at `0x2010_1894`. No ROM RX-DCO parent or
synchronous delay is required by the transition.

The RX-DCO `phy_dc_iq_est` child is now fully decomposed as
`PhyDcIqEstimateTransition`. The exact rev0 ROM references are
`phy_iq_est_enable` at `0x2f82_89d4`, `phy_iq_est_disable` at
`0x2f82_8a88`, `phy_dc_iq_est` at `0x2f82_8ab4`, and `phy_linear_to_db` at
`0x2f82_6542`. Rust owns the setup writes at `0x2010_044c` and
`0x2010_0450`, both one-microsecond timer boundaries, the single-sample
readiness observation at `0x2010_047c`, activity observation at
`0x2010_08d0`, all three signed accumulators, the exact fixed-table power
conversion, and the ordered disable tail.

The ROM readiness spin is not reproduced. Each false readiness observation
must be delivered by an independent hardware/timer edge; it cannot self-wake
or request another sample. The owner may instead deliver a typed timeout.
Both success and timeout clear measurement bit one, cross an external
one-microsecond timer edge, and then clear start bit zero. The hidden
halfword at `phy_param_rom + 0x1ac` has no write in the Rust path: its
diagnostic activity count is an ordinary field in the transition outcome or
failure.

The complete rev0 ROM `phy_get_rx_sig_pwr` body at `0x2f82_9ea2`, size
`0x76`, is now `PhySignalPowerTransition`. For every crystal-duty sample it
owns both clock-enable actions, the asynchronous disable tail for the
previous estimator session, `shift = 12` estimator setup, readiness edge,
and the four signed MMIO reads at `0x2010_0454..=0x2010_0460`. Its stateless
suffix preserves the ROM arithmetic shifts, wrapping 32-bit sum/difference,
signed full-width squares, carry and wrapping 64-bit addition. Success leaves
the estimator enabled exactly as ROM does; the next sample begins by
disabling it. A readiness timeout instead performs the complete disable tail
before exposing a typed failure. Crystal-duty search therefore has no
remaining synchronous signal-power callback.

Operation twenty-five is also complete. The pinned archive body
`libphy.a[phy_reg.o]::phy_fe_reg_update`, size `0x32`, is deliberately used
instead of the similarly named ROM function: the archive call site performs
two fresh-read RMW updates at `0x20100c08`, setting `0x02000000` and then
`0x04000000`, followed by a fresh-read RMW at `0x20100448` setting bits 1:0.
It then returns. The ROM variant has an additional
`phy_dac_scale_set(1)` tail-call which is not present in this pinned cold-init
path and is therefore not invented in Rust.

`phy_freq_reg_init()` belongs to `phy_wakeup_init` and operation twenty-six
`phy_set_chan_freq_hw_init(2, 4)`, not the earlier crystal-duty point. The
prefix therefore enters the channel-frequency transition only after the
front-end update completion edge.

Operation twenty-six is now decomposed from pinned
`libphy.a[phy_hw_freq.o]` and the exact ROM ELF. Its parent calls
`phy_freq_reg_init(2, 4)`, `phy_get_rf_freq_init(0x55, 0)`, and
`phy_freq_i2c_data_write(1)`. The first child is a complete five-store Rust
MMIO leaf. Its hidden `phy_param[0x193]` test is an explicit boolean input:
false retains `(2, 4)`, while true selects the ROM override `(0, 2)`.

The central 85-entry RF table no longer needs an implicit C buffer or a
1,020-byte Rust SRAM mirror. `PhyFrequencyTableTransition` retains only the
two measured PLL-cap endpoints, crystal selector, two crystal-duty bytes,
the upper five bits of PHY-I2C register `0x63:6`, and current entry/word
indices. It computes one exact three-word record for frequencies
`0x960..=0x9b4` using the recovered signed `/ 64` interpolation, the pure
RFPLL SDM arithmetic, and the unsigned boundary behavior of archive
`phy_get_xtal_duty`. Each of the 255 hardware-memory writes is exposed as an
identity-bound action.

ROM `phy_freq_i2c_mem_write` is also a complete Rust MMIO leaf: it replaces
the eleven-bit address at `0x2010001c`, writes the caller-owned mode/data word
at `0x2010002c`, then produces the exact bit-20 write pulse. It has no wait or
busy observation.

The final `phy_freq_i2c_data_write(1)` graph is now Rust-owned too. Its exact
eleven descriptors are generated on demand from three explicitly completed
PHY-I2C snapshots and the former `phy_param[0x1af]` bit. Descriptor kind zero
expands to three frequency-memory writes, kind one to one mode-seven write,
and the remaining kinds to one mode-three write. Rust then packs the eleven
derived number-addresses into the exact register image at
`0x20100030..=0x2010003c`. The corresponding MMIO leaf is one finite
read/modify/write plus three stores.

`PhyChannelFrequencyInitTransition` now composes the entire pinned operation:
the five-register setup, initial capacitor value, RFPLL points `0x985`,
`0x960`, and `0x9a0`, the 85-entry table, and the final descriptor graph.
Every RFPLL delay and observation remains an external completion. The two
redundant vendor capacitor reads after the low and high RFPLL calls are not
repeated: the child transition already owns those exact final values. The
former `phy_param[0xa4] & 0x20` initialized flag is an explicit Rust-owned
boolean. Its warm branch skips table calibration but still refreshes the
descriptor graph, matching the archive parent.

`PhyRfInitPrefixTransition` no longer terminates at
`ReadyForChannelFrequencyInitialization`. It retains the captured crystal
selector and passes the two owned crystal-duty calibration winners directly
as the frequency table's middle and outer duty values. It requests only the
three remaining explicit control fields: register-mode override, table
initialized state, and front-end descriptor bit. Its success outcome now
contains the complete channel-frequency result; an RFPLL deadline is a typed
terminal failure.

The exact analysis sources are
`esp32s31_rev0_rom.elf` SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`
and `libphy.a` SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.
They are analysis oracles only and are not linked into firmware. Host tests
cover cold and warm aggregate paths, all 255 table publications, both dynamic
tail descriptor kinds, exact number-address packing, and out-of-order
completion rejection. The aggregate remains preparatory and dead-stripped
from the qualified hardware image until the Rust cold-init executor replaces
the live `register_chipv7_phy` parent.

## Prepared explicit owner for `phy_param`

The complete initial image of the remaining cold-PHY object is now recovered
and represented by `phy_cold::PhyColdState`. The reference section is
`libphy.a[phy_init.o]::.data.phy_param`: 508 bytes, four-byte alignment, and
section SHA-256
`d8b4dbeeedcfb2cbaa6a00d2a7c84bc8c9ad5bbf54a2ff6bc30dee7f3b46ed83`.
The containing pinned archive has SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.
Its complete initial state contains only these nonzero bytes:

| offset | value | offset | value | offset | value |
|---:|---:|---:|---:|---:|---:|
| `0x002` | `0xbf` | `0x003` | `0x20` | `0x006` | `0x54` |
| `0x00b` | `0x01` | `0x00e` | `0x60` | `0x00f` | `0x01` |
| `0x012` | `0x1f` | `0x013` | `0x16` | `0x014` | `0x01` |
| `0x015` | `0x40` | `0x016` | `0x02` | `0x018` | `0x50` |
| `0x024` | `0x30` | `0x1ab` | `0x01` | `0x1af` | `0x01` |

The type is deliberately neither `Copy` nor `Clone`: moving it transfers the
one software owner of the radio parameter state. It applies the exact
71-byte init-profile mapping, owns the RC-calibration mutation, supplies
typed snapshots for filter-DCAP, crystal-duty and channel-frequency
transitions, and receives all five parameter mutations produced by the
completed `phy_rf_init` prefix. The retained 524-byte calibration record is
a separate fixed-size `PhyCalibrationRecord`; backup, recovery, identity and
checksum processing use only bounded local-memory traversals. No allocator,
callback, MMIO, wait, panic edge, or hidden C state is involved.

`PhyRfColdInit` composes that unique owner with
`PhyRfInitPrefixTransition`. Its local step applies at most one bounded
state-only completion. A hardware, timer, or observation action is returned
unchanged to the outer executor and advances only when an identity-bound
completion is supplied. It contains no future, waker, self-poll, retry loop,
or implicit progress.

The first target primitive for that outer executor is also explicit.
`PhyColdI2cTransaction` separates command start from completion observation.
A byte or masked read uses one external completion edge; a masked write is a
read/modify/write requiring two independent edges. If the target finish leaf
still reports `Busy`, the transaction remains in the same `Await*` state and
returns `StillPending`. It does not spin, register a waker, or ask the
executor to poll it again; only a later peripheral edge or an outer Rust
deadline may cause another observation.

The first executor lowering layer is now concrete rather than only a target
adapter sketch. `PhyColdExternalBinding` admits exactly five owned operation
kinds: PHY-I2C, finite MMIO, sampled MMIO, PBus command, and Rust timer. It
has no vendor callback, synchronous fallback, or generic function-pointer
variant. The bindings and their in-flight PHY-I2C/PBus transactions
deliberately are neither `Copy` nor `Clone`; producing the transition
completion consumes the identity token.

The current lowering covers the direct `phy_rf_init` MMIO operations, command
memory initialization, bias/filter/init writes, RC and charge-pump I2C
operations, SAR2, parameter reads, and the non-RFPLL channel-frequency
I2C/MMIO/memory actions. A masked write retains its outer field identity while
using two distinct read and write completion edges. Direct delays in the
outer, open-I2C, PBus-clear, RC, and charge-pump transitions are represented
by `PhyColdTimerBinding`; expiry is supplied only by the outer Rust executor.
`PhyColdPbusBinding` now covers every force transaction in PBus clear, XTAL
prepare, nested RX-DCO, and XTAL restore. A `Busy` observation preserves the
single awaiting token without polling or arranging a wake; a later
peripheral edge or Rust deadline consumes it as an exact completed or timed
out parent completion. `PhyColdObservationBinding` separately owns the
finite work-mode writes and their one sampled settle-condition bit for both
PBus clear and XTAL restore.

The remaining reachable crystal-duty graph is now lowered as well. Open-I2C
captures the exact `0x2010_d800` cycle epoch after its finite power/pulse
transaction and checks the ROM-compatible inclusive 9,999-cycle deadline
with one later wrapping subtraction. Nested RFPLL/XTAL PHY-I2C operations,
all RFPLL/RX-DCO/DC-IQ/signal-power/restore timer actions, tone and clock
MMIO, estimator control, and PBus restore pulses retain their complete parent
identity. RX-DCO field masking, fixed `phy_pbus_rd(1, 2)`, DC/IQ readiness
and accumulators, and signal-power readiness and accumulators are separate
one-shot sampled observations. A false readiness result does not arrange
another sample; an independent Rust deadline consumes the same binding as a
typed timeout. Unknown register/mask or PBus selector/path tuples fail
closed.

The full two-pass crystal-duty success traversal asserts that every reachable
external action is accepted by `PhyColdExternalBinding`; representative
identity tests cover every operation class and explicit timeout completions.
Both the host library and the `riscv32imafc-unknown-none-elf`
`strict-no-wait,hil-vendor-tx` target configuration compile.

This is not yet the live cold-init implementation. The remaining work is to
connect this completed prefix graph to the Rust cold-init executor, port the
larger `phy_bb_init` calibration suffix, and then port the outer
`register_chipv7_phy` sequencing. Only after that graph is complete will the
HIL publish `PhyColdState` to the temporary ROM ABI and remove the vendor
`phy_param` definition.

## First Rust-owned `phy_bb_init` slices

The next pinned parent is `libphy.a[phy_init.o]::phy_bb_init`, size `0x16a`.
Its complete relocation graph contains 26 direct child-call sites naming 24
unique functions. Exact disassembly establishes two branches:

- `phy_param[0xa4] & 0x08` skips the first eight calibration children; a
  completed first pass sets that same bit;
- nonzero `phy_param[0x196]` requests `phy_wifi_enable_set(0)` before the
  unconditional TX-rate initialization.

The parent begins by setting bit two of `0x2010_0800` and replacing bits 1:0
of `0x2010_0028` with two. It restores the latter field to zero after
`phy_chip_set_chan(11, 0)`. Both operations are now typed finite
`PhyBbMmioAction` transactions. The complete direct call ledger is:

| order | pinned child and arguments | current Rust status |
|---:|---|---|
| 1 | `phy_txdc_cal_init(&phy_param[0xa8], 15, 0, 0)` | calibration transition pending |
| 2 | `phy_pwdet_code_cal()` | complete Rust-owned PBus/timer/SAR transition |
| 3 | `phy_tx_cap_init()` | calibration transition pending |
| 4 | `phy_tsens_temp_read()` | complete Rust-owned PHY-I2C/MMIO transition |
| 5 | `phy_tx_pwctrl_init(0)` | calibration transition pending |
| 6 | `phy_txdc_cal_pwdet_init(1, 0, 0)` | calibration transition pending |
| 7 | `phy_dcode_cal_init()` | complete nested RFPLL/I2C transition |
| 8 | `phy_txiq_cal_init()` | calibration transition pending |
| 9 | `phy_set_tx_cfr_mem(32)` | complete Rust-owned transition |
| 10 | `phy_bt_tx_gain_init()` | calibration transition pending |
| 11 | `phy_set_pbus_mem()` | complete Rust-owned 60-entry transition plus six-word state commit |
| 12 | `phy_tsens_temp_read()` | reuses complete temperature transition |
| 13 | `phy_rxiq_cal_init(0, &phy_param[0xa4], 0)` | calibration transition pending |
| 14 | `phy_rx_table_init()` | complete Rust-owned state plus finite MMIO |
| 15 | `phy_rfrx_sat_rst(0)` | complete finite Rust MMIO |
| 16 | `phy_check_rx_sat()` | Rust-owned bounded async polling transition and one-shot MMIO binding complete |
| 17 | `phy_set_rx_gain_table(0x985, 0)` | RX gain transition pending |
| 18 | `phy_rfrx_sat_rst(1)` | complete finite Rust MMIO |
| 19 | `phy_reg_init()` | complete composed finite Rust MMIO |
| 20 | `phy_bb_agc_reg_update()` | complete finite Rust MMIO |
| 21 | `phy_reg_update_new()` | existing complete finite Rust MMIO |
| 22 | `phy_enable_agc()` | complete finite Rust MMIO |
| 23 | `phy_chip_set_chan(11, 0)` | cold-state channel transition pending |
| 24 | conditional `phy_wifi_enable_set(0)` | complete finite Rust MMIO |
| 25 | `phy_i2c_txrate_init()` | complete Rust MMIO; indirect slot removed |
| 26 | tail `phy_bb_txpwr_track(1)` | complete finite Rust MMIO |

`phy_set_tx_cfr_mem(32)` is now `PhyTxCfrTransition`. It reads the high byte
of `0x2010_0408` exactly once, retains that byte in Rust state, then exposes
32 separately completed entry publications. Entries zero through nine use
data word `0x0e13`; the remainder use zero. Each entry updates the eight-bit
wrapping address field in `0x2010_0844`, writes `0x2010_0848`, and performs
the exact commit-bit pulse. A non-cloneable `PhyTxCfrMmioBinding` consumes
one external operation identity; there is no loop in the target leaf,
callback, delay, wait, or fallback.

The complete ROM `phy_bb_agc_reg_update`, `phy_enable_agc`, and
`phy_wifi_enable_set` bodies plus archive `phy_bb_txpwr_track` are now direct
Rust register transactions as well. Their raw MMIO remains isolated in
`radio_hal`; `phy_bb` contains the ownership, sequencing vocabulary, and
pure TX-CFR address transform. No unported child is represented by a generic
vendor-call action.

Both calls to complete ROM `phy_rfrx_sat_rst` are typed as prepare and
finalize phases and retain the branch-specific fresh-read register order.
Complete ROM `phy_i2c_txrate_init` is direct as well: Rust reproduces its two
register-field writes and the full `phy_txgain_comp_pacfg_new(1)` tail rather
than dispatching through `g_phyFuns+0x30`.

Two shared descendants needed by the pending RX-table graph are now complete
typed Rust MMIO leaves. ROM `phy_write_gain_mem` writes the three caller-owned
words to `0x2010_0848..=0x2010_0850`, then replaces the low 20 bits of
`0x2010_0844` with the explicit entry index and write bit. ROM
`phy_iq_corr_enable` is the finite two-register update at `0x2010_0438` and
`0x2010_0c0c`. This does not yet mark either `phy_rx_table_init` or
`phy_set_rx_gain_table` complete: their table generation and remaining
descendants stay outside the lowering.

The complete `phy_reg_init` call graph is now one typed Rust action consuming
only the explicit former `phy_param[0x121]` and `phy_param[0x120]` inputs.
It includes `phy_agc_reg_init`, `phy_wifi_agc_sat_gain`, `phy_bb_reg_init` and
its `phy_btbb_wifi_bb_cfg2` tail, `phy_bb_wdg_cfg`, `phy_tx_paon_set`, both
branches of `phy_rx_11b_opt`, the full `phy_tx_pwctrl_bg_init` →
`phy_en_pwdet` → `phy_pwdet_sar2_init` graph, `phy_noise_floor_auto_set`,
`phy_ant_init`, `phy_bt_filter_reg`, and the `phy_mac_enable_bb` tail. Every
reachable body is a fixed register sequence with no allocation, wait,
hardware-dependent exit, ROM-owned RAM, or indirect dispatch.

`phy_rx_table_init` is complete on top of that graph. Its sole software-state
write, `phy_param[0x120] = 0x4f`, is now a method on the unique
`PhyColdState`; the same step captures explicit bytes `0x002` and `0x121`.
The target action derives and publishes exactly 79 typed gain-memory entries,
then runs the recovered `phy_reg_init`, AGC-update and AGC-enable suffix. No
raw parameter pointer, ROM call, polling exit, allocation, or hidden global
mutation remains in this child.

`phy_check_rx_sat` is now represented by a caller-driven Rust transition.
The recovered archive body enters PBus debug mode, publishes eleven exact
PBus commands, blocks for five microseconds, then polls
`0x2010_08d0[21:20]` exactly 100 times. The replacement exposes the delay as
an async timer completion and each register read as a separate identity-bound
action. It always requests PBus work-mode restoration before returning
success or failure. The unique `PhyColdState` captures the only input, former
`phy_param[0x002]`, and owns the only persistent effect: a nonzero sample
count sets byte `0x1ae` to one; a zero result never clears it and failed
operations cannot mutate it.

No dedicated completion interrupt for `0x2010_08d0[21:20]` is visible in the
available S31 PAC/SVD or ROM symbols. The retained polling is therefore the
radio contract, but it is no longer a blob or executor spin loop: a
non-cloneable target binding performs exactly one volatile read, and the Rust
state machine issues at most 100 such samples. The executor may yield or arm
an async timer between samples. Hardware-dependent open loops elsewhere must
likewise gain a finite count or deadline before activation.

`phy_pwdet_code_cal` is now a Rust-owned transition as well. The 76-byte root,
118-byte `phy_pwdet_ref_code` child, power conversion graph, debug/work-mode
wrappers and fixed PBus helpers have been reduced to explicit inputs,
identity-bound actions and two signed outputs. The entry path publishes 15
exact PBus transactions, the exit path publishes seven, and all one- and
two-microsecond delays are external Rust async timer edges. Four SAR samples
replace the ROM stack buffer; each sample is extracted from
`0x2010_081c[29:17]`, and the exact unsigned threshold plus
`phy_linear_to_db` arithmetic is pure Rust.

The readiness condition `0x2010_080c[16:14] == 7` is intentionally still
polled. No interrupt source for this field has been proved, so deleting the
poll would invent a hardware contract. The replacement performs one volatile
read per non-cloneable `PhyPwdetReadyBinding`; a false sample returns to the
same state and only an outer executor may schedule another sample. That
executor also owns a finite async deadline. Deadline expiry runs the full
tone-stop, TX-clock-disable and PBus work-mode restoration path before
reporting failure.

Former parameter inputs at `0x002`, `0x012`, `0x0a8..=0x0af`, `0x1aa` and
`0x01a..=0x01d` are captured by `PhyColdState::pwdet_parameters`. Only a
successful outcome may update the two reference codes and set bit 24 of the
word at `0x0a4` (byte `0x0a7` bit zero). The ROM `phy_param` pointer and
`g_phyFuns` callback table are absent from this child.

This baseband work is still preparatory and dead-stripped from the qualified
image. Activation is deliberately deferred until every reachable child has
an explicit lowering and the complete parent can reject unknown or
out-of-order completions without a vendor escape.

All 456 host tests pass. The target
`riscv32imafc-unknown-none-elf` `strict-no-wait,hil-vendor-tx` configuration
also compiles. The last qualified target strict audit covers 6,407
functions with zero violations and reports runtime ownership debt of one
explicit RX fallback, zero stateful/unproven runtime roots, and zero
temporary MMIO roots. The corrected linked-state audit expands that fallback
to 11 archive definitions / 3,236 bytes, four mutable blob objects / 1,412
bytes, and three ROM-ABI cells. The cold-PHY graph still reports exactly one
live mutable blob symbol, `phy_param`, of 508 bytes, because the prepared owner
is dead-stripped until activation. The generated application image from the
published dependency chain is 894,352 bytes.

## In-progress slice: `g_ic`

The linked-state audit reports the complete 788-byte `g_ic` object because ELF
symbols do not describe fields. This is intentionally conservative; it does
not mean that all 788 bytes are needed by the strict runtime. Relocation and
instruction inspection of the original three strict vendor referrers, plus
the event-5 consumer called directly by Rust, gives the narrower graph:

| vendor leaf | `g_ic` fields read | current purpose | status |
|---|---|---|---|
| `ieee80211_set_tx_desc` | `0x10`, `0x14` | identify STA versus AP interface | interface registry ready; leaf remains |
| `ieee80211_hostapd_data_txcb` | `0x14`, `0x74` | find AP state and enter mesh-only activity update | replaced by exact non-mesh Rust no-op |
| `ieee80211_post_hmac_tx` | `0x258` | select optional cached-TX path | replaced; ordinary STA/AP queue publication is Rust |
| `ieee80211_output_process` | `0x1ac`, `0x1b0` | optional pending-frame queue | one-frame compatibility stage; classifier, CCMP key/header, and ESF alignment leaves replaced; consumer remains |

The reference objects are pinned
`libnet80211.a[ieee80211_output.o]` and
`libnet80211.a[ieee80211_hostap.o]`. The offsets above are instruction
operands or relocation addends, not inferred names.

Rust code currently touches additional `g_ic` fields because it already
reproduces finite pieces of vendor STA/AP behavior. They must be migrated by
meaning rather than collected into a Rust byte-for-byte `g_ic` clone:

- interface publications at `0x10` and `0x14`;
- mesh-mode gate at `0x74`;
- software-key pointer slots beginning at `0x148`;
- AP TIM state beginning at `0x1b6`;
- lifecycle/promiscuous state at `0x1f5` and `0x1f7`;
- crypto gate and AP/STA MAC addresses at `0x210`, `0x214`, and `0x21a`;
- configuration-dirty state at `0x226`;
- cached-TX policy at `0x258`;
- STA authorization state at `0x274`;
- protocol selection at `0x2be` and `0x2c0`;
- RX-policy selector at `0x2cc`.

The first implementation step is complete: an interface registry adopts the
STA/AP publications during cold handoff and exposes role-checked handles, not
raw `g_ic` offsets. STA link, WPA2 STA/AP node lookup, AP TIM handling, and the
strict AP beacon completion now obtain interface identities from this
registry. The pre-handoff AP-start probe deliberately retains its separate
cold read because the registry is not published yet.

The same handoff rejects active mesh state, a non-empty `g_ic+0x1ac`
pending-frame queue, and the vendor cached-TX mode.
Those are immutable strict-profile invariants, not flags polled on each
runtime operation. Under the non-mesh invariant the pinned
`ieee80211_hostapd_data_txcb` returns before reading its frame, so the strict
TX callback table now installs the exact Rust no-op instead. This removes that
function as a strict vendor root.

Node tables and interface contents remain separate owners: publishing an
interface does not grant arbitrary mutable access to every field behind its
pointer. Cold adoption now also copies each present interface's six-byte MAC
address into 16 bytes of aligned atomic SRAM storage; the live encapsulator
does not call `wifi_get_macaddr` or inspect a vendor interface field. This was
verified on S31 hardware through passive scan, WPA2 association, DHCP, ping,
DNS, TCP, and post-link data with zero recorded allocations. The final ELF
still passes the strict no-wait/no-heap audit with zero violations.

The Rust replacement validates the descriptor interface, strict-hart and
radio-owner identity, and home-channel state, appends through the recovered
`frame+0x30` intrusive link, and reserves the sole PP event-5 token. Event
publication occurs only on the empty-to-non-empty transition. It contains no
allocation, wait, retry, indirect call, or `g_ic` access. The persistent input
queue and its event-token state are now one explicit Rust object in internal
SRAM.

Event 5 removes exactly one frame from that queue and runs the recovered
ordinary STA/AP path directly. It resolves the bounded peer, copies the
Ethernet header into an owned local value, rejects raw/WAPI/HE-prefix,
NAN/mesh, off-channel, and AP power-save cases, then applies the pure
address/LLC/QoS plan. The live target adapter advances the per-TID sequence,
invokes only the separately interposed classifier, CCMP, alignment,
descriptor, and PTI leaves, and enters `ppTxPkt` without publishing a vendor
mailbox. Another Rust event token is reserved only when another frame remains
and no nested publication already reserved it.

Completion ownership is no longer inherited from recycled descriptor state.
The recovered `ieee80211_output_process` branch is an explicit pure rule:
STA EAPOL (`0x888e`) gets callback bit 3, ordinary STA data gets zero, and AP
traffic gets the encapsulator's callback bit 12. Host tests cover the complete
STA/AP matrix. Hardware validation observed M2 and M4 through the Rust
completion path, completed authorization, DHCP, ping, DNS, TCP, and HTTP, then
passed the 4096-datagram/four-HTTP strict stress workload with all 4786 TX
credits returned, empty queues, no rejected event posts, and unchanged
post-handoff allocation counters.

The next leaf inside that compatibility stage is now Rust-owned.
`ieee80211_classify` was recovered from the pinned
`libnet80211.a[ieee80211_output.o]`: EAPOL and WAPI select the fixed-rate bit
and priority 7; STA ARP and DHCP/DNS select the same fixed-rate policy; IPv4
DSCP and the IPv6 traffic class select user priority; multicast or a non-QoS
node use priority 7; and WMM admission control follows the recovered monotonic
four-state downgrade graph. The Rust implementation writes descriptor bit
`0x0200_0000` directly rather than calling the PP/TRC helper and bounds the
four-state admission graph to three transitions.

ESP32-S31 ROM does not reach this leaf through the exported symbol. JTAG and
ROM disassembly establish that `ieee80211_output_process` calls
`net80211_funcs+0x24`. Strict handoff therefore validates that slot against
the pinned vendor address or the Rust replacement, writes the replacement,
and reads it back. GNU wrapping remains mandatory for direct archive
references. A JTAG snapshot of the running image showed the slot equal to
`__wrap_ieee80211_classify`; the same image completed passive scan, WPA2
association, DHCP, ping, DNS, TCP, and HTTP with zero allocation counters.

The WPA2-CCMP security-selection leaf is now Rust-owned as well. Its reference
is the pinned `libnet80211.a[ieee80211_crypto.o]` and
`libnet80211.a[ieee80211_crypto_ccmp.o]` pair. Descriptor bit 1 selects the
group hardware-key index at `node+0x135`; otherwise the pairwise index at
`node+0x134` is used. The Rust boundary resolves that index only through the
fixed `STATIC_VENDOR_KEY_SLOTS` registry, validates the pinned CCMP object and
16-byte key length, advances the key object's 48-bit TX packet number by the
recovered value three, and inserts the exact eight-byte CCMP header. It neither
reads the vendor software-key pointer array at `g_ic+0x148` nor dispatches
through the cipher object at offset `+0x10`.

The ESP32-S31 runtime reaches this leaf through `net80211_funcs+0x44`. Strict
handoff adopts and reads back that slot exactly as it does the classifier
slot. The public `ieee80211_crypto_encap` name is an absolute ROM export at
`0x2f800cac`, so GNU wrapping is not used: the final linker fragment aliases
the public name to the uniquely named Rust boundary and retains the pinned ROM
address only for pre-strict cold-init delegation. The strict ELF audit proves
both the alias and the callback-table adoption contract. Hardware verification
completed passive scan, WPA2 association, DHCP, ping, DNS, TCP, and HTTP with
zero allocations after this replacement.

The subsequent `ieee80211_align_eb` leaf is also an exact finite Rust port.
The pinned `libnet80211.a[ieee80211_output.o]` implementation reserves the
802.11 header, moves the MPDU down by the resulting zero-to-three-byte
alignment delta, and encodes the total length into bits 27:14 of the ESF
storage word. The Rust policy admits only the ordinary STA/AP 24-byte legacy
or 26-byte QoS headers, requires the caller's reservation to equal that header
length, checks every subtraction and the 14-bit total length before mutation,
then commits the data pointer and packed storage word. It performs no
allocation, wait, retry, global-state read, or indirect callback.

As with the CCMP leaf, `ieee80211_align_eb` is an absolute ESP32-S31 ROM export
(`0x2f800c7c`). The final linker fragment aliases it directly to the unique
Rust boundary and retains the ROM address only for pre-strict delegation.
The final ELF records both addresses explicitly, and the hardware image passed
WPA2 plus post-link traffic without entering the invalid-layout trap.

The ordinary non-HE part of `ieee80211_set_tx_desc` has now been recovered
from the pinned `libnet80211.a[ieee80211_output.o]` oracle as a pure Rust
policy plus a target adapter. The pure policy reproduces the eight-priority
WMM queue mapping, STA/AP rate-context selector, descriptor flag and security
masks, bounded opaque node-bit transforms, TWT record selection, and the
remaining finite descriptor bytes. Request bit `0x08` is used by strict STA
data TX, while `0x10` was observed during strict AP cold-start management TX.
The pinned leaf branches only on `0x08` and otherwise ORs both qualified bits
into the descriptor. HE descriptor bit 31, priorities above seven, and every
other request bit trap before the first mutation.

Strict handoff now adopts two additional scalar inputs: the initialized
configuration byte formerly read through `g_wifi_nvs+0x44a`, and
`g_itwt_fid`, which is rejected above seven. This is a transitional one-time
cold-state read, not an NVS call: the strict descriptor path reads only the
Rust registry. A later cold-initialization slice should construct both values
directly from Rust configuration and remove the vendor publications entirely.

The public `ieee80211_set_tx_desc` name is another absolute ROM export
(`0x2f800c98`). GNU `--wrap` cannot interpose it reliably because the ROM
linker fragment captures the generated `__wrap_*` name. The late linker
fragment therefore retains the ROM address as
`__real_ieee80211_set_tx_desc`, aliases the public name to the unique Rust
entrypoint, and has a final-value `ASSERT` for the alias. Equivalent assertions
now protect the existing post-HMAC, CCMP, and ESF-alignment ROM aliases. Runtime
Rust pointer comparisons are intentionally not used as link proofs because
LLVM does not model linker-script aliases.

This alias immediately replaces direct management-frame calls from Rust.
The STA HIL completed passive scan, open authentication, HT20/WMM association,
the Rust WPA2 four-way handshake, DHCP, gateway ping, DNS, TCP, and HTTP.
Allocation counters were unchanged across strict authentication and
association, all 19 post-link TX frames released their static credits, and
the one-shot critical snapshot reported zero other-core stalls and zero
wrong-hart entries.

The Ethernet-to-802.11 geometry adjacent to the descriptor leaf is now the
live data encapsulator: STA/AP address selection, RFC 1042 LLC/SNAP,
QoS/no-ack policy, multicast handling, sequence wrap, callback ownership, and
the priority byte are bounded and allocation-free. The final-ELF auditor
rejects any direct call to `ieee80211_output_process`; its absolute ROM symbol
may remain linked for cold/vendor compatibility but is not callable by the
strict image.

The outer `libpp.a[pp.o]::ppTxPkt` shell is now Rust-owned. The pinned RV32
disassembly supplies the exact interface selector, priority-to-hardware-queue
table, MAC-time register read, and `pTxRx` tail-link offsets. In the armed
profile Rust sequences the existing protocol, security and rate leaves,
applies the finite observed mapper table, and publishes the frame. It does not
call `ic_interface_enabled`, `lmacIsIdle`, `ppMapTxQueue`, or the cached-HMAC
queue consumer.

The strict AP cold-start run additionally qualified the retained beacon mapper
class: frame control `0x0080`, legacy rate `12`, descriptor flags
`0x0080_0412`, AP selector `0x0004_0000`, peer state `0x83`, and descriptor
byte four `0x07`. It is an existing bounded beacon layout already shared by
the Rust security and completion policies; the mapper preserves byte four and
does not enter aggregation or power-save search state.

The same AP run qualified the direct-LMAC beacon completion rather than
borrowing a descriptor state from an older vendor path. Hardware completed
the 204-byte frame as lengths `0x00ac_0020`, layout `0x2000`, buffer word
`0xc033_02f8`, descriptor flags `0x0080_0412`, and descriptor-security word
`0x0104_0000`. The pinned `ppProcTxDone` persistent-object branch proves the
inverse operation: remove four bytes of trailer accounting, remove the
one-transmission eight-byte PP prefix, clear layout bit `0x2000`, and clear
descriptor ownership bit `0x0080_0000`. Rust admits that exact hardware status
in addition to the previously measured vendor completion variants, validates
the encoded length and fixed 1600-byte management bound before mutation, and
returns the beacon to its `0x0004_0000` base selector for reuse.

Longer AP operation also observed the same persistent 204-byte beacon through
the direct-LMAC ACK-timeout completion with descriptor-security word
`0x0404_0000`. The pinned vendor completion restores persistent geometry
before branching on later callback/recycle policy, so Rust accepts this exact
status variant with the same length and ownership checks. A missed beacon ACK
therefore no longer terminates the radio owner.

With the beacon visible, an external active scan also supplied the adjacent AP
probe-response mapper class: frame control `0x0050`, rate `12`, layout
`0x2003`, descriptor flags `0x0800_0010`, priority `7`, AP selector
`0x0004_0000`, and peer state `0x83`. The mapper admits only that complete
tuple; changing the peer state or any descriptor role still fails closed.

The first laptop join then qualified the plaintext AP authentication-response
mapper tuple: frame control `0x00b0`, rate `12`, layout `0x2730`, zero
descriptor flags, priority `7`, selector `0x0004_0000`, and peer state `0x83`.
As with the probe response, the low layout bits are a fixed-slot identity;
only the already-proven upper `0x2000` headroom state affects mapping.

After that response, the laptop join reached the AP association-response
mapper with frame control `0x0010`, rate `11`, layout `0x2731`, zero descriptor
flags, priority `7`, selector `0x0004_0000`, peer word `0x2100_0000`, and
peer flag `1`. The nonzero flag is admitted only as part of this complete
post-association tuple; all earlier mapper classes retain their zero-flag
requirement.

An independently connected Android station then reached the same association
response with layout `0x2f31` and peer byte `0x84` equal to `2`; every other
mapper input was unchanged. The mapper now models the two hardware-observed
one-based AP connection identities explicitly for association response,
message one, ADDBA response, and pairwise HT-QoS data. Values zero and three
remain rejected, and broadcast/group pseudo-peers retain their separate exact
states.

The next join edge reached WPA2 message one with frame control `0x0288`, rate
`11`, layout `0x2000`, descriptor flags `0x0200_200c`, priority `7`, selector
`0x0004_0000`, peer word `0x2100_0000`, and peer flag `1`. This is the exact
post-association AP EAPOL mapper tuple; it reuses the already-qualified
plaintext AP EAPOL security layout without admitting other data frames.

Unknown mapper input now records all eight words of the finite decision input
in a fixed `.critical.bss` SRAM object before executing the fail-closed trap.
The detail word is release-published last, so the terminal panic path can
print a coherent record with direct ROM output and without allocating,
locking, waiting, accessing PSRAM, or relying on stack-heavy trap-frame
formatting. This is qualification instrumentation only: it does not widen the
accepted mapper domain or provide a vendor fallback.

That record exposed the first post-authorization AP network frame as an exact
group-CCMP mapper tuple: frame control `0x4208`, rate `12`, layout `0x2000`,
descriptor flags `0x0000_200b`, priority `7`, security/control word
`0x0004_0342`, AP peer state `0x83`, and peer flag `0`. The adjacent Rust
security leaf had already qualified this descriptor and expanded its CCMP
headroom. The mapper initially admitted only this complete observed state;
pairwise/QoS data remained separately fail-closed.

The Android WPA2 join later emitted the same group-CCMP class from static-slot
layout `0x2003` with descriptor flags `0x0200_200b`. The additional
`0x0200_0000` fixed-per-packet-rate bit is already a bounded input in the adjacent
security and completion leaves, so the mapper now admits the two observed
group descriptor words explicitly. An unrelated `0x0100_0000` bit remains
rejected.

The first completion of that frame retained the same validated `0x0020:0x0068`
lengths, `0x2000` layout, `0xc022_0082` buffer equation, and callback bit 12,
while hardware returned group-key status `0x0104_0342`. The AP power-save
callback policy now accepts that exact status alongside the two previously
measured group-CCMP outcomes. It remains a no-op only after the complete
bounded geometry is checked; nearby selector/status values are rejected.

Associating a second WPA2 station while the first Android peer remained active
later returned group-key status `0x0404_0342` for the same frame control
`0x4208`, lengths `0x0020:0x0068`, changing static-slot layout `0x202a`,
buffer equation `0xc022_0082`, descriptor flags `0x0000_200b`, and callback
bit 12. That complete measured outcome is now admitted alongside the older
group completions; `0x0405_0342` and any change to the bounded geometry remain
fail-closed.

After DHCP made the AP network ready, the client requested RX aggregation.
The existing bounded Rust ADDBA response reached the mapper as frame control
`0x00d0`, rate `11`, layout `0x2732`, zero descriptor flags, priority `7`,
selector `0x0004_0000`, peer word `0x2100_0000`, and peer flag `1`. This exact
post-association Action tuple is now distinct from the older STA Action class;
pre-association and zero-peer-flag variants still fail closed.

Its first acknowledged completion preserved frame control `0x00d0`, lengths
`0x0020:0x000d`, layout `0x2732`, buffer equation `0xc00b_402c`, zero
descriptor flags, and callback mask `0x0000_2004`, while hardware returned
descriptor-security `0x0104_0000` with status byte `1`. That exact successful
pair is admitted in addition to the two older measured ADDBA outcomes; changing
either the status or any structural field remains rejected.

The first protected pairwise AP downlink then reached the mapper as QoS data
frame control `0x4288`, HT rate `33`, layout `0x2000`, descriptor flags
`0x0000_2009`, fresh priority byte `0x20`, pairwise selector
`0x0004_0348`, associated-peer word `0x2100_0000`, and peer flag `1`. The
finite mapper admits only this complete measured tuple and rewrites descriptor
byte four to the recovered treatment `7`; the group-key selector, an already
mapped priority, and a pre-association peer remain independently rejected.

An Android station later exposed the adjacent fixed-per-packet-rate form of
that first pairwise downlink: frame control `0x4288`, internal rate code `11`,
layout `0x2002`,
descriptor flags `0x0200_2009`, priority `7`, pairwise selector
`0x0004_0348`, associated-peer word `0x2100_0000`, and peer flag `1`. It is
admitted as a separate complete mapper class for the two already-qualified AP
peer identities. The unrelated `0x0100_0000` flag and mixing priority `0x20`
into this fixed-per-packet-rate class remain rejected.

When a second WPA2 station joined while Android was in power-save state, an
ordinary net80211 event encountered the associated node's measured sleep bit.
Unicast ownership now stays in the radio command until the peer-bound
Active/PS-Poll/removal future reports an edge; no status loop, delay, vendor PS
queue, or RTOS primitive is entered. Group ownership instead crosses the ESF
boundary immediately into a 16-frame fixed Rust queue with an eight-frame
per-peer/pseudo-peer bound. That distinction is required because group
traffic has no `ff:ff:ff:ff:ff:ff` active edge and one retained radio command
would serialize delivery to one frame per DTIM.

The pinned `ieee80211_hostap_send_beacon_process` computes DTIM count as
`period - 1 - ((tsf / beacon_interval_us) % period)`. Its ROM
`hal_get_tsf_time` source returned zero in the adopted runtime, so the stock
builder emitted count one continuously. The Rust submit boundary now stamps
timestamp, DTIM count, and TIM bitmap-control bit zero from the executor's
monotonic clock immediately before hardware ownership. Beacon TX-done only
observes the value that was actually transmitted and publishes one async group
DTIM epoch when count is zero.

The pinned `pwrsave_flushq` established the remaining air contract: it sets
802.11 More Data (`frame_control` bit `0x2000`) on every non-final retained
MPDU and submits the already prepared FIFO directly. The Rust continuation
reproduces that bit before copying the header into aligned ESF storage, clears
the multicast TIM bit on the final element, and advances at most one ESF per
executor event while a persistent DTIM edge exposes the rest of the bounded
FIFO. HIL measured the resulting non-final mapper tuple as frame control
`0x6208`, rate 12, layout `0x2029`, flags `0x0000_200b`, priority 7, group
selector `0x0004_0342`, and pseudo-peer state `0x83`; only this More Data
variant of the existing group-CCMP class was added.

A deterministic async HIL stimulus submitted three eight-frame broadcast
bursts after the laptop entered power save. Sixteen frames took the DTIM
queue, all 25 application TX owners were returned, hardware and software
queues ended empty, and there were no overflow or admission rejects. After a
subsequent ICMP/HTTP run the cumulative result remained balanced at
49/49/49/49 application TX ownership, 20/20 ICMP replies and HTTP 200 while
beacon completions continued. The final ELF audit remained at 24 roots, 6,407
functions, and zero no-wait/no-heap violations.

Unicast Active, PS-Poll, and removal publications are keyed by both station
MAC and the Rust AP association epoch. A foreign frame is therefore unable to
occupy a readiness slot, and a disconnect/reassociation cannot transfer a
one-frame PS-Poll credit to a new session using the same MAC. Event-table
absence is explicitly Pending rather than an edge: bounded-slot replacement
can delay a retained owner until its next real peer event, but can never cause
transmission to a still-sleeping peer. The ordinary ESF queue and the WPA2
command owner also retain the association epoch; removal or replacement makes
each queued owner independently cancellable and recyclable without a drain
loop. Same-MAC reassociation publishes the old-generation removal before the
new nonzero generation becomes visible.

The `/power-save-cancel-test` HIL path then retained one laptop response,
published its synthetic peer-bound removal edge, and returned every application
and hardware TX owner (`12/12/12/12`, hardware `4/4`). The intentionally
cancelled HTTP request timed out without an unauthorized response. A real
laptop disconnect/reconnect immediately afterwards exercised removal plus two
same-MAC association generations; WPA2 authorization recovered, ICMP completed
5/5, HTTP returned 200, and ownership remained balanced at `17/17/17/17`
with hardware `8/8`. The immortal radio owner and beacon clock continued
through both cancellation and reassociation.

The AP association slot now also owns the nonzero 14-bit AID read once from
the already-qualified node during join. Legacy PS-Poll admission parses the
mandatory `0xc000 | AID` Duration/ID field and publishes a one-frame credit
only when MAC, association generation, and AID all match the same Rust slot.
Malformed, zero, stale, and cross-peer AIDs are ignored without a lookup,
retry, callback, or mutation of the deferred queue.

Keeping the first Android peer active while associating the laptop exposed the
second pairwise key selector before security headroom was changed:
frame control `0x4288`, HT rate code `33`, descriptor flags `0x0000_2009`,
priority `0x20`, descriptor control `0x0004_0349`, peer word
`0x2100_0000`, and peer identity `2`. The low control byte is the AP pairwise
hardware key index with the AP direction bit: peer one used `0x48` (slot 8)
and peer two uses `0x49` (slot 9). Security now admits both measured selectors,
while the mapper binds `0x348` only to peer one and `0x349` only to peer two;
slot 10 and crossed peer/selector combinations remain rejected.

The first acknowledged peer-two downlink retained frame control `0x4288`,
lengths `0x0022:0x0038`, layout `0x2000`, buffer equation `0xc016_8052`,
descriptor flags `0x0000_3009`, and callback bit 12, while hardware returned
status `0x0104_0349`. Only that measured slot-nine completion is admitted;
the adjacent `0x0114_0349` retry outcome remains rejected until observed.

Concurrent Android ICMP and AP-to-laptop TCP later exercised a maximum-MTU
peer-two completion: frame control `0x4288`, lengths `0x0022:0x05ea`,
changing static-slot layout `0x2ad0`, exact buffer equation `0xc183_0604`,
descriptor flags `0x0000_3009`, callback bit 12, and pairwise hardware status
`0x0404_0349`. The slot-nine completion is admitted only with the same bounded
pairwise geometry; adjacent `0x0405_0349` remains rejected.

The repeated two-client run then completed 4 MiB and 16 MiB AP-to-laptop TCP
streams while the Android peer continuously exchanged ICMP traffic. The
16 MiB stream completed in 22.526 seconds with no changed payload bytes;
all `15_222` committed hardware TX credits were returned, the software and
hardware TX queues were empty, and both stations subsequently completed
20/20 ICMP probes. The final strict ELF audit still reported 24 roots, 6,407
discovered functions, and zero no-wait/no-heap violations.

Its first successful completion retained frame control `0x4288`, lengths
`0x0022:0x0038`, layout `0x2000`, buffer equation `0xc016_8052`,
descriptor flags `0x0000_3009`, and callback bit 12, while hardware returned
pairwise status `0x0104_0348`. The AP completion leaf now admits that measured
status in addition to the previously qualified retry/rate-control outcomes;
the complete geometry is still checked before the callback becomes a no-op,
and an adjacent `0x0105_0348` selector remains rejected.

Sustained AP-to-station TCP then exercised a maximum-MTU retry completion:
frame control `0x4a88`, lengths `0x0022:0x05ea`, changing static-slot layout
`0x2c71`, exact buffer equation `0xc183_0604`, descriptor flags
`0x0000_2109`, callback bit 12, and pairwise hardware status `0x0204_0348`.
That status is admitted beside the previously measured `0x0214/0x02a4`
outcomes only after the same bounded geometry checks; `0x0205_0348` remains
rejected.

A repeated maximum-MTU AP downlink later completed without the retry bit,
using layout `0x27ac`, descriptor flags `0x0000_3009`, and hardware status
`0x0404_0348`; its lengths and `0xc183_0604` buffer equation were unchanged.
This outcome is also admitted as an exact pairwise status, while the adjacent
`0x0405_0348` selector is rejected.

The first hardware run with this shell completed the full STA workload:
passive scan, HT20/WMM association, WPA2, DHCP, ping, DNS, TCP, HTTP, ADDBA,
and 4,096 UDP datagrams. It released 4,786 of 4,786 TX frames and 690 of 690 RX
frames, drained 21,284 of 21,284 PP events with no rejects, and preserved the
post-handoff allocation snapshot. This also established that the observed
`0x2000`, `0x2008`, and `0x2010` ESF layout values are static-slot identities:
only bit `0x2000` is consumed by the pinned mapper.

The TX scheduler slice is now separated from the 1,044-byte `pTxRx` object.
Strict handoff proves all sixteen vendor logical queues empty and idle,
validates each empty intrusive tail link, and copies only four initialized
hardware masks and four rotation cursors. From that point producer append,
selection, dequeue, error rollback, and timeout-chain requeue operate on one
fixed SRAM `StrictTxQueueState`. The last runtime `ppDequeueTxQ` call is gone.
A full hardware stress run released 4,787/4,787 TX frames and 692/692 RX
frames, drained 21,371 PP events without rejection, and changed no allocation
counter.

The TX-done registration/list slice has now moved as well. Handoff rejects a
non-empty vendor completion queue or an invalid empty tail link, then copies
the two callback masks and six strict-profile callback identities into one
fixed SRAM `StrictTxDoneRegistry`. Completion append/dequeue and callback
filtering no longer touch `pTxRx`. Hardware qualification completed WPA2,
ADDBA, the full network workload, 4,096 UDP datagrams, and 4 HTTP transfers
with balanced TX/RX/PP ownership and an unchanged allocation snapshot.

The two observed per-logical-queue PPDU-format bytes now move with the
scheduler state. Their bits are not assigned speculative names: they remain
opaque adopted PLCP length/data inputs until a register-level meaning is
proven. The LMAC TX path consequently has no post-handoff `pTxRx` access.

The RX callback registry is now Rust-owned too. Handoff copies only the three
words used by the pinned `ppRxPkt` router: STA, AP, and NAN callbacks at
`pTxRx+0x3f8/+0x3fc/+0x400`. It rejects an AP callback other than
`ap_rx_cb` and rejects any NAN callback. Runtime RX routing and A-MPDU gap
expiry read the immutable SRAM registry and never dereference `pTxRx`.

The queue layout comes directly from pinned `libpp.a[pp.o]` disassembly:
`ppEnqueueRxq` clears `packet+0x30`, appends through the tail-link stored at
`pTxRx+0x398`, then points that slot at the new link; dequeue reads the head
at `+0x394`, advances through `packet+0x30`, and restores the empty tail-link
invariant. Neither leaf locks, waits, or retries; `Locked` documents an
external serialization requirement. `lmacRxDone` is the interrupt-side
producer and immediately publishes PP event 17.

Hardware verification after callback adoption completed passive scan, WPA2,
DHCP, ping, DNS, TCP, HTTP, ADDBA, 4,096 UDP datagrams and 4 HTTP transfers.
It released 4,786/4,786 TX and 692/692 RX owners, drained 21,295/21,295 PP
events, changed no allocation counter, and measured 20.931 Mbit/s.

The interrupt-to-executor RX queue is now Rust-owned too. Pinned
`libpp.a[wdev.o]::wdev_funcs_init` stores the ROM `lmacRxDone` address in the
mutable `pp_wdev_funcs+0x1dc` slot. Handoff masks only local interrupts,
requires the vendor RX queue to be empty with its canonical tail link, and
redirects that slot to `wifi_strict_lmac_rx_done`. The replacement appends
through `packet+0x30` into a fixed internal-SRAM queue; executor dequeue uses
the same queue and the final ELF forbids calls to both `lmacRxDone` and
`ppDequeueRxq_Locked`.

RX readiness is durable queue state rather than a fallible continuation
message. The empty-to-non-empty edge wakes the Rust executor, while
`RadioFuture` checks RX directly as a third round-robin source beside vendor
and internal events. Thus a full finite event queue cannot strand an RX
packet, and a continuously ready RX source cannot starve the other two.
The earlier USB-JTAG observation found the registered Embassy waker vtable at
`0x4000bc5c` and its wake target at `0x400b556e`, both in flash; the task data
was in SRAM at `0x2f06ab00`. Disassembly identified the target as
`embassy_executor::raw::waker::wake`: it atomically marked the task runnable,
pushed it into the shared executor transfer stack with a CAS retry, then
called the SRAM `__pender`.

The strict STA HIL now replaces that boundary with a fixed one-owner executor.
`RadioOwnerFuture` is initialized once in SRAM. Its custom `RawWaker` vtable
and clone/wake/wake-by-ref/drop leaves are also in SRAM, and wake performs only
the bounded S31 `FROM_CPU_INTR2` register write and readback. The final-image
auditor reads the vtable from
`.critical.data.wifi_strict.radio_executor`, resolves all four entries to
their exact SRAM symbols, and requires the software-interrupt entry itself in
SRAM. Consequently the hard RX ISR no longer enters the shared Embassy run
queue or any CAS retry. The first hardware stress run passed WPA2, 4,096 UDP
datagrams and four HTTP transfers with balanced TX/RX/PP ownership, no rejects
or allocation delta, and 27.477 Mbit/s.

The cache boundary now has explicit Rust ownership too. S31 exposes no
hardware cache-enable status bit, so the upstream ESP-IDF cache HAL maintains
that state in software. One internal-SRAM atomic byte records whether cached
execution is available, whether the radio future currently owns the poll
lease, whether readiness was deferred, and whether the immortal future
terminated. The SRAM `try_suspend` leaf closes cached execution only when no
poll is active and otherwise fails immediately; callers must retry
asynchronously. The SRAM `resume` leaf reopens execution and re-pends one
software interrupt for durable deferred readiness. The interrupt acquires the
poll lease with one AMO, never waits, and does not dereference cached future
state while the gate is closed.

The final-image audit requires both cache leaves, every waker target, and the
software-interrupt entry in SRAM. A hardware run that deliberately began with
the gate closed passed the complete WPA2/network stress workload with
4,786/4,786 TX, 691/691 RX, 20,598/20,598 PP events, no allocation delta, and
25.801 Mbit/s. It verifies deferred startup and normal reopening, not a
physical cache-disable cycle. This ownership protocol must be wired into every
future flash/cache owner before the repository can assert a whole-firmware
cache-off proof.

The final hardware run with the durable Rust producer/consumer queue completed
scan, WPA2, DHCP, ping, DNS, TCP, HTTP, ADDBA, 4,096/4,096 UDP datagrams and
4/4 HTTP transfers. It balanced 4,786/4,786 TX and 691/691 RX owners, drained
20,591/20,591 PP events without rejection, preserved the allocation snapshot,
and measured 19.634 Mbit/s. No strict-runtime path now dereferences `pTxRx`;
the object remains a cold-initialization and one-shot handoff oracle until
those initializers are ported. Packet recycling, PP protocol routing, and the
bounded rate-context lookup are now Rust-owned. The remaining aggregate RX
vendor boundary is `wDev_ProcessRxSucData`; its protocol routing, rate
lookup/update, and public network-buffer release tails are already Rust-owned.

The interrupt waker now also has an explicit ownership-ordering contract:
producers publish readiness before waking, and the consumer registers before
testing readiness. Registration contention returns with a durable pending bit
instead of re-pending the same software interrupt. This prevents the
high-priority radio bottom half from starving a preempted producer that owns
the short waker lock. Six cold-start stress cycles passed after this change;
JTAG recorded one actual contention in the final passing cycle. All cycles
retained balanced TX/RX/PP ownership and zero allocations.

Final ELF verification must continue to use a non-empty STA configuration;
otherwise the HIL binary deliberately enters `pending()` before Wi-Fi
initialization and LTO removes the unreachable strict runtime.

The base-layout multi-descriptor indication body is now Rust-owned as well.
The recovered hardware and ESF length fields are fourteen bits, so safe Rust
accepts only chains of two through 64 descriptors whose joined length is at
most `0x3fff`. It computes the complete copy plan before the raw-pointer leaf:
the first 0x38-byte control prefix, the remainder of the first full segment,
every complete middle segment, and the tail's actual received length. The
leaf validates the exact chain terminus, recycles the detached hardware
prefix once, and publishes one kind-7 owner. Copy-mode-one split frames retain
the pinned immediate-discard result.

This rare path has two fixed owners rather than increasing all 32 ordinary RX
objects. Each owner has a 0x90-byte ESF header and its two-word ownership
bitmap in internal SRAM; two 16-KiB payloads are placed in initialized PSRAM.
The hard RX interrupt only queues through the intrusive link inside the SRAM
header. Payload access begins on the radio executor and remains behind the
typed Radio -> Network -> Free owner. Aggregate reorder IDs occupy a separate
two-entry suffix and therefore do not increase the normal network-channel
credit derived from the kind-7 pool.

The configured final image contained the aggregate ownership bitmap at
`0x2f0353a8`, aggregate headers at `0x2f0353b0`, and the payload arena at
`0x50001038`. Its state audit reported zero mutable blob bytes and zero
ROM-ABI indirection cells reachable from strict leaves. The intentional SRAM
cost raised the qualified strict-static ceiling from 311,745 to 311,922
bytes; the 32-KiB PSRAM arena is excluded from that internal-SRAM metric. The
strict audit still inspected 6,407 functions under 25 roots with zero
no-wait/no-heap violations.

The following hardware regression completed WPA2, DHCP, DNS, TCP/HTTP,
4,096/4,096 UDP datagrams, and 4/4 HTTP transfers at 26.343 Mbit/s. It
balanced 4,787/4,787 TX and 692/692 RX owners with zero allocations, ESF
rejects, or vendor indication fallbacks. Ordinary 1,500-byte Ethernet traffic
kept `max_descriptors=1` and `rust_multi_indicate_routes=0`; consequently the
hardware run proves whole-path non-regression and memory placement, while a
real multi-descriptor MPDU remains a separate jumbo/A-MSDU HIL qualification.

The remaining RX compatibility boundary is now classified by seventeen
mutually exclusive fixed counters. This exposed a false fallback after link:
`g_wdev_csi_rx` became non-null even though every received unit reported a
zero CSI length. The pinned `wDev_IndicateFrame` disassembly proves that the
callback pointer is irrelevant for this admitted profile. At
`+0xf4..+0x10a`, `s5` is overwritten with the ten-bit CSI length from metadata
bytes `0x26..0x27`; the call to `wdev_csi_rx_process` at `+0x298` is guarded
by `beqz s5`. The Rust admission predicate already rejects every value other
than `Some(0)`, so reading the callback pointer introduced a stricter condition
than the binary and was removed.

The resulting image passed 275 host tests and the 6,407-function/25-root
strict audit with zero violations. Its strict graph reaches no mutable blob
global or ROM-ABI state cell. Seventeen diagnostic counters raise the
temporary strict-static baseline from 311,922 to 311,990 bytes. Hardware then
processed all 709 RX units through Rust: 695 data routes, 14 management routes,
zero vendor fallbacks, zero indication fallbacks, and zero copy/allocation
rejects. WPA2, DHCP, ICMP, DNS, TCP/HTTP, 4,096/4,096 UDP datagrams and 4/4
HTTP transfers passed at 30.332 Mbit/s with 4,787/4,787 TX owners returned and
an all-zero allocation snapshot.

## Completed MAC address and RX-policy MMIO slice

The public `ic_set_mac`, `ic_set_rx_policy`, and
`ic_set_rx_policy_ubssid_check` symbols are now Rust-owned. Their complete
references are the pinned `libpp.a[if_hwctrl.o]` wrappers and
`libpp.a[hal_mac.o]` leaves:

- MAC address programming packs six caller-owned bytes into the register pair
  starting at `0x2010_405c + interface * 8`, then sets the evidenced
  high-register valid bit;
- RX policy uses the three bounded queue records at `0x2010_40d8`, the
  address-policy records at `0x2010_4004`, and the management-policy records
  at `0x2010_4060`;
- unique-BSSID policy admits exactly queues zero through three and performs
  the two recovered ordered read/modify/write operations.

All address calculations and masks are pure tested transforms. The ABI leaves
contain finite volatile MMIO only: no ROM/vendor call, loop, wait, delay,
allocation, or global driver state. Invalid MAC pointers/interfaces trap
before dereference; invalid RX-policy queues preserve the recovered wrapper
return contract. The known archive callers are interface and supplicant
configuration paths, not interrupt handlers, so these three functions remain
flash-mapped and do not consume the 48-byte CPU0 stack margin. A trial SRAM
placement was correctly rejected by the 16 KiB stack gate.

The qualified final ELF resolves every public name to its corresponding
`wifi_strict_*` symbol. Runtime ownership debt decreases from
`1 fallback + 9 stateful/unproven` to
`1 fallback + 6 stateful/unproven`; strict vendor roots decrease from ten to
seven and reachable vendor functions from 23 to 17. The 6,407-function audit
still reports zero no-wait/no-heap violations, zero mutable blob globals
reachable from strict leaves, and zero temporary MMIO roots. Internal-SRAM
strict storage remains 313,293 bytes and the CPU0 stack remains 16,432 bytes.

The flashed ESP32-S31 image completed passive scan, open authentication,
HT20/WMM association, WPA2 M1-M4, DHCP, gateway ping, DNS and HTTP 200.
It returned 19/19 TX and 16/16 RX owners without queue rejection, recorded
zero allocation/reallocation/free calls, zero other-core stalls, and no
`ppTask` entry.

## Completed management-frame allocation boundary

The public `ieee80211_getmgtframe` symbol is now Rust-owned. Its complete
reference is the pinned
`libnet80211.a[ieee80211_ets.o]::ieee80211_getmgtframe` body, size `0x5c`.
The replacement preserves its finite transform:

- checked `header + body`, rounded up to four bytes;
- ESF kind 3 through 64 bytes, kind 2 through 256 bytes, and kind 4 above
  256 bytes;
- body pointer at `buffer_descriptor.data + header`;
- original body length stored as a 16-bit value at ESF offset `0x16`.

The replacement reuses the existing sixteen-entry, fixed 1,744-byte Rust ESF
management pool and its single ownership bitmap. It introduces neither a
second ledger nor storage. Pool exhaustion, arithmetic overflow, a body length
larger than `u16`, or a malformed descriptor all fail immediately. There is no
heap call, retry loop, delay, wait, or critical section in this wrapper.
Before strict handoff it retains the already documented cold ESF fallback;
prearmed and strict operation uses only the Rust pool.

The qualified final ELF resolves `ieee80211_getmgtframe` and
`wifi_strict_ieee80211_getmgtframe` to `0x400d067c`. Host coverage increased
to 315 sequential tests. The 6,407-function strict audit still reports zero
violations; runtime ownership debt decreases from
`1 fallback + 6 stateful/unproven` to
`1 fallback + 5 stateful/unproven`, strict vendor roots decrease from seven
to six, and reachable vendor functions from 17 to 16. Strict leaves still
reach zero mutable blob globals and zero ROM-ABI state cells. Internal-SRAM
strict storage remains 313,293 bytes and the CPU0 stack remains 16,432 bytes
because the task-context wrapper is flash-mapped.

The hardware regression completed passive scan, open authentication,
HT20/WMM association, WPA2 M1-M4, DHCP, gateway ping, DNS and HTTP 200.
The ESF snapshot ended with zero of sixteen management slots claimed and zero
rejected operations. It returned 18/18 TX and 15/15 RX owners, recorded zero
allocation/reallocation/free calls, zero other-core stalls, and no `ppTask`
entry.

## Completed WPA2 hardware key-slot boundary

The public `ic_set_key`, `ic_del_key`, and `wDev_Insert_KeyEntry` symbols are
now Rust-owned. Their complete references are the pinned bodies
`libpp.a[if_hwctrl.o]::ic_set_key` (68 bytes),
`libpp.a[if_hwctrl.o]::ic_del_key` (8 bytes),
`libpp.a[wdev.o]::wDev_Insert_KeyEntry` (142 bytes),
`libpp.a[hal_crypto.o]::hal_crypto_clr_key_entry` (138 bytes), and
`libpp.a[hal_crypto.o]::hal_crypto_enable` (144 bytes).

The replacement preserves the evidenced finite operations: interface-specific
crypto-control address selection, control-bit composition, the policy
read/modify/write at `0x2010_4810`, hardware key programming through the
already bounded `hal_crypto_set_key_entry` leaf, key-valid bitmap clearing,
and ten ordered zero stores when a slot is removed. It contains no allocation,
retry, wait, delay, task primitive, or vendor/ROM call other than the existing
finite key-programming MMIO leaf.

`StaticWpa2Keys` and the fixed vendor-slot tokens remain the sole typed Rust
owners of key material and slot lifetime. The strict path deliberately does
not mirror the legacy `if_ctrl` algorithm cache or the `wDevCtrl` teardown
bitmaps: duplicating those implementation caches would introduce a second
source of truth. Removal instead clears the hardware entry explicitly and
returns the Rust-owned slot token.

The qualified final ELF resolves `ic_del_key` and its replacement to
`0x400d03b6`, `ic_set_key` to `0x400d03f8`, and
`wDev_Insert_KeyEntry` to `0x400d043a`. Host coverage increased to 316
sequential tests. Runtime ownership debt decreases from
`1 fallback + 5 stateful/unproven` to
`1 fallback + 2 stateful/unproven`; strict vendor roots decrease from six to
three and reachable vendor functions from 16 to 10. The 6,407-function audit
reports zero no-wait/no-heap violations, zero mutable blob globals reachable
from strict leaves, and zero ROM-ABI state cells. No static storage was added:
internal-SRAM strict storage remains 313,293 bytes and the CPU0 stack remains
16,432 bytes.

The hardware regression completed passive scan, open authentication,
HT20/WMM association, WPA2 M1-M4, DHCP, gateway ping, DNS and HTTP 200.
It returned 18/18 TX and 16/16 RX owners, recorded zero
allocation/reallocation/free calls, zero other-core stalls, zero ESF
management claims or rejects at shutdown, and no `ppTask` entry.

## Completed current-channel ownership boundary

The strict channel switch no longer calls `ic_set_current_channel`. The
complete pinned references prove that this is a legacy cache publication, not
a hardware operation:

- `libpp.a[if_hwctrl.o]::ic_set_current_channel` is a 12-byte null check and
  tail call;
- `libpp.a[wdev.o]::wDev_SetCurChannel` is a 26-byte copy of the two selector
  bytes to `wDevCtrl[0x2c..=0x2d]`.

The Rust channel state already separates the requested selector from the
physically active selector. `channel_switch::State::channel` owns the request
while a transition is in progress; `ChannelState::current` is published only
after the asynchronous MAC-stop edge, PHY programming, CSI bandwidth update,
and MAC restart complete. No strict-runtime reader consumes the two legacy
`wDevCtrl` bytes. Preserving that C cache would therefore create a second,
prematurely published source of truth rather than retain useful behavior.

The vendor symbols remain linked for pre-handoff `wl_chm` compatibility, but
are unreachable from the strict runtime graph. No replacement ABI, new
global, allocation, wait, delay, polling loop, or unsafe state alias was
introduced. Runtime ownership debt decreases from
`1 fallback + 2 stateful/unproven` to
`1 fallback + 1 stateful/unproven`; strict vendor roots decrease from three
to two and reachable vendor functions from 10 to 8. The 6,407-function audit
reports zero no-wait/no-heap violations and zero mutable blob globals or
ROM-ABI state cells reached by strict leaves. Internal-SRAM strict storage
remains 313,293 bytes and the CPU0 stack remains 16,432 bytes.

Hardware verification exercised the semantic part of the change rather than
only startup: a six-record passive scan changed channels through the Rust
state machine, then open authentication, HT20/WMM association, WPA2 M1-M4,
DHCP, gateway ping, DNS and HTTP 200 completed. It returned 18/18 TX and
15/15 RX owners with zero allocation/reallocation/free calls, zero
other-core stalls, no ESF rejection, and no `ppTask` entry.

## Completed MAC-restart runtime boundary

The strict channel switch no longer calls `ic_mac_init`. The replacement is a
Rust-owned, finite MMIO transaction derived from the complete pinned bodies:

- `libpp.a[if_hwctrl.o]::ic_mac_init`, 40 bytes;
- `libpp.a[hal_mac.o]::hal_mac_init`, 48 bytes;
- `libpp.a[hal_pwr.o]::pwr_hal_select_wifimac_regdma_link`, 32 bytes;
- `libpp.a[pm.o]::pm_get_tx_blocks_retention_mask`, 36 bytes;
- `libpp.a[pm.o]::pm_set_wifimac_regdma_link_selection`, 10 bytes.

The handoff already sets `WIFI_PS_NONE` and reads it back before strict
operation. Under that invariant the retention-mask query returns all ones.
Rust therefore clears the evidenced `0x00ff_1000` mask at `0x2010_4cac`,
then selects REGDMA link four in bits 20:17 of `0x2010_d83c`. Both are one
ordered volatile read/modify/write.

The final vendor store of one to `g_wifimac_regdma_link_selected` is not
reproduced. It is a cache consumed by the vendor PM getters, while strict PM
hooks are disabled under the verified no-power-save profile. Keeping the
write would preserve hidden C state with no strict consumer. The Rust leaf
has no call, loop, wait, delay, allocation, or non-MMIO mutable state; the
channel state machine provides its single-owner serialization.

This completes the stateful runtime-root ledger. Strict vendor roots decrease
from two to one, reachable vendor functions from eight to three, and ownership
debt from `1 fallback + 1 stateful/unproven` to
`1 fallback + 0 stateful/unproven`. The sole remaining runtime root is the
explicit `wDev_ProcessRxSucData` compatibility fallback. The 6,407-function
audit reports zero violations, zero mutable blob globals reachable from strict
leaves, and zero ROM-ABI state cells. Internal-SRAM strict storage remains
313,293 bytes and the CPU0 stack remains 16,432 bytes.

The hardware regression completed a six-record passive scan, open
authentication, HT20/WMM association, WPA2 M1-M4, DHCP, gateway ping, DNS and
HTTP 200. It returned 18/18 TX and 16/16 RX owners, recorded zero allocation,
reallocation, or free calls, zero other-core stalls, no ESF rejection, and no
`ppTask` entry.

## Typed PHY-PBus and PHY-I2C capability boundary

The recovered S31 SVD now generates typed identities for the PHY-PBus,
PHY-I2C host/master registers, all 45 command-RAM words, and the PMU analog
I2C power/reset registers. The active `PhyColdExternalBinding` target methods
take `&mut RadioRegisters`; the application obtains that borrow only from
`Radio<P, Powered>`.

This removes raw addresses from the cold path for:

- PMU RF and peripheral-I2C power/reset;
- PBus debug/work mode, force-test publication/completion, RX-DCO packed
  reads, and RX/TX clock pairs;
- PHY-I2C host reads/writes, clock selection, master registration, and
  command-RAM initialization.

Each HAL method names the complete ROM/blob body used for operation order.
Register and field identities remain independently sourced from the S31 SVD,
official PMU header, or instruction-exact masks.

The reusable calibration bindings outside `PhyColdExternalBinding` now use
the same capability boundary. RFPLL, RXIQ/TXIQ, DCO, gain, temperature,
saturation, power and power-detector I2C/PBus target methods all require a
`RadioRegisters` borrow. The transitional `*_unowned` PHY-I2C leaves and raw
PBus force-test leaves have been deleted, and these target methods are no
longer `unsafe`. A target-source audit rejects their old method names and
finds no `unsafe start_target`, `unsafe observe_target_edge`, or
`unsafe sample_target_once`.

This closes ownership migration for the recovered PHY-I2C and PBus command
engines.

The shared PHY table-memory aperture is now behind the same capability. The
recovered SVD/PAC owns its base-index source, three data words, multifunction
command word and six PBUS boundary words. PBUS-memory, TX-CFR, baseband
RX-table, RX-gain and channel TX-gain publications all require
`&mut RadioRegisters`. The old TX-gain `extern "C"` leaf accepted five raw
pointers and depended on seed/output fields being adjacent in memory; its
replacement receives an owned `PhyWifiTxGainImage` and expresses the vendor
halfword concatenation with ordinary Rust indexing. The open front-end
initializer also configures the shared high-byte base index through the PAC,
so active code no longer reaches this aperture by address.

Other calibration MMIO remains separate work: it can move behind the same
capability only after each register identity and field mask has been added to
the recovered SVD/PAC.

The vendor-comparison HIL has one explicit exception to the normal safe
`power_up` transition:
`Radio::assume_powered_after_external_initialization`. It consumes the unique
`Owned` value and returns `Powered` without touching registers, because
replaying the normal reset pulse after vendor calibration would invalidate
the oracle state. The method is `unsafe`; its invariant requires the external
initializer to have completed all clock/power/reset prerequisites and to stop
accessing the radio before Rust adopts it. Production open-PHY initialization
does not use this bridge.

## Typed PHY AGC and 11b register boundary

The next recovered MMIO cluster is generated as `PHY_AGC_ORACLE`. Its primary
evidence is the complete rev0 ROM ELF, SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
not a neighboring-chip register header. Four finite bodies prove the
addresses, masks, values and access order: `phy_bb_agc_reg_update`,
`phy_disable_agc`, `phy_enable_agc`, and both branches of
`phy_rx_11b_opt`.

The new `phy_agc` HAL owns these transactions through
`&mut RadioRegisters`. It keeps unknown electrical meanings explicitly
unknown while preserving:

- all fifteen writes in the baseband AGC update;
- the disable clear followed by the enable set/clear pulse;
- the single disable write;
- the five independently read field replacements and final window update in
  either 11b branch.

The live baseband state machine, RX-table suffix and channel transition now
pass their existing unique register borrow into these leaves. Their former
raw-pointer implementations and hard-coded addresses are gone.

## Typed PHY post-initialization register boundary

The complete blob-derived post-init sequence now uses the same
`PHY_AGC_ORACLE` PAC and `&mut RadioRegisters` capability. Its primary blob
source is pinned `libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`:
the complete `phy_reg_update_new` parent and `phy_set_ftm_en` tail. The
complete rev0 ROM `phy_wifi_agc_sat_gain` leaf at `0x2f827db0`, size `0x0c`,
proves the two full-word stores.

The generated PAC adds the five post-init register identities at offsets
`0x705c`, `0x7064`, `0x7114`, `0x78c8` and `0x7d4c`, and reuses the
instruction-proven nine-bit field on the shared `0x7104` word. Unknown
electrical meanings remain `UNKNOWN`. The safe HAL method preserves the
seven vendor writes and their order, including a fresh read before each of
the two field replacements at `0x78c8`. A second safe method owns the same
two saturation-gain destinations for the dynamic value used during
`phy_reg_init`.

`PhyBbMmioBinding::execute_target` and `configure_phy_registers` now pass
their existing unique register borrow into these operations. The exported
`wifi_strict_phy_reg_update_new` C ABI, its six raw addresses, and duplicate
mask helpers are deleted.

## Typed AGC initialization, RF RX saturation, and gain limits

The remaining raw consumer of shared `0x2010705c` is now removed together
with its complete neighboring operations. Complete rev0 ROM
`phy_agc_reg_init` at `0x2f8278d8`, size `0xd8`, proves ten fresh-read
updates. Complete rev0 ROM `phy_rfrx_sat_rst` at `0x2f828944`, size `0x42`,
proves the common full-word write and both two-update branches. Complete
pinned `libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table` proves its final two
limit writes. None of these leaves requires a guessed neighboring-chip
field name.

The generated `PHY_AGC_ORACLE` PAC now describes the parameter word, shared
AGC control, shared saturation control, RF-saturation configuration, low
gain limit, final high-byte initialization, and capped RX-gain limit. The
discontiguous `0xd1080000` phase mask is deliberately split into its
instruction-proven unknown subfields. Safe HAL methods retain all ten
AGC-init writes, all three writes in either saturation phase, and the two
final limit writes with the reference's fresh-read order.

`configure_phy_registers` and the baseband MMIO binding reuse their existing
`&mut RadioRegisters`. `PhyRxGainInitMmioBinding::execute_target` now also
requires that borrow, so its root MMIO actions can no longer execute outside
the radio owner. The source audit rejects raw `0x08bc`, `0x705c`, `0x7068`,
`0x7094`, `0x7128`, and `0x713c`.

## Shared AGC, PBus pulse, RX compensation, and antenna ownership

The remaining active users of shared `0x2010_702c` are now represented by one
PAC register rather than overlapping raw aliases. Complete rev0 ROM
`phy_pbus_force_mode` at `0x2f824102`, size `0x90`, proves the high-byte
replacement and the set/delayed-clear pulse. Complete pinned
`libphy.a[phy_reg.o]::phy_set_rx_comp_new`, size `0x28`, proves the independent
low-byte replacement and the companion high-byte replacement at
`0x2010_70a0`. The existing RX-gain field remains the same physical identity.

Complete rev0 ROM `phy_ant_init` at `0x2f827df4`, size `0x44`, also closes the
three raw antenna updates. Its middle field shares `0x2010_7030` with the
independently proven AGC-disable bit; the SVD records both fields on one
register. The other updates are localized at `0x2010_711c` and
`0x2010_7120`. Field names retain `UNKNOWN` because these bodies prove masks,
values, and order but not electrical semantics.

Safe HAL leaves preserve the two RX-compensation writes, the PBus tail's
high-byte/set/clear sequence around caller-owned delays, and all three antenna
writes. The PBus debug/work-mode pair uses the existing typed `PHY_PBUS`
registers. TX-DC, TX-DC/PWDET, PWDET, TX calibration environment, RXIQ
initialization, RX-gain DC, TXIQ, and RX-saturation MMIO bindings all receive
`&mut RadioRegisters`, so those actions cannot cross the radio capability
boundary.

The raw C ABI, raw antenna/PBus wrappers, and duplicate mask helpers are
deleted. The target source audit rejects `0x0884`, `0x088c`, `0x702c`,
`0x7030`, `0x70a0`, `0x711c`, and `0x7120`, closing this shared-register
migration without assigning guessed neighboring-chip names.

## Typed channel cleanup and BBPLL control

The channel cleanup's final two raw leaves are now behind the same
`RadioRegisters` capability. Complete pinned
`libphy.a[phy_reg.o]::phy_dc_mem_clr`, size `0x1c`, proves two fresh-read
edges at `0x2010_703c`: set bit 20, then clear it. SVD v0.7 records that
single instruction-exact field as `CLEAR_PULSE_UNKNOWN`.

Complete rev0 ROM `phy_bbpll_cal` at `0x2f827dbc`, size `0x1c`, proves one
fresh-read replacement at `0x2010_f818`: false selects encoded value one in
bits 3:2 and true selects encoded value two. The word is already the PAC's
`PHY_I2C_MASTER.MASTER_CONTROL`, shared with independently recovered
master-register fields, so the new HAL method reuses that identity and
preserves every unrelated bit.

Cold initialization, reusable register initialization, and every channel
change now call the safe HAL methods with their existing unique register
borrow. The two raw exported C leaves, the unused raw master-register wrapper,
and their duplicate masks/tests are removed. The target audit rejects raw
`0x703c` and `0xf818`, leaving no alternate access path for either physical
register.

## Owned frequency-memory and channel control

SVD v0.8 and the generated PAC now own the complete frequency/channel MMIO
slice used by open cold init and channel changes. Complete rev0 ROM bodies
prove all addresses, masks and fresh-read ordering; complete pinned
`register_chipv7_phy`, `phy_bb_init`, and `phy_chip_set_chan` bodies prove the
parent order. The public Espressif tree contains no source definitions for
these internal PHY symbols, so uncertain electrical roles remain explicitly
`UNKNOWN`.

The new safe `phy_frequency` HAL owns frequency-module reset and hardware
ownership, register initialization, the 85-entry frequency-memory publisher,
packed PHY-I2C number-address words, channel-switch pulse and readiness
sampling, NRX quotient calculation, BSS/CBW fields, shared FBW/BT filtering,
Wi-Fi/baseband enable fields, and TX-cap command-memory publication. Host
models record every read and write, including the two-read NRX operation and
all pulse edges.

Cold init, baseband init, register init, D-code and channel transitions pass
their existing `&mut RadioRegisters` into these methods. In particular,
`PhyDcodeMmioBinding::execute_target` is now safe and requires the explicit
borrow; it can no longer access `0x7848` through a hidden volatile pointer.
The old raw wrappers, address constants, duplicate arithmetic helpers and
their unit tests are deleted.

The source audit now rejects raw `0x001c..0x003c`, `0x0874`, `0x4400`,
`0x7848`, `0x7ce0`, `0x7ce4`, `0x9c18`, and `0xfc04`, together with the old
wrapper names. This leaves one PAC identity for each physical register,
including the documented mode-dependent collision of frequency-memory
address bit ten and module-reset bit 18.

## Owned baseband initialization and power-detector MMIO

SVD v0.9 and the generated PAC now own the complete raw register slice used
by `phy_reg_init`, `phy_bb_reg_init`, `phy_tx_paon_set`,
`phy_bb_wdg_cfg`, `phy_noise_floor_auto_set`, `phy_i2c_txrate_init`,
`phy_bb_txpwr_track`, and the PWDET/TX-DC power-detector leaves. The primary
evidence is the complete S31 rev0 ROM and pinned `libphy.a`, whose exact
hashes, symbol addresses and sizes are recorded in the SVD source ledger.

The two new safe HAL modules preserve the original fresh-read order rather
than merging adjacent updates. Unknown electrical roles remain explicit in
PAC field names. `configure_phy_registers`, cold RF init, baseband actions,
RX-gain initialization, PWDET, TX calibration and TX-DC/PWDET bindings all
borrow the unique `RadioRegisters` owner. PWDET ready and SAR-result sampling
now require that borrow as well; the old no-argument target readers are gone.

The duplicate raw wrappers, address constants, mask transforms and their
second unit-test copy were deleted from `radio_hal.rs`. The source-only audit
now rejects their names and all exclusively migrated physical addresses,
including the `0x0808..0x081c`, baseband `0x7400..0x7cd0`, and auxiliary
`0x20701068` leaves. Shared IQ, TX-gain compensation, and front-end identities
remain valid transitional raw consumers until every operation on those
physical words moves together.
