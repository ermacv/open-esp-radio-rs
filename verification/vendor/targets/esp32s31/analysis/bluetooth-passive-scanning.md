# ESP32-S31 legacy passive BLE scanning frontier

This note defines the next bottom-up Controller slice after Direct Test Mode
and legacy non-connectable advertising.  Its first product target is narrow:
receive legacy advertising PDUs on the LE 1M primary channels and emit standard
HCI LE Advertising Reports.  Active scanning, extended advertising, initiating
and connections are not prerequisites.

The vendor ULL is evidence about hardware ownership and timing, not an
architecture to reproduce.  The open driver will keep portable Link Layer
policy in `driver/bluetooth/ll`, hardware register transactions in the
restricted PAC, and private controller-SRAM encoding in the ESP32-S31 memory
crate.

## Pinned identity evidence

The current S31 instruction authority is
[`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31),
whose `libble_app.a` SHA-256 is
`62dbe7216619d1f1e3dcd51233d91b211add15c7c746851af0be6a632cdae195`.
The same-chip initial archive
[`espressif/esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd),
`libble_app.a` SHA-256
`ec10a20eaf869f7cd2300100fe54826980525911f8417206af5a0745a9f85f63`,
retains descriptive symbols.  Matching object identity, function-section
index, extent and control-flow shape supply role names only; all behavioral
claims come from complete current S31 bodies.

The following identities are sufficient to bound the first passive-scanning
slice.

| Current S31 member and symbol | Section | Same-chip role name |
| --- | ---: | --- |
| `43.o:r_sym_ble_YNRdWpFhJV48tYZ6nKiv` | 128 | `r_ble_ll_scan_set_scan_params` |
| `43.o:r_sym_ble_ZmPaGKsQP6Lxm8pZyhdo` | 114 | `r_ble_ll_scan_set_enable` |
| `43.o:r_sym_ble_GUS6jpjPpQlTJuvOrYMV` | 108 | `r_ble_ll_scan_rx_pkt_in_on_legacy` |
| `43.o:r_sym_ble_vcZNbFK4oJbHVErpPHHl` | 54 | `r_ble_ll_scan_send_adv_report` |
| `65.o:r_sym_ble_ruvdJkUUpoEtaH2xv1jH` | 18 | `r_ble_lll_scan_alloc_rxbuf` |
| `65.o:r_sym_ble_FI34eU2bYuxQYjeVS51T` | 76 | `r_ble_lll_scan_alloc_memory` |
| `65.o:r_sym_ble_XwMfMguyw9lccbIeVsvN` | 28 | `r_ble_lll_scan_reset_link_state` |
| `65.o:r_sym_ble_jMEoTs6F9eIXRCc3ssv4` | 99 | `r_ble_lll_scan_rx_process` |
| `65.o:r_sym_ble_XYr0jejHXj4ULilAqOsG` | 103 | `r_ble_lll_scan_recycle_buffer` |
| `65.o:r_sym_ble_ZZKI1PtdMgQh831P4IWz` | 107 | `r_ble_lll_scan_restart` |
| `65.o:r_sym_ble_q4hMJ7XLGGCzxwmAKSge` | 111 | `r_ble_lll_scan_chk_resume` |
| `65.o:r_sym_ble_h1CfV40z3TOeYWAmKSQ9` | 115 | `r_ble_lll_scan_recycle_sch_item` |
| `65.o:r_sym_ble_QOG2ExWuZYIMUrJH3TXE` | 117 | `r_ble_lll_scan_stop` |
| `65.o:r_sym_ble_znMr0TnKK4lkFEsrathq` | 121 | `r_ble_lll_scan_start` |
| `49.o:r_sym_ble_eTAdDKHzRfvU8IdHAKlf` | 14 | `r_ble_lll_get_rxed_buffer` |
| `49.o:r_sym_ble_m6P3iS8lEEytcaOFBVC4` | 16 | `r_ble_lll_set_rxbuf_default_value` |
| `49.o:r_sym_ble_9HlsTu6stE6q0PM6rEpk` | 18 | `r_ble_lll_append_rx_buffer` |
| `49.o:r_sym_ble_0dI0cNOVzvRIEd1gPpi7` | 20 | `r_ble_lll_rxpdu_copy` |
| `49.o:r_sym_ble_N2bQ5jI8Lnppq1TkXRdA` | 38 | `r_ble_lll_rx_buffer_link_prepare` |
| `61.o:r_sym_ble_YWBOuvQw70C562FKtQQy` | 45 | `r_ble_lll_mmgmt_rxbuf_cnt_get` |
| `61.o:r_sym_ble_rAmBz3o2v26sqbA8bMfL` | 51 | `r_ble_lll_mmgmt_rxbuffer_disable_insert_check` |
| `61.o:r_sym_ble_AzvE27e0dx0P2JPL5N20` | 53 | `r_ble_lll_mmgmt_alloc` |
| `61.o:r_sym_ble_R2AW03Rf36LQ7NOPHFN6` | 82 | `r_ble_lll_mmgmt_sm_num_match` |
| `61.o:r_sym_ble_StyBzg5dzXElrGKr9PEI` | 91 | `r_ble_lll_update_global_rxlink_params` |
| `61.o:r_sym_ble_HL6xpyhopnPTnSDqTURd` | 97 | `r_ble_lll_mmgmt_reset_rxlink` |
| `61.o:r_sym_ble_ciUbjr6ihzocNthFnrg3` | 123 | `r_ble_lll_mmgmt_alloc_global_rxlink_mem` |
| `61.o:r_sym_ble_UagS1VQZDxizyWqNpmtA` | 125 | `r_ble_lll_mmgmt_alloc_buffer_hdr` |
| `61.o:r_sym_ble_REEg6swLrLA2MChrbW6D` | 129 | `r_ble_lll_mmgmt_alloc_rx_buffer` |
| `61.o:r_sym_ble_RrhJz2UYSxQiy2foMpKL` | 131 | `r_ble_lll_mmgmt_rxbuffer_cnt_change` |
| `61.o:r_sym_ble_lecwwE0KZNKhANvOphXa` | 135 | `r_ble_lll_mmgmt_update_global_rxlink` |
| `61.o:r_sym_ble_bUNJ21TLtnDa9owGUE7A` | 137 | `r_ble_lll_mmgmt_rxbuf_direct_alloc` |

