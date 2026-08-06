# ESP32-S31 recoverable connected RX fault frontier

Date: 2026-08-06

Evidence ID: `HIL_ESP32S31_STA_RX_FAULT_2026_08_06`

## Cell

- target: ESP32-S31 revision 0.0;
- scenario: `radio` / `open-radio-hil`;
- memory profile: PSRAM code/data with ISR, DMA and stack placement audited in
  internal SRAM;
- peer: external WPA2 HE20 AP;
- protocol: v9 discriminated station fault evidence;
- injection: one real completed connected-RX DMA unit after ownership transfer
  from the walker and before staging allocation/copy.

The driver-owned `Esp32s31RxStageAdmissionPolicy` receives only value metadata
for a completed unit and may lower its admitted payload length. It cannot see
or mutate a descriptor, retain payload memory, recycle DMA buffers or publish
a frame. The HIL policy lowered the limit to zero once, so the real non-empty
unit crossed the production `TooLong` path. That path recycled the complete
descriptor chain and waited for reload before reporting the discard.

The default production policy is zero-sized and statically dispatched. The
HIL state machine is supplied only by the HIL composition and is not a `dyn`
observer or a post-`service_rx()` error decorator.

## Commands

```text
cargo hil flash radio --port /dev/ttyACM0
OPEN_RADIO_HIL_STA_SSID=... OPEN_RADIO_HIL_STA_PASSWORD=... \
  cargo hil station rx-fault --serial /dev/ttyACM0 --timeout-seconds 90
```

Credentials remained host-owned and were provisioned over the framed UART
protocol.

## Typed result

```text
injection=ConnectedRxBeforeStagingOverCapacity
classification=RecoverableFrameDiscard
descriptor_reloaded=true
following_unit_staged=true
same_ring_live=true
service_result_ok=true
station_epoch_completed=true
post_reconnect_udp_rx_bytes=31744
post_reconnect_udp_rx_datagrams=124/124
```

The post-reconnect session reported zero transport errors, zero queue drops,
zero RX buffer-full events and zero FIFO-overflow events. All 124 measured
frames carried protocol-validated A-MPDU containment. The application image
remained 40.06% of its partition and passed both placement and autonomous
source-graph audits.

## Boundary

This cell proves recoverable frame discard and continued use of the same live
RX ring. It does not claim that reload timeout, a corrupt ring or disagreement
between descriptor metadata and backing storage is recoverable. Those cases
cross an ambiguous ownership frontier and must remain reset-required.

The next recovery qualification is an in-place platform radio reset from the
already typed connected-TX quarantine frontier. It must consume the exact
quiesced IRQ/hardware/halted-RX/quarantined-TX owner bundle, prove MAC/baseband
reset, release TX resources only with an unforgeable reset-complete token, and
reach a fresh station generation without resetting the whole SoC or USB link.
