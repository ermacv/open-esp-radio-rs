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

1. recover the response-capable legacy advertising RX chain that yields one
   exact `CONNECT_IND` and its packet-end controller timestamp;
2. define the first-anchor and scheduler-window contract from that timestamp;
3. close the remaining connection link-state and scheduler-item fields as
   semantic accessors inside the memory crate;
4. join the prepared graph to the existing common scheduler admission,
   publication, completion and post-unlink owners;
5. add SN/NESN, retransmission and supervision before exposing ACL success;
6. add only the mandatory LL control procedures needed by the supported HCI
   surface.

The completion and next-anchor functions remain important evidence roots, but
they do not block the identity-only preparation implemented now.
