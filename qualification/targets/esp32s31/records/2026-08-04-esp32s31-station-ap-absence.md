# ESP32-S31 prolonged AP absence and retry exhaustion

Date: 2026-08-04
Board: ESP32-S31 revision 0
Scenario: `radio` / `open-radio-hil`
Profile: `psram-code-psram-data`
Runtime CRC32: `343d7698`
Application image: 1,194,640 bytes

Qualification ID: `HIL_ESP32S31_STA_AP_ABSENCE_2026_08_04`

## Claim

After a real disappearance of the associated AP, the production station
lifecycle returns the complete connected epoch, performs three bounded fresh
13-channel scans, classifies each empty result as `NoCandidate`, and returns
all scan owners before reporting retry exhaustion. No host-requested healthy
cycle or synthetic disconnect can satisfy this cell.

Protocol v7 is the qualification input. The host accepted only this exact
unsolicited sequence:

```text
Connected { generation: 0 }
Disconnected { generation: 0, reason: BeaconLoss }
AttemptFailed { generation: 1, attempt: 1,
                stage: CandidateSelection, reason: NoCandidate }
AttemptFailed { generation: 1, attempt: 2,
                stage: CandidateSelection, reason: NoCandidate }
AttemptFailed { generation: 1, attempt: 3,
                stage: CandidateSelection, reason: NoCandidate }
RetryExhausted { generation: 1, attempts: 3,
                 stage: CandidateSelection, reason: NoCandidate }
```

Lifecycle publication waits for the exact event to be serialized by the USB
protocol task. Queue admission alone is not accepted at the terminal edge.
Text diagnostics remain non-authoritative.

## Procedure

The repository-controlled Linux HE20 WPA2-PSK/CCMP AP was available for the
initial connection and then kept down. Credentials remained host-owned and
were provisioned through the framed UART protocol.

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
cargo hil station ap-absence --serial /dev/ttyACM0 --timeout-seconds 120
```

The AP guard restored `wlan0` to managed mode on exit.

## Evidence

| Boundary | Result |
| --- | --- |
| Cold active scan | 5,816 ms; 13/13 Probe TX, zero Probe TX failures |
| Initial link | Open Authentication, Association status 0/AID 1, WPA2 Message 4 status 0 |
| Peer disappearance | typed generation-zero `BeaconLoss` |
| Connected task group stop | interrupt, RX protocol, benchmark and control owners returned |
| RX DMA/key/TX teardown | queue empty; descriptor base `0x2f03ea50`; both CCMP keys cleared; TX returned |
| Retry scan 1 | 13/13 Probe TX, zero failures, no candidate, empty returned queue |
| Retry scan 2 | 13/13 Probe TX, zero failures, no candidate, empty returned queue |
| Retry scan 3 | 13/13 Probe TX, zero failures, no candidate, empty returned queue |
| Retry policy | backoff 100 ms then 200 ms; typed exhaustion after three generation-one attempts |
| AP removal to exhaustion | 17,658 ms |
| Image audits | placement PASS; autonomous source graph PASS |

The diagnostic `attempts=4` at the final lifecycle exit includes the initial
successful generation-zero attempt. The protocol's `attempts=3` is the
generation-one retry count and is the value asserted by the host.

The final UART transcript contains no `result=FAIL`, panic or abort. Its
SHA-256 is
`eb0f26264f0c23a905ffefcdfa6c22c1394a430e34e16f35fc0d1571fc9e9990`.

## Remaining boundary

This cell proves bounded recovery policy and owner return for an unavailable
peer. It does not inject a MAC TX timeout, RX DMA failure, scan stop failure or
the platform reset action required by a terminal owner frontier.
