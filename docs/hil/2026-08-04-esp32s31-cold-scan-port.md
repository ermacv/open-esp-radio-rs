# ESP32-S31 shared cold/running scan-port qualification

Qualification ID: `HIL_ESP32S31_SHARED_SCAN_PORT_2026_08_04`

Scenario: `radio` / `open-radio-hil`
Profile: `psram-code-psram-data`
Device: ESP32-S31 revision 0.0
Runtime CRC32: `d4b41d11`

The initial cold active scan now uses the same production scan transaction
port as a later connected-station rescan. The two modes retain distinct
hardware owners: cold scan carries `ColdRadioRegisters` and polling-only
`Esp32s31ColdScanTx`, while running scan carries cooperative registers,
stopped connected RX resources and `MacInterruptSetup`-guarded polling TX.
Both modes share channel switching, RX start/observe/stop, active-probe
fallback, dwell timing, next-ring preparation, candidate selection and bounded
telemetry through `Esp32s31ScanPort`.

`radio_hil.rs` no longer implements `Esp32s31StaScanPort` or owns its former
`RadioHilColdScanOwner`. It supplies fixed storage, station policy and a
non-retaining addressed-frame observer, then consumes the returned PHY, PAC,
RX and TX owners. The net change removed 229 lines from the HIL file; the file
now has 6,850 lines including the new cold-scan evidence marker.

All 123 ESP32-S31 Embassy integration host tests passed. Target debug check,
release build, placement audit and autonomous-source-graph audit passed. The
application artifact was 1,260,176 bytes; image-size classification is
deliberately deferred to a separate task and is not used as an acceptance
claim here.

The release image was flashed through `/dev/ttyACM0`. Runtime credentials were
provisioned over HIL protocol and were not embedded in the image or report.
The cold scan returned five records after 32 raw frames and 15 recycled-ring
epochs. All 13 active probes completed, no probe failed, 16 addressed Probe
Responses were observed, and both RX and active-scan gates were true.

Then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three cycles. Every running scan returned the same descriptor
base `0x2f03ea10`, an empty connected RX queue, 13 successful Probe Requests
and a selected candidate which proceeded through fresh Authentication,
Association, WPA2 and connected entry. The UART evidence had SHA-256
`710ad36026441e05a266ade0fed74e26026e158a55ec83594e1774a756e5563d`.

The next driver extraction is the duplicated initial/reconnect pre-connected
attempt composition. HIL still sequences channel retune, Authentication,
Association, peer programming and WPA2 ports separately for those two modes;
their production primitive owners are already shared, but their higher-level
typed transaction is not.
