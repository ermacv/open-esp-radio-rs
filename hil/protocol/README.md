# HIL protocol v80

Host and firmware must both use version 80. Other versions are rejected
before interpreting their command and evidence layouts.

`ProbeMemoryBenchmark` runs one pre-initialization CPU, blocking GDMA or async
GDMA copy from SRAM/PSRAM into SRAM. A request specifies 1..=4096 payload bytes
per frame, 1..=32 frames and 1..=64 measured iterations. Each iteration copies
at most 49,152 payload bytes, excluding storage padding and guards. The CPU
copies frames in a loop; GDMA uses one scatter-gather chain per iteration.
`MemoryBenchmarkCompleted` echoes the request and retains completed iterations,
terminal correctness status and separate elapsed/foreground counter scopes.
Completed iterations account for entire batches whose payloads and guards
passed verification; a partially completed batch does not add an iteration.
Foreground means the whole CPU/blocking operation, or async prepare/start,
poll and cleanup windows. IRQs inside those windows remain included. These
values do not measure CPU utilization. The host imposes a per-case response
deadline; a target stalled inside synchronous hardware preparation may need
reset. Feature discovery identifies images implementing this diagnostic.

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
      <- StartStation(credentials) | StartAccessPoint(credentials, HT channel, static IPv4)
       | ScanWifi | StartMonitor | CaptureMonitor
```

Credentials exist only in role commands, are bounded/redacted/zeroized and
never enter scenario files or logs. AP IPv4 configuration is applied by the
HIL application to its persistent network stack, not by the radio driver.
Calibration bytes are opaque, chunked and CRC-protected; the host persists
them, never target NVS/flash.

RX admission follows the compiled sink's ownership contract. A receiver with
a separate output pool waits for its queue and buffer credits while retaining
the original staging owner. Initialization cannot override that requirement
or select direct dispatch for a sink which lacks immediate publication credit.

Traffic uses one state machine for UDP/TCP and RX/TX/bidirectional:

```text
Configure -> Arm -> Start -> SessionReady -> Evidence -> Finished(CRC)
          -> ReplayResult -> Evidence -> Finished(same CRC)
          -> AcknowledgeResult -> Idle
```

The target retains the complete result before its first publication, including
fixed link and stack snapshots. `ReplayResult` changes only the response
envelope identity and sequence, not evidence or its digest. The host verifies
the replay before acknowledging removal of the retained result.

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
of the result contract. The host stores exact received bytes in `uart.bin`, a
lossy text view in `uart.log`, and decoded events plus link/finalization health
in `protocol.jsonl`. Commands are omitted because they can carry secrets.
A capture owns one boot; transport loss or an unexpected reboot invalidates
outstanding operations and wakes their waiters. Optional waits return no event
only while the link remains healthy.

Wi-Fi commands admit only operations valid for the current `WifiIdle`,
`WifiStation`, `WifiAccessPoint` or `WifiMonitor` owner. Admission, successful
completion and terminal role failure are distinct request-correlated events.

Read-only attachment discovers a running runtime with `GetCapabilities` in an
envelope whose boot ID and session ID are zero. The reply is a correlated
`Hello` carrying the current nonzero boot ID; all subsequent queries bind to
that boot. This exception admits no other command, and protocol-version checks
still apply. Older firmware that rejects discovery must be explicitly updated;
the host never resets it to obtain status. An attachment begins at the current
event sequence; a reset-driven qualification capture still requires the boot
`Hello` at sequence zero. Both modes reject later sequence gaps and reboot.

`GetStatus`, `QueryStackUsage` and `QueryLinkHealth` do not initialize the runtime
or consume retained results. Stack queries can return `InvalidState` while
initialization is pending or session ownership prevents a safe snapshot. A
status observation preserves this unavailability and cumulative link counters
instead of applying a new workload's acceptance criteria to previous activity.
