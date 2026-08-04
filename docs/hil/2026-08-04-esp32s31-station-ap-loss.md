# ESP32-S31 controlled AP-loss recovery

Date: 2026-08-04
Board: ESP32-S31 revision 0
Scenario: `radio` / `open-radio-hil`
Profile: `psram-code-psram-data`
Runtime CRC32: `565e366d`
Application image: 1,194,640 bytes

Qualification ID: `HIL_ESP32S31_STA_AP_LOSS_2026_08_04`

## Claim

A real disappearance of the associated AP reaches the production beacon-loss
deadline, returns the connected runner and every executor/DMA/key owner, enters
the outer station lifecycle's fresh-candidate path, and reconnects after the
same AP becomes available again. This is not a host-requested healthy cycle.

Protocol v6 emits reliable unsolicited lifecycle edges. The host accepted this
cell only after observing this exact sequence:

```text
Connected { generation: 0 }
Disconnected { generation: 0, reason: BeaconLoss }
Connected { generation: 1 }
```

`LinkPolicy` and `ReconnectRequested` cannot satisfy the middle edge. Text
diagnostics are not qualification input.

## Procedure

The repository-controlled Linux HE20 AP was started and stopped through the
narrow `open-radio-net` sudo helper. It used channel 11, WPA2-PSK/CCMP and the
host-owned credentials already supplied to the HIL environment. The runner
restored `wlan0` to managed mode on both success and error paths.

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
cargo hil station ap-loss --serial /dev/ttyACM0 --timeout-seconds 120
```

## Evidence

| Boundary | Result |
| --- | --- |
| Initial connected generation | PASS after 9,563 ms |
| Initial Authentication | attempt 1, one response |
| Initial Association | status 0, AID 1 |
| Initial WPA2 | replay 2, Message 4 status 0 |
| AP removal to beacon loss | PASS after 1,152 ms; `beacon_lost=1` |
| Connected task group stop | PASS; benchmark and protocol acknowledged |
| RX/control return | empty queues/reorder state; RX DMA stopped |
| Key teardown | pairwise slot 4 and group slot 1 cleared, bitmap zero |
| TX return | PASS |
| Running scan | 5,605 ms, 13/13 Probe TX, candidate on channel 11 |
| Scan owner return | descriptor base `0x2f03ea50`, empty queue |
| Fresh Authentication | attempt 1, one response |
| Fresh Association | status 0, AID 1 |
| Fresh WPA2 | replay 2, Message 4 status 0 |
| Recovered connected generation | PASS after AP restart in 5,857 ms |
| Persistent network reuse | PASS, `network_started=0` |

The encoded image passed the SRAM/PSRAM placement audit and autonomous source
graph audit. The UART transcript contains no `result=FAIL`, connected-task
timeout or panic. Its SHA-256 is
`535f775caa5da083bed1f82bb7babcf96fbeafdeeee58ab41215bfb09fe129ba`.

After qualification, the helper restored `wlan0` to managed mode and the host
reassociated with its external network.

## Remaining boundary

This cell proves recovery when the AP returns before the first complete
running scan ends. It does not yet qualify prolonged AP absence, multiple
`NoCandidate` retries, retry exhaustion, or injected TX/RX hardware failure.
