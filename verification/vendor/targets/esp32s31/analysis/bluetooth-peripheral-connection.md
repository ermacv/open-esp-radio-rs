# ESP32-S31 BLE peripheral-connection hardware boundary

This note records only reviewed hardware-facing facts needed by the Rust
driver. It is not a plan to reproduce the vendor Controller architecture.
Portable Link Layer policy remains in `driver/bluetooth/ll`; this document
identifies the S31 controller-SRAM fields and event edges that a chip backend
must lower.

## Current evidence

The current `libble_app.a` connection functions are rooted by the
`ble-peripheral-connection-hardware` Blobray scope. The corresponding current
symbols are recorded in `functions/reviewed.toml` and can be inspected without
exporting a disassembly dump:

```console
target/blobray/blobray inspect function \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  ble-controller:r_sym_ble_2ZQ4FJGb6hQUDPQ9jp4a --full
target/blobray/blobray inspect function \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  ble-controller:r_sym_ble_DCD5eVhcHQ9ueSpewKn1 --full
```

`r_sym_ble_2ZQ4FJGb6hQUDPQ9jp4a`, reviewed as
`ble_lll_conn_reset_link_state`, proves two direct semantic transfers from the
connection state into the private `0x84`-byte link state:

| Semantic value | Connection state | Link state | Reviewed transform |
| --- | ---: | ---: | --- |
| CRC initialization | `+0x74` | `+0x2c` | low 24 bits are copied; the high byte remains owned by other link-state flags |
| Access Address | `+0x70` | `+0x38` | the complete 32-bit value is copied |

The named same-chip connection-input path independently shows that these
connection-state values originate from the Access Address and CRCInit octets
of `CONNECT_IND`. The Rust memory boundary therefore accepts their wire-order
octets and installs only those two reviewed fields. No integer link-state image
is accepted from the portable LL or exposed back to it.

The rest of `ble_lll_conn_reset_link_state` also initializes packet-length,
power, CTE, PHY-policy and private configuration fields. Those writes are not
yet assigned a source-owned connection-event contract and are deliberately not
part of the Rust identity transition.

The separate `ble-connectable-advertising-response` scope closes the causal
path into that transition. Current `ble_lll_adv_rx_process` reads the received
packet owner, passes its PDU at the reviewed advertising-packet boundary and
converts the descriptor's captured time word at packet-prefix offset `+0x10`
with
`ble_phy_get_actual_tx_time`. That helper first converts controller ticks to
microseconds, then subtracts two PHY-mode-indexed calibration terms. The
result is an on-air packet-start time, not a packet-end time and not an
ordinary observation of controller `now()`.

The controller-memory codec now copies that word into an opaque
`BluetoothLePacketCapturedTime` beside the received PDU and RSSI. It exposes
no field mask or scheduler-time claim. The published task service now performs
the only permitted conversion: the opaque value enters the retained S31
scheduler epoch without re-anchoring it and then the initialized PHY
calibration. The result is a single-use `BluetoothLe1MPacketStartTiming`; no
raw tick or scheduler image escapes that operation.

The calibration is not implicit zero state. Current `ble_phy_module_init`
copies three separately owned tables into the BLE PHY environment before the
normalizer can use them: the 40-channel frequency mapping, PHY-mode packet
prefix airtime and the receive address-capture delay. The channel mapping and
packet-prefix airtime are derived in Rust from the LE channel/PHY definitions;
only the S31 receive-capture delay remains a reviewed chip fact. The memory
owner keeps the resulting tables private and exposes a value-only LE 1M
normalization operation, so neither an extracted table nor its positional
indices cross into the Link Layer.

Current `ble_ll_adv_rx_pkt_in` routes PDU type 5 to
`ble_ll_adv_conn_req_rxd`; after address/filter admission,
`ble_ll_conn_peripheral_start` parses the request and reaches
`ble_ll_conn_created`. For a legacy primary-channel `CONNECT_IND`, connection
creation derives the first anchor as:

