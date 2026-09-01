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
- the scanner RX header/payload topology and selector-one current/next
  publication order;
- the completion status that transfers one received buffer back to the CPU;
- the packet bytes, length, address type/address and RSSI locations required
  for one standard HCI LE Advertising Report.

Exact extended-PHY, active-scan request/response, duplicate-cache and vendor
timer/callout behavior are explicitly deferred.  They do not block the first
passive-scanning vertical slice.