The independently named C61 body at
[`espressif/esp32c61-bt-lib@c800514c39a3e491bb13bb224987e109623d2cf2`](https://github.com/espressif/esp32c61-bt-lib/tree/c800514c39a3e491bb13bb224987e109623d2cf2)
corroborates the 102-byte `r_ble_lll_scan_chk_resume` identity.  It is not
register or ABI evidence for S31.

## Proven lower transaction

Complete current `r_sym_ble_znMr0TnKK4lkFEsrathq` establishes the outer start
transaction without requiring the vendor's software object graph:

1. select and allocate scanner memory through
   `r_sym_ble_FI34eU2bYuxQYjeVS51T`;
2. reset the hardware-consumed link state through
   `r_sym_ble_XwMfMguyw9lccbIeVsvN`;
3. place primary channel 37 in the first scheduler item, and in the optional
   second item when that item exists;
4. publish `1` to `BLE_SCAN_CONTROL.COMMAND_2` and `COMMAND_1`, then publish
   either `0x100` or `1` to `COMMAND_0` on the two observed branches;
5. wake the common RF owner, derive the first event time and invoke the common
   scheduler insertion path;
6. retry only the scheduler-collision result `-2`, increasing the requested
   delay by 100 controller-time units on every retry.

The branch that chooses `COMMAND_0=0x100` versus `COMMAND_0=1` depends on a
software-state predicate that has not yet been reduced to the passive-1M
domain.  Consequently these complete words remain positional PAC operations;
the driver must not expose them as guessed enable or PHY fields.

The already reviewed global-memory classifier is also applicable: scanner
scheduler kind two uses current/next RX selector one.  DTM's private graph is
not reusable here.  The scanner must own a normal current/next RX chain and
the hardware-to-CPU rotation, completion fence and backpressure that go with
it.

## Proven allocation boundary

Complete current `r_sym_ble_ruvdJkUUpoEtaH2xv1jH` does not allocate an opaque
vendor scan object.  It composes the common receive-memory primitives around
one already allocated link state:

1. `r_ble_lll_rx_buffer_link_prepare(link_state, 1)` clears link-state words at
   `+0x68`, `+0x70` and `+0x78`;
2. `r_ble_lll_mmgmt_rxbuffer_disable_insert_check(link_state, 1)` configures
   direct-allocation and skip-insertion policy on the global RX-link object
   referenced at link-state `+0x7c`;
3. `r_ble_lll_mmgmt_rxbuf_direct_alloc` receives the pointer stored at
   link-state `+0x64` and tail-calls the global RX-link update routine;
4. allocation succeeds only when `r_ble_lll_mmgmt_rxbuf_cnt_get(link_state)`
   reports a nonzero count.

The alternate `r_ble_lll_rx_buffer_link_prepare(link_state, 0)` path allocates
a 24-byte buffer header, stores the same header pointer at `+0x68` and `+0x70`,
clears `+0x78`, and marks header word `+0x0c`.  The scanner takes the first
path, so reproducing a vendor heap or general-purpose mmgmt allocator is not a
driver requirement.  What remains to prove is the final selector-one RX-link
image produced by `r_ble_lll_mmgmt_update_global_rxlink`, not the allocator's
software bookkeeping.

Passive scanning also has no TX-buffer prerequisite.  Complete current
`r_ble_ll_scan_set_scan_params` validates the first HCI payload octet as zero
or one and stores it in the selected PHY configuration byte at `+0x04`.
Complete current `r_ble_lll_scan_alloc_txbuf` reads that same byte through the
scanner's selected-PHY pointer and returns success immediately when it is
zero.  Zero is the portable passive scan type, as independently documented by
the pinned
[`esp-nimble` controller source](https://github.com/espressif/esp-nimble/blob/916be244a9c646bc16fd65507478cf3fe717d8ed/nimble/controller/src/ble_ll_scan.c).
The first vertical slice therefore needs an RX chain only; scan-request PDU
construction remains deferred with active scanning.

The admission-only Blobray scope intentionally removes upper packet parsing
and HCI reporting roots.  It reaches 255 functions versus 257 in the complete
passive-scanning scope, confirming that the dominant open graph is the shared
allocator/scheduler path rather than host policy.  Its generated interface
anchors are navigation prerequisites, not hardware evidence, and must not be
filled with guessed contracts.

## Proven receive graph and recycle transaction

The complete current common LLL and memory-manager bodies close the physical
shape of one scanner RX node without making the vendor allocator part of the
driver architecture.  Each node consists of a 24-byte header and a separately
linked packet allocation.  The header positions used by the complete current
bodies are:

| Header offset | Proven use |
| ---: | --- |
| `+0x00` low 20 bits | compressed successor-header link |
| `+0x04` low 20 bits | compressed packet-allocation link |
| `+0x0c` bit 31 | hardware-completion gate examined before a node can be returned to software; cleared before republishing the node |
| `+0x10` bit 0 | private list-rotation marker used by the common removal/append path |
| `+0x14` | full CPU pointer used while unlinking and reconnecting the chain |

Complete current `r_ble_lll_mmgmt_rxbuffer_cnt_change` requests a payload
capacity of `0xff` from `r_ble_lll_mmgmt_alloc_rx_buffer`.  The latter reserves
`0x1e + 0xff = 0x11d` bytes and writes the capacity plus two into packet bytes
`+0x05` and `+0x06`.  The packet positions needed by the first open role are:

| Packet offset | Proven use |
| ---: | --- |
| `+0x0c` low 24 bits | producer sentinel, reset to all ones before publication and required to change before accepted processing |
| `+0x0f` signed byte | receive-strength value copied into the LL receive metadata consumed by the legacy advertising-report path |
| `+0x18` low 16 bits | hardware-written receive cursor/epoch value; reset to all ones and checked both against the active memory-manager epoch and the bounded received extent |
| `+0x1c` | first on-air Link Layer PDU octet |
| `+0x1d` | Link Layer PDU payload-length octet |
| `+0x1e` | first Link Layer PDU payload octet |

The raw PDU location is not inferred from a diagnostic string.
`r_ble_lll_scan_copy_into_mbuf` passes packet `+0x1c` to the complete common
`r_ble_lll_rxpdu_copy` body and supplies exactly the `+0x1d` length plus the
two-byte advertising header.  The open driver can therefore parse the PDU
directly from its typed packet owner and does not need an `os_mbuf` clone.
Address kind is the standard TxAdd header bit and AdvA is carried by the first
six payload octets; those are portable LL semantics, not additional S31 SRAM
fields.

The complete recycle path is also closed:

1. `r_ble_lll_get_rxed_buffer` starts at link-state `+0x68`, rejects a header
   whose completion gate is still clear, validates the packet sentinels and
   active memory epoch, and returns only the corresponding header;
2. `r_ble_lll_scan_recycle_buffer` passes that header to
   `r_ble_lll_scan_rx_process` and never requires a vendor ULL allocation to
   read a legacy PDU;
3. after successful processing, `r_ble_lll_append_rx_buffer` restores both
   packet sentinels, clears the completion gate, reconnects the header after
   link-state `+0x70`, and updates the visible RX-link snapshot before another
   node is consumed.

The vendor's global RX-link object is software bookkeeping, not hardware
descriptor storage.  Complete current `r_ble_lll_update_global_rxlink` maps
scanner kind two to selector one.  Complete current
`r_ble_lll_update_global_rxlink_params` snapshots link-state reserve/head/tail
positions `+0x78/+0x68/+0x70` into that software object and performs stable
reads of selector one's current pointer; it does not publish a new MMIO
pointer.  Complete current `r_ble_lll_mmgmt_reset_rxlink` closes the cold
publication order for selectors one and two: after clearing the first
header's links and installing it as the software head/tail, it publishes that
header to `CurrentRx` and then publishes zero to `NextRx`.  The open driver
should therefore own a small fixed RX chain and reproduce that ordered
selector-one transaction through restricted PAC accessors.  The general
allocator, reference counts, callbacks and `os_mbuf` conversion are
deliberately excluded.

## Open architecture

The first implementation should contain these owners, in this order:

1. a private, pinned scanner arena with affine CPU-owned, published, running,
   completed and reclaimed states;
2. a restricted-PAC scan-start transaction and stable read accessor for the
   existing scan hardware snapshot;
3. a controller role that schedules only passive LE 1M windows on channels
   37, 38 and 39 and uses the existing list/interrupt runtime;
4. a portable LL parser that accepts bounded legacy advertising PDUs and owns
   duplicate-filter policy independently of the hardware codec;
5. the existing `bt-hci` command/event types for LE Set Scan Parameters, LE Set
   Scan Enable and LE Advertising Report.

No upper layer may construct register images.  No public LL type may contain
the vendor link-state words.  SRAM masks remain private implementation details
of typed memory accessors, just as for the DTM descriptors.

## Return gate from research to implementation

Implementation can resume as soon as the focused
`ble-legacy-passive-scanning` Blobray scope closes these four facts for one
legacy passive 1M event:

- the mandatory link-state and scheduler-item fields that select RX and a
  primary advertising channel;
- ~~the scanner RX header/payload topology and selector-one routing~~ -- closed
  by the complete common allocation, reset, removal and append bodies above,
  including the cold `CurrentRx=head` then `NextRx=zero` publication order;
- ~~the completion status that transfers one received buffer back to the
  CPU~~ -- closed by the header completion gate plus packet sentinel and epoch
  checks;
- ~~the packet bytes, length, address type/address and RSSI locations required
  for one standard HCI LE Advertising Report~~ -- closed at packet
  `+0x1c/+0x1d/+0x1e` and signed receive byte `+0x0f`.

Only the first item remains a research blocker.  Once its passive-1M reset
image is reduced, implementation should immediately resume with the fixed
typed RX arena and return to research only if that concrete lower transaction
exposes another missing field.

Exact extended-PHY, active-scan request/response, duplicate-cache and vendor
timer/callout behavior are explicitly deferred.  They do not block the first
passive-scanning vertical slice.