```text
normalized packet start
  + CONNECT_IND on-air duration for the received PHY
  + WinOffset * 1.25 ms
  + 1.25 ms
```

The first receive-window width is `WinSize * 1.25 ms`. This proves that the
portable LL may own PDU validation, address admission and transmit-window
arithmetic, while the S31 backend must supply a typed, PHY-calibrated packet
time. Exposing the raw descriptor time word or substituting a later live clock
sample would move an unresolved hardware transform into protocol code.

That boundary is now implemented. Portable LL publishes the protocol-derived
LE 1M `CONNECT_IND` airtime and relative WinOffset/WinSize positions. The S31
connection runtime consumes the single-use packet-start value, adds the packet
airtime and relative positions with wrapping scheduler semantics, and retains
the resulting absolute first window beside the still-unsubmitted connection
event and identity-prepared SRAM graph. Cancellation returns both the pristine
graph and event counter zero; there is no `now()` input on this path.

Exact normalized-body correspondence between the current obfuscated archive
and the older named same-chip archive identifies
`r_sym_ble_DCD5eVhcHQ9ueSpewKn1` as `ble_lll_conn_peripheral_new`. Its current
ESP32-S31 body proves that the first event additionally depends on all of the
following:

- a live controller-time observation used with the connection interval to
  derive the first anchor;
- a separate prepared start/end scheduler window;
- the selected data channel lowered to radio frequency and PHY/rate fields;
- scheduler insertion, conflict handling and a role-specific completion
  callback;
- post-insertion retry/reschedule policy before the device is reported as
  connected.

The same body closes a useful descriptor subset without requiring names for
the vendor's private aggregate types. The Rust memory codec now consumes only
semantic values and performs these positional transforms privately:

| Private object field | Source-owned input | Reviewed first-event behavior |
| --- | --- | --- |
| link state `+0x00` | owned empty TX sentinel | stores the compressed endpoint, 251-octet S31 capability and the two transmit-path ready states |
| link state `+0x04` | signed default TX power | shared S31 five-bit rounded-power projection |
| link state `+0x08` | owned initialized RX pool | stores the compressed head and initial unconsumed receive sentinel |
| link state `+0x0c` | S31 baseline control policy | installs the duplicated value 2 and makes that policy active |
| link state `+0x18` | negotiated connection interval | interval converted as a duration into raw controller ticks |
| link state `+0x14`, `+0x1c`, `+0x20`, `+0x30` | new unencrypted connection | clears packet history/control state and installs the recovered initial sequence profile |
| link state `+0x2c` | CRCInit | preserves the low 24-bit CRC seed and marks that context ready |
| link state `+0x50` | powered epoch's opaque global workspace link plus S31 common-radio policy | retains the default value 3, stores the compressed `workspace + 8` endpoint, clears the separate four-bit mode and marks the direction-finding configuration ready |
| link state halfword `+0x56` | disabled-CTE ordinary-role policy | preserves the reviewed low flags, clears the unsupported mode region and installs the disabled baseline |
| link state `+0x60` | S31 first-event conflict policy | starts at 13; later conflict handling increases it and saturates at 15 |
| scheduler item `+0x04` | ready state | sets the reviewed context-ready flag |
| scheduler item `+0x14` | LE 1M plus rounded TX power | selects the LE 1M rate lanes and copies the rounded-power projection |
| scheduler item `+0x18` | data-channel index plus bounded priority | maps data channels 0--36 to the S31 frequency image and copies the four-bit priority into both lanes |
| scheduler item `+0x2c`, `+0x2e` | first transmit-window width plus a symmetric timing guard | stores the short receive-wait duration and its fixed mode; the descriptor image remains private to the memory codec |
| scheduler item `+0x38` | new event | clears the initial status |
| scheduler item `+0x44`, `+0x48` | resolved common-scheduler window | stores the accepted start and end only after overlap resolution |
| scheduler item `+0x4c` | new event | clears the reviewed low bookkeeping byte |

