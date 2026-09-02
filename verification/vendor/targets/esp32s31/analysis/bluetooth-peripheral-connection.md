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

These dependencies explain why Access Address plus CRCInit is not a runnable
event image. The Rust owner now also attaches a separate statically allocated
two-node RX rotation graph. That pool represents the common non-scanning
selector-two class rather than a connection-private vendor allocation, so a
future response-capable advertiser can transfer the exact affine owner after
accepting `CONNECT_IND`. Pre-publication cancellation clears the link-state RX
endpoints and recovers both the pristine connection allocation and the intact
pool. The resulting graph still has no scheduler publication or `RUN`
transition.

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

## Next closure order

The first two former blockers are closed: packet timestamp conversion and the
causal absolute first-window contract. The retained scheduler epoch now also
projects that window back into a validated non-empty raw Controller interval,
including the common preparation lead and preserving cancellation. This is a
CPU-only candidate; it is not yet composed into task-side admission. The
shortest remaining path to one real peripheral event is:

1. attach the now-static shared RX pool and selector-two RX publication to the
   response-capable connectable-advertising graph, then transfer the pool and
   accepted packet to the existing task-service normalizer;
2. close the remaining connection link-state and scheduler-item fields as
   semantic accessors inside the memory crate, then consume the existing raw
   candidate from task-side admission;
3. join the prepared graph to the existing common scheduler admission,
   publication, completion and post-unlink owners;
4. add SN/NESN, retransmission and supervision before exposing ACL success;
5. add only the mandatory LL control procedures needed by the supported HCI
   surface.

The completion and next-anchor functions remain important evidence roots, but
they do not block the now timing-complete, still unpublished first-event owner.
