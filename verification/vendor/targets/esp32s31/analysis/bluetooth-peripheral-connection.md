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
converts the descriptor's captured time word with
`ble_phy_get_actual_tx_time`. That helper first converts controller ticks to
microseconds, then subtracts two PHY-mode-indexed calibration terms. The
result is an on-air packet-start time, not a packet-end time and not an
ordinary observation of controller `now()`.

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

`r_sym_ble_DCD5eVhcHQ9ueSpewKn1`, reviewed as
`ble_lll_conn_peripheral_new`, proves that the first event additionally depends
on all of the following:

- a live controller-time observation used with the connection interval to
  derive the first anchor;
- a separate prepared start/end scheduler window;
- the selected data channel lowered to radio frequency and PHY/rate fields;
- scheduler insertion, conflict handling and a role-specific completion
  callback;
- post-insertion retry/reschedule policy before the device is reported as
  connected.

These dependencies explain why Access Address plus CRCInit is not a runnable
event image. The current Rust identity-prepared owner has no scheduler
publication or `RUN` transition and can only be cancelled back to the pristine
allocation.

## Next closure order

The shortest path to one real peripheral event is:

1. publish a typed S31 PHY-calibrated packet-start observation from the
   response-capable advertising RX owner;
2. define the first-anchor and scheduler-window contract from that observation
   plus the portable PDU-airtime and transmit-window calculation;
3. close the remaining connection link-state and scheduler-item fields as
   semantic accessors inside the memory crate;
4. join the prepared graph to the existing common scheduler admission,
   publication, completion and post-unlink owners;
5. add SN/NESN, retransmission and supervision before exposing ACL success;
6. add only the mandatory LL control procedures needed by the supported HCI
   surface.

The completion and next-anchor functions remain important evidence roots, but
they do not block the identity-only preparation implemented now.
