# ESP32-S31 associated-peer port qualification

Qualification ID: `HIL_ESP32S31_STA_PEER_PORT_2026_08_04`

Scenario: `radio` / `open-radio-hil`  
Profile: `psram-code-psram-data`  
Device: ESP32-S31 revision 0.0  
Runtime CRC32: `bf8e8ead`  
Application image: 1,194,640 bytes  

This cell requalifies the scan-to-connected peer boundary after its remaining
driver policy moved from `radio_hil.rs` into the production
`Esp32s31StaPeerPort`. The port now owns both finite transactions around
Association:

- derive `StaPeerScanPolicy` from the selected immutable `ScanRecord`;
- install scan-time HT A-MPDU, HE BSS-color and optional WMM policy before the
  Authentication/Association runners use the candidate;
- retain an opaque prepared-peer token across the protocol transaction;
- combine that token, the accepted Association Response, selected PHY and
  observed noise floor into one `StaPeerAssociationPlan`;
- install association-response WMM, HE peer/AID/BSR state and beamforming rate
  control in recovered order;
- return `Esp32s31ConnectedStaPeer`, containing the stable connected-link facts
  and the exact initialized rate-control owner, plus a value-only diagnostic
  report.

The initial and reconnect HIL paths now invoke the same port. They no longer
construct either peer policy, call `program_he20_peer_state`, program rate
control, or manufacture their own connected-link value. HIL only maps a typed
port failure to its outer lifecycle disposition and prints the returned report.
The private `StaConnectedLink` type and borrowed rate-control session were
removed from HIL; the connected session carries the production peer owner by
value.

All 114 `open-esp-radio-esp32s31-wifi-embassy` host tests passed. The new mock
hardware test proves the ordered scan-policy, association-policy, HE peer,
association/AID, buffer-status and beamforming programming edges and verifies
the returned HE20 connected owner. `cargo hil build radio` passed placement and
autonomous-source-graph audits without increasing the 1,194,640-byte image.

The image was flashed through `/dev/ttyACM0`, then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three cycles. The first and every repeated epoch completed HE20
Association, WPA2 M1--M4 and connected entry. Each controlled transition ran a
13-channel scan with 13/13 successful Probe transmissions. The peer port
reported initialized Dot11Ax rate control for the first epoch and every
reconnect; the RX descriptor base remained `0x2f03e9d0`, and each returned
frontier had zero queued frames.

The transient UART evidence had SHA-256
`b8639a6daeef549d23c9cb3a62ce893ffb091a528031ef25fea93530a24aaf76`.
Credentials were runtime-provisioned and are absent from the image/report.

This proves production ownership of ESP32-S31 selected-peer policy and hardware
programming across repeated epochs. It does not yet prove the remaining
connected-entry resource composition outside HIL, real AP-loss recovery or
injected programming failures on hardware.