The priority is no longer an application-supplied integer. The retained
current options object and the older named options object are byte-identical:
the first-event transform maps its priority input to 13 and its common-radio
policy input to 3. These scalars are reviewed chip policy inside the backend.
The channel-frequency mapping is the ordinary LE data-channel ordering around
the three primary advertising-channel positions; the portable LL still sees
only a validated data-channel index. Likewise, signed dBm, interval and a
non-empty wrapping window are the only dynamic inputs visible above the memory
crate. Masks, shifts, rounded-power values and SRAM offsets do not leave that
codec.

The current and named older S31 `ble_ll_conn_created` bodies additionally prove
that the first scheduler reservation does not end at the upper edge of the
transmit window. For LE 1M it retains another 5,154 microseconds of event time
and a one-unit boundary guard. The source-owned backend now preserves that
complete reservation and begins it before the receive anchor by the common
preparation lead plus the open NimBLE 16-microsecond uncertainty guard and one
boundary unit. The portable LL still owns only WinOffset and WinSize.

This also agrees with the architectural split in
[public Espressif NimBLE](https://github.com/espressif/esp-nimble/blob/916be244a9c646bc16fd65507478cf3fe717d8ed/nimble/controller/src/ble_ll_conn.c#L2868-L2873):
its first peripheral event separately retains `periph_cur_tx_win_usecs` and a
connection-event deadline based on `BLE_LL_CONN_INIT_SLOTS`. The open source is
used to identify the two scheduling concepts, not to import its NPL scheduler
or make the vendor's connection aggregate an ABI.

The complete `ble_lll_conn_peripheral_new` body derives its receive-wait value
as `WinSize * 1.25 ms + 2 * timing_guard + 61 us`. Every valid legacy
transmit window fits its short descriptor form, whose encoding is now private
to `BluetoothPeripheralConnectionReceiveWait`. No upper layer accepts the
duration/configuration word.

The event remains deliberately CPU-owned. Complete current and named
same-chip direction-finding bodies prove that ordinary advertising, sync and
connection link states all retain one controller-global `0x20`-byte
environment even when IQ sampling is disabled. The open driver now claims and
initializes that separate static workspace before MMIO, publishes its disabled
descriptor through CTE buffer zero, clears software ownership through generated
PAC accessors and retains the joined SRAM/HAL owner for the complete powered
epoch. It does not reproduce the vendor allocator or make the workspace
connection-private. A distinct memory-layer transition now consumes the
resulting opaque environment link, privately installs the `workspace + 8`
configuration endpoint and adjacent disabled-CTE policy, and returns a new
affine graph state. The task-side transition first reserves the requested
window in the common timeline, obtains the separate sequence-deadline
observation, and only then writes the overlap-resolved window into the private
descriptor. Cancellation at either CPU-owned frontier restores the timeline,
graph and protocol event. In particular, the descriptor uses the common
scheduler's resolved window rather than the requested window: overlap
insertion is allowed to displace the initial candidate.

Exact correspondence also identifies the current allocation suffix with the
named same-chip `ble_lll_conn_slave_new`. The connection link state's selected
scheduler head is the item at the private free-list head. Allocation reads that
item's compressed predecessor, advances the private head to the predecessor,
detaches the selected item and passes only that item to the common scheduler;
it does not publish the complete two-item private chain. The memory codec now
models that transition explicitly. The task service can join the detached
item to its exclusive empty common list, and a failed join or pre-publication
cancellation restores both the selected item's predecessor and the original
private head. No compressed pointer or list word leaves the memory crate.

These dependencies explain why Access Address plus CRCInit is not a runnable
event image. The Rust owner now also attaches a separate statically allocated
two-node RX rotation graph. That pool represents the common non-scanning
selector-two class rather than a connection-private vendor allocation, so a
future response-capable advertiser can transfer the exact affine owner after
accepting `CONNECT_IND`. Pre-publication cancellation clears the link-state RX
endpoints and recovers both the pristine connection allocation and the intact
pool. The resulting graph reaches a reversible common-list merge and now
crosses the complete first publication prefix. Before the first
irreversible MMIO, the common-list identity and encoded HEAD are validated
together. The infallible suffix publishes the pool through the non-scanning
selector-two PAC/HAL accessor, publishes that exact item as the common
scheduler HEAD, prepares dynamic interrupts and consumes the matching RUN
proof into hardware-owned connection state. No raw selector, address or
register image crosses the memory/HAL boundary. There is deliberately no
software rollback after selector-two or HEAD becomes hardware-visible;
completion now consumes the affine fenced list-zero observation and classifies
only in-flight, zero and opaque-nonzero status. It keeps the graph, RX
publication, portable event and timeline reservation hardware-owned. The
fresh hardware-head observation, atomic source-list unlink/mailbox arm and
finite interrupt-or-direct removal gate now retain the same owners through a
removal-ready state. The lower memory boundary now binds that exact proof,
copies every contiguous completed RX PDU before mutation and restores only the
event-local scheduler item and receive rotation. It deliberately preserves the
live connection link state. Scheduler timeline/list release still gates CPU
ownership at the role boundary and the later protocol-state advance.

Production retention now treats that graph and pool as one reusable affine
allocation. Cold start binds the physical default transmit-power policy once,
and the sole task runtime must check out both owners before preparing an event.
The runtime slot remains observably vacant until cancellation or a future
completed recycle returns the same pair of opaque storage identities. A
foreign graph or RX pool cannot fill the slot, even if a native model assigned
it the same synthetic controller address. Thus no borrowed production runtime
can copy, replace or silently recreate connection memory while an event owns
it.

The same exact correspondence identifies
`r_sym_ble_1KGaCqPI03xSu9c6Rh0G` as `ble_lll_conn_update_link_state`. Together
with the default and custom aborted-opcode writers, its complete body replaces
the former opaque word at `BTMAC_BLE_PHY_INIT + 0x4ac` with a narrower reviewed
contract. Bit 0 selects the custom aborted-opcode path. Link-state refresh
independently replaces bit 1, clearing it exactly when both private connection
flags are set and setting it on every other path. The register is therefore
published as `CONNECTION_ABORT_CONTROL` with field accessors; the inner
hardware meaning of bit 1 remains deliberately unnamed.

This MMIO fact is not a blocker for an unencrypted first event. It belongs to
the later connection link-state refresh/encryption transition, which must
consume semantic connection state and choose one PAC accessor. No raw
register image or private vendor flag layout is allowed to cross into the
portable Link Layer.

## Completion recycle and recurring-event facts

Current-to-named correspondence also identifies the complete connection-event
suffix:

- `r_sym_ble_vssfeWXnPIcnyOfdsX00` is the current
  `ble_lll_conn_recycle_sch_item`;
- `r_sym_ble_Dugdawung2wRHuTkJeRz` is the current
  `ble_lll_conn_sched_next_anchor`;
- `r_sym_ble_bp2AWMfX9zEpJsmzOTUB` is the current
  `ble_lll_conn_reschedule_event`.

The scheduler-item status is not a success/error boolean. Zero takes one quiet
bookkeeping path. A nonzero value adds diagnostic accounting and then exposes
independent flag branches; both zero and nonzero items are returned to the
private free list, and either may later continue or destroy the connection
according to separate connection state. The Rust status must therefore remain
`Zero` versus opaque `NonZero`, and LL event completion must not be selected by
that distinction alone.

The recycle body first pushes the completed item onto the connection-private
free list: it preserves the allocation prefix, installs the former private
head as predecessor, then makes the completed item the new private head. The
reschedule body pops that same item, clears its status and rebuilds its event
fields; failed insertion pushes it back. This confirms that completed common
list removal returns an event-local scheduler item while preserving the live
link state.

Recurring events use a different profile from the first event:

- baseline priority is 8, with conflict escalation saturating at 15;
- the receive wait includes accumulated window widening and controller-owned
  guard terms and selects a short or long descriptor form;
- the proposed anchor advances from the prior nominal anchor, never from a
  fresh `now()` sample;
- missed intervals advance the proposed event counter by `skipped + 1`, while
  admission retries remain provisional until one candidate commits.

The link-state timing word at the reviewed `+0x34` position is a hardware
input/output location, not an ordinary persistent field. Current
`r_sym_ble_DCD5eVhcHQ9ueSpewKn1` and named `ble_lll_conn_slave_new` write the
connection interval minus the common 440-microsecond LLL reserve, converted to
scheduler ticks, before the first event. Current
`r_sym_ble_bp2AWMfX9zEpJsmzOTUB` and named `ble_lll_conn_reschedule_event`
restore the same input for recurrence. After completion, current
`r_sym_ble_vssfeWXnPIcnyOfdsX00` and named `ble_lll_conn_recycle_sch_item` pass
the location to the reviewed `ble_phy_get_actual_tx_time` operation. That
operation interprets the value as a captured receive time, projects it into
Controller microseconds and removes two PHY-mode calibration terms to recover
an on-air packet-start reference. The CPU does not write the location between
publication and this read.

The memory API now models that reuse as three ownership phases: required
semantic event-span input while CPU-owned, hardware-owned storage after RUN,
and an opaque captured-anchor observation after fenced removal. The raw word
and its positional reuse remain private to the codec. The capture is not the
first completed RX packet timestamp and remains available even when the copied
RX batch is empty.

The chip timing path now consumes the whole recycled owner into a distinct
packet-start-normalized typestate. It projects the connection capture through
the retained scheduler epoch and the existing LE 1M PHY calibration without a
fresh time sample or epoch reanchor. An unavailable epoch returns the complete
unchanged recycled owner. The transition retains status, RX batch, memory and
the in-flight portable LL event without interpreting or advancing any of them.

The first completion must correct the recurring phase from the captured
packet-start. On the software window-widening path, the actual start replaces
the committed anchor, the proposed anchor moves by the same delta, the
fractional residual and accumulated guard reset, and the actual start becomes
the widening reference. The separately identified mode flag selects automatic
window widening. The initial source-owned driver may support only the software
path and fail closed if automatic widening is unexpectedly active; the raw
vendor flag must not enter portable LL.

That last branch exposes a portable-LL gap: the current one-event completion
transition advances the channel selector and counter by exactly one. Recurrence
needs a distinct provisional skipped-event transition which advances protocol
time/channel selection without falsely claiming that the skipped intervals ran
in hardware. Scheduler admission may retry proposed anchors, but only the
accepted recurrence may commit that proposed LL state.

## Next closure order

Packet timestamp conversion, the causal absolute first-window contract, the
reviewed channel/interval/power/priority/window descriptor subset and
task-side common-timeline admission are closed. The retained scheduler epoch
projects the window into a validated raw Controller interval and converts the
negotiated interval independently as a duration, avoiding subtraction of two
truncated absolute projections. The selected private scheduler item is now
detached and reversibly merged into the exclusive common list. Event-local
scheduler and RX memory plus the exact timeline/list ownership are now
reclaimed after completion. The shortest remaining path to a recurring
peripheral connection is:

1. attach the now-static shared RX pool to the response-capable
   connectable-advertising graph, then transfer the pool and accepted packet to
   the existing task-service normalizer;
2. classify the independent event/destroy branches and commit exactly one
   completed portable LL event where continuation is valid;
3. correct the planned phase to the normalized actual packet start and add
   recurrence through a provisional
   `skipped + 1` LL transition and the same typed publication path;
4. add SN/NESN, retransmission and supervision before exposing ACL success;
5. add only the mandatory LL control procedures needed by the supported HCI
   surface.

Continuation/destroy classification and next-anchor proposal are now the
shortest closure roots. They do not block the already
source-ordered preparation, publication, fenced completion, post-unlink and
CPU-recovery path, but they do block recurrence of the first hardware-owned
event.
