# ESP32-S31 connected epoch transition qualification

Qualification ID: `HIL_ESP32S31_CONNECTED_EPOCH_TRANSITIONS_2026_08_04`

Scenario: `radio` / `open-radio-hil`
Profile: `psram-code-psram-data`
Device: ESP32-S31 revision 0.0
Runtime CRC32: `c51449b4`
Application image: 1,203,712 bytes

This cell requalifies repeated connected epochs after the remaining RX-entry
and connected teardown driver sequencing moved out of `radio_hil.rs`.

The production Embassy integration crate now owns:

- a consuming pre-connected-to-live RX transition which accepts an already
  live WPA2 frontier, starts only halted/prepared frontiers and returns the
  exact owner on error;
- initial construction of `Esp32s31RxEpochResources`, so initial and reconnect
  paths converge on the same persistent RX resource owner;
- `Esp32s31StaTxEpoch`, which retains immutable control-TX construction policy
  while the connected runner owns the descriptor and rejects overwrite or
  duplicate take/restore operations;
- a named connected-TX return containing descriptor resources, sequence owner,
  pairwise-key authority and descriptor-only aggregate storage;
- one bounded pairwise/group CCMP clear transaction in the recovered group-
  then-pairwise order;
- `Esp32s31ConnectedStaTeardownPort`, which enforces control shutdown, RX-DMA
  stop, idle TX return and key clearing in that order.

The teardown error is stage-specific. Control failure returns the complete
backend and GTK authority; RX failure returns the shutdown control frontier
with the still-live RX/TX owners; active TX failure returns the stopped RX
frontier with TX and key authorities. No failure path discards a unique owner.

HIL no longer calls `start_with_storage`, `take_live`,
`Esp32s31ConnectedRx::new`, `ConnectedControl::shutdown`,
`ConnectedRx::try_stop`, `ConnectedTx::try_into_teardown_parts`, individual
key-token `clear` methods or `Esp32s31ControlTx::new`. It supplies static
storage and reports the production transaction outcome.

All 121 Embassy-integration host tests and all 79 ESP32-S31 MAC tests passed.
`cargo hil build radio` passed placement and autonomous-source-graph audits.
The owner-preserving failure frontier increased the application image by 9,072
bytes relative to `c7a6b50b`; the resulting image still occupies 38.26% of its
application partition and no DMA/SRAM placement changed.

The image was flashed through `/dev/ttyACM0`, then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three requested cycles. Each stopped epoch reported zero queued
RX frames at the stable descriptor base `0x2f03e9d0`, a zero key-valid bitmap
after clearing pairwise slot 4 and group slot 1, and a returned control TX
owner. Every subsequent running scan, Authentication, Association and WPA2
transaction completed and re-entered HE20 connected service.

The transient UART evidence had SHA-256
`2d595d207ea621725dd1acbfff37dbc1aafc4c8f35dbff0b8a277c3cfb5c0b24`.
Credentials were runtime-provisioned and are absent from the image/report.

This proves production ownership of RX entry and ordered connected teardown
across repeated healthy epochs. MAC interrupt epoch activation/quiescence and
executor acknowledgement of benchmark/protocol task stop remain platform/HIL
composition and are the next extraction boundary. Fault-injected teardown and
real AP-loss recovery remain unqualified.
