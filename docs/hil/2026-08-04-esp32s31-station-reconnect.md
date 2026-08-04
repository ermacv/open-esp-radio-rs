# ESP32-S31 controlled STA epoch reconnect

Date: 2026-08-04  
Board: ESP32-S31 revision 0  
Scenario: `radio` / `open-radio-hil`  
Profile: `psram-code-psram-data`  
Runtime CRC32: `5d5b575d`

Qualification ID: `HIL_ESP32S31_STA_RECONNECT_2026_08_04`

## Claim

One healthy connected epoch can be stopped at a production runner transaction
boundary, fully torn down, and reconstructed on the same peer without another
PAC singleton or static allocation. The returned cooperative hardware, RX DMA
frontier, network stack, TX/A-MPDU resources, control mailbox, PMK, nonce and
sequence owners complete a second Association, WPA2 four-way handshake and
entry into the production connected runner.

This does not claim recovery from AP disappearance. The controlled run retains
the scanned peer and channel, starts again at Association, and intentionally
distinguishes `WifiRunnerExit::Stopped` from beacon-loss `Disconnected`.

## Procedure

The image was built and flashed with:

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
```

Credentials remained host-owned and were provisioned over the framed UART
protocol. The lifecycle qualification was then run with:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --timeout-seconds 120
```

The host accepts the cell only when protocol v4 advertises station epoch
control, the stop command is acknowledged, the production stop marker is
observed, no reconnect failure marker appears, and the target reaches
`production-reconnect-connected-enter`. The local transcript is written to
`target/hil/esp32s31/qualification/station-reconnect/uart.log`.

## Observed evidence

| Transition | Result |
| --- | --- |
| Initial Open Authentication | PASS, 167 ms |
| Initial Association | status 0, AID 32, HE20/WMM, 21 ms |
| Initial WPA2 M3 | one M2 transmission, replay 2, 9 ms |
| Initial WPA2 M4 | TX status 0 |
| Initial protected data | ARP TX/RX PASS with CCMP RX |
| Controlled runner stop | PASS, source `host-station-epoch-cycle` |
| Connected owner return | PASS, halted RX queue empty |
| Second Association | status 0, AID 32, one response frame, 21 ms |
| Second peer programming | HE20, QoS, noise floor -93 dBm, metric 70 |
| Second WPA2 M3 | one M2 transmission, replay 4, 10 ms |
| Second WPA2 M4/key install | TX status 0, pairwise slot 4, group slot 1 |
| Second connected entry | PASS |

The image also passed the SRAM/PSRAM placement audit and autonomous source
graph audit. Host tests prove that a stop already ready at idle publishes
link-down and returns `Stopped`, while a stop raised during an active TX waits
for the normal completion path before returning ownership.

## Remaining qualification

- repeat several controlled cycles instead of parking in epoch two;
- route real beacon loss through scan/candidate selection and Authentication;
- qualify AP loss and recovery, retry/backoff exhaustion, and an injected
  TX/RX hardware failure;
- move cold scan into the reusable allocation-free STA service which now owns
  Open Authentication, initial Association/WPA2 and later reconnect attempts.

## Production lifecycle addendum

The reconnect cell was repeated after introducing the executor- and
chip-independent `StaLifecycleService`. The latest qualified runtime CRC32 was
`34c3a724`; image size remained exactly 1,129,104 bytes and both
placement/source-graph
audits passed. The UART trace proved that generation 0, attempt 1 began with
`refresh_candidate=0 phase=authentication`, completed Open Authentication in
168 ms, initial Association (status 0, AID 23) and WPA2 Message 3 (one Message
2 transmission, replay 2), then returned the exact connected owner. After the
explicit 100 ms disconnect backoff, generation 1,
attempt 1 began with `refresh_candidate=0 phase=reconnect`, completed
reassociation (AID 23), WPA2 (one Message 2 transmission, replay 4) and entry
into the second connected epoch.

An earlier image had stopped after initial WPA2 timed out waiting three seconds
for Message 1 because that returned retry owner was discarded. Initial
Association/WPA2 now runs inside `StaLifecycleService`, so this failure is
bounded and retryable without rebuilding hardware. The error path has host
ownership tests and target compilation, but still needs an injected board
failure to qualify the retry itself. Open Authentication now also runs inside
the lifecycle and preserves its complete owner on failure.

The initial cold scan runs under `StaCandidateScanService` and the production
`Esp32s31StaScanBackend`. Its HIL port carried the cold PAC token by value
across the complete channel order 6, 1--5, 7--13, returned that token after
candidate selection, and only then crossed `into_running`. The production
executor owns mandatory RX-stop cleanup after every bounded dwell and its host
tests also cover a drain failure followed by stop and stop-failure precedence.
`Esp32s31ScanRx` additionally retained the descriptor capability through
`Prepared -> Live -> Halted`, recycled completed prefixes without the former
full-ring mask, and transferred the exact halted ring into Authentication.
`RadioHilJoinRx::Initial` and both raw-address recovery loops were absent from
this image. All 13 board channel transactions completed without a scan-service
failure in 5,641 ms. This qualifies the shared transaction/RX owners and HIL
cold port, not production placement of cold PHY/TX, a running port or a rescan
after AP loss.
