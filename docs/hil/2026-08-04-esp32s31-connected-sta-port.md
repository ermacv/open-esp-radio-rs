# ESP32-S31 connected-STA port qualification

Qualification ID: `HIL_ESP32S31_CONNECTED_STA_PORT_2026_08_04`

Scenario: `radio` / `open-radio-hil`  
Profile: `psram-code-psram-data`  
Device: ESP32-S31 revision 0.0  
Runtime CRC32: `c7a6b50b`  
Application image: 1,194,640 bytes  

This cell requalifies the connected entry boundary after rate selection and
the RX/TX/control owner graph moved from `radio_hil.rs` into the production
`Esp32s31ConnectedStaPort`.

The port consumes the exact `Esp32s31ConnectedStaPeer` returned after
Association and owns the following policy and composition:

- validation of aggregate capacity, retry limits, QoS, TX/RX BlockAck windows
  and beacon-loss policy before any unique peer resource moves;
- ordinary and aggregate rate selection from the associated PHY and recovered
  peer capabilities;
- construction of the connected RX dispatcher and staged protocol with the
  selected station address, BSSID, AID and ingress policy;
- the control-TX to ordinary/A-MPDU ownership handoff, including PTK and
  sequence ownership;
- TX/RX BlockAck, initial ADDBA, beacon-loss and reorder-command control;
- assembly of the one production `Esp32s31WifiBackend` accepted by
  `WifiRunner`.

HIL now supplies compile-time scenario values, pinned/static resources, the
`embassy-net` sink, executor placement and diagnostics. It no longer constructs
`ConnectedRxDispatcher`, `Esp32s31ConnectedTx`, `StaTxBlockAckSessions`,
`Esp32s31ConnectedControl` or `Esp32s31WifiBackend` directly. A rejected
configuration returns the exact peer before PTK, sequence or pinned TX storage
can move. A busy control-TX handoff likewise returns every unique owner and
does not allocate.

All 117 `open-esp-radio-esp32s31-wifi-embassy` host tests passed, including
three new connected-port tests for coherent policy, RX/control binding and
owner-preserving rejection. `cargo hil build radio` passed placement and
autonomous-source-graph audits without changing the 1,194,640-byte image.

The image was flashed through `/dev/ttyACM0`, then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three requested cycles. Every epoch reached HE20 connected
service with data rate code `0x17` and aggregate rate code `0x23`. Every stop
returned an RX frontier with descriptor base `0x2f03e9d0` and zero queued
frames, cleared pairwise slot 4 and group slot 1 to a zero valid-key bitmap,
and returned the ordinary TX owner. Each running rescan completed all 13 Probe
transmissions without a TX failure.

The transient UART evidence had SHA-256
`5c84db1d37b1afb5dc4d203ac86533476746c02da5577511b9fe6a1bf77c8c5e`.
Credentials were runtime-provisioned and are absent from the image/report.

This proves production ownership of the connected policy and steady-state
owner graph across repeated epochs. It does not yet prove production ownership
of initial/reconnected RX-DMA activation, MAC interrupt epoch activation, or
the ordered connected teardown/crypto-clear transition; those calls still
form the remaining driver orchestration inside HIL.
