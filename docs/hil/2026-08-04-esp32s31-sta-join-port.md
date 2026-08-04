# ESP32-S31 production STA join port qualification

Qualification ID: `HIL_ESP32S31_STA_JOIN_PORT_2026_08_04`

Scenario: `radio` / `open-radio-hil`  
Profile: `psram-code-psram-data`  
Device: ESP32-S31 revision 0.0  
Runtime CRC32: `080db958`  
Application image: 1,203,712 bytes  

This cell requalifies the Authentication/Association owner boundary after the
ESP32-S31 DMA/parser/TX composition moved out of `radio_hil.rs` into
`Esp32s31StaJoinPort`. The production port now owns:

- the stable pre-connected RX owner and DMA storage binding;
- descriptor-to-management-MPDU extraction and terminal-frame retention;
- selected-peer RX-filter preparation;
- Open Authentication request publication;
- Association PHY selection, the calibrated rate-16-through-25 power vector,
  HE Power Capability and HE UL-MU Power Capability construction;
- Association request publication and return of the exact RX frontier.

HIL supplies only the current PAC/control-TX owners, fixed scratch storage,
station/candidate policy and a diagnostic observer. The observer records the
post-TX register snapshot and the already-selected HE power profile; neither
callback performs or wraps a driver transaction.

The host contract suite ran all 112 tests in
`open-esp-radio-esp32s31-wifi-embassy`. The focused port tests prove HE power
derivation and the ordered start/auth/association/service/stop boundary with
diagnostics kept on an independent observer.

`cargo hil build radio` passed both placement and autonomous-source-graph
audits. The image was flashed through `/dev/ttyACM0`, then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three requested cycles. Each generation performed a 13-channel
running scan with 13 successful Probe transmissions and zero Probe failures,
fresh Open Authentication, accepted HE20 Association, WPA2 Message 1 through
Message 4, and entry into a new connected epoch. The RX descriptor base stayed
`0x2f03ec50`; every returned connected epoch reported an empty queued-frame
frontier. Observed reconnect Authentication was 52 ms in each cycle and
Association was 21 ms in each cycle. The advertised HE power tuple was
minimum -11 dBm, maximum 20 dBm and relative rate-16 vector
`[0, 0, 0, 1, 1, 2, 2, 4, 5]`.

The transient UART evidence had SHA-256
`ddd9da5ab5561750c90c023e9795074c847918d2533c4db1e9e2833a1a256c24`.
Network credentials were provisioned over the typed HIL protocol and are not
part of the firmware image or this report.

This proves the concrete ESP32-S31 join port and repeated resource handoff. It
does not yet prove that WPA2 receive/transmit/key-install composition is free
of HIL driver logic, nor real AP-loss recovery or injected join failures.
