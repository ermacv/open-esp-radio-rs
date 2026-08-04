# ESP32-S31 connected TX reset frontier

Date: 2026-08-04

Evidence ID: `HIL_ESP32S31_STA_TX_FAULT_2026_08_04`

## Cell

- target: ESP32-S31 revision 0.0;
- image: `open-radio-station-tx-fault`;
- runtime CRC32: `b12621f7`;
- application image size: 1,194,640 bytes;
- memory profile: PSRAM code/data with ISR, DMA and stack placement audited in
  internal SRAM;
- peer: repository-controlled WPA2 HE20 AP on channel 11;
- protocol: v8 typed station fault command/evidence;
- injection: one connected network TX, after the production backend returned
  `Pending` from real descriptor publication and before its next service edge.

The HIL-only backend decorator replaced that next service wake with the
impossible simultaneous `TX_COMPLETE | TX_TIMEOUT` image. It did not create a
synthetic lifecycle failure and did not bypass descriptor ownership. The
ordinary or aggregate production transaction therefore had to execute its
normal conflicting-interrupt path and call `require_reset` on the live slot.

## Commands

```text
cargo hil flash station-tx-fault
OPEN_RADIO_HIL_STA_SSID=... OPEN_RADIO_HIL_STA_PASSWORD=... \
  cargo hil station tx-fault --timeout-seconds 60
```

The host runner then reset the same image and required a new network-ready TX
service while leaving the controlled AP running.

## Typed result

```text
injection=ConnectedTxAfterPublication
classification=RadioResetRequired
runner_returned=true
executor_tasks_stopped=true
rx_dma_stopped=true
tx_owner_reset_required=true
```

The first three returned owners are safe for diagnostic/platform reset work.
The TX owner is deliberately not returned as idle: its hardware-visible slot
remains quarantined and cannot enter reconnect teardown. A cold USB reset then
completed PHY/MAC initialization, Association, WPA2 and `udp-tx-ready` again.
Neither captured boot contained a `result=FAIL` or panic marker.

Artifact SHA-256:

```text
evidence.txt       8df7bef088afb6182758f106ec211186f1f295d9f99d5c5b667d51df96e78f8c
uart-fault.log     193416a6c6e22f10da97103dc2c0418dcb00da386e2b00d286b6d58c4d791cf7
uart-recovery.log  d9ce0bff4f1120284cfe90fd7a4ab512317d5cfe357478e52f83a96337b5abeb
```

## Boundary

This cell proves one terminal connected-TX ownership frontier and cold
platform recovery. It does not yet implement an in-place whole-radio reset,
inject RX-DMA failures, or qualify every natural hardware error. Ordinary
timeout/collision retry remains covered by deterministic production unit tests
and existing traffic cells; it is not relabeled as this reset-required case.
