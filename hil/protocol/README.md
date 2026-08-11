# HIL protocol v25

Source types are authoritative. A frame is:

```text
00 00 | COBS(fixed-header | postcard(body) | CRC32C) | 00
```

The 34-byte little-endian header contains magic `ORHL`, framing version,
command/event kind, protocol version, boot ID, message sequence, session ID,
request ID and payload length. CRC covers header and body. A wrong kind or
version is rejected before postcard decoding. Host and target treat decode
errors, sequence gaps and bounded-queue loss as protocol failure.

Boot is role-neutral:

```text
Hello -> WaitingForInitialization
      <- optional calibration chunks
      <- Initialize(IPv4 policy)
      -> Initialized + WifiIdle
      <- StartStation(credentials) | ScanWifi | StartMonitor | CaptureMonitor
```

Credentials exist only in `StartStation`, are bounded/redacted/zeroized and
never enter scenario files or logs. Calibration bytes are opaque, chunked and
CRC-protected; the host persists them, never target NVS/flash.

Traffic uses one state machine for UDP/TCP and RX/TX/bidirectional:

```text
Configure -> Arm -> Start -> SessionReady -> Evidence -> Finished(CRC)
          -> AcknowledgeResult -> Idle
```

Evidence is typed. Every session includes transport, UART link health and CPU
stack watermarks. UDP adds the radio facts needed for qualification; the RX
diagnostic image also adds delivery-frontier evidence. Detailed histograms and
timings remain text diagnostics and cannot establish readiness or completion.

An uncertain host response is resolved without guessing:

```text
GetStatus -> OperationStatus
ReplayResult | Cancel (before Start) | Recover (terminal state)
```

Target event sequence continuity and both endpoint decoder counters are part
of the result contract. `protocol.jsonl` contains decoded target events plus a
final link-health record; commands are omitted because they can carry secrets.

Wi-Fi commands admit only operations valid for the current `WifiIdle`,
`WifiStation` or `WifiMonitor` owner. Admission and completion are separate,
request-correlated events. AP wire types remain unavailable until the target
advertises a complete AP implementation.
