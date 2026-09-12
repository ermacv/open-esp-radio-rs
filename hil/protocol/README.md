# HIL protocol v126

Host and firmware must both use version 126. Other versions are rejected
before interpreting their command and evidence layouts.

`phy_rx_hot_sram` identifies the paired placement experiment. It requires the
RX-delivery diagnostic image and moves the direct RX-gain transaction into
internal SRAM without changing its graph or ROM short-delay policy.

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

`BluetoothPeripheral::StartAdvertising` requests a bounded diagnostic
`ADV_IND` on channel 37 through production HCI. `Snapshot` returns boot-lifetime
advertising/peripheral RUN counts, retries and the first terminal reason.
These are software publication observations; a peer observation is required
to establish RF delivery. The start response includes the public address.
HCI rejection stages are Reset (0), address read (1), parameters (2), data (3)
and enable (4).

The Bluetooth image advertises `bluetooth_peripheral` for this interface.
DTM is rejected once the peripheral probe starts. Another advertising start is
accepted only after the previous connection has reported idle restoration;
that restart omits HCI Reset. The diagnostic detail includes the cumulative
closed-connection count and last disconnect reason (including `0x3e` for
failed establishment). Active LL Reset is not composed.
The host ends the probe with a
board reset; after 30 seconds the target reports lease expiry and resets the
whole board. A USB failure during the probe also resets the board. Neither
path reports logical HCI quiescence. The default connection timing policy
still lacks a local clock bound, so recurrence stops at that explicit limit.

Source types are authoritative. A frame is:

```text
00 00 | COBS(fixed-header | postcard(body) | CRC32C) | 00
```

The 34-byte little-endian header contains magic `ORHL`, framing version,
command/event kind, protocol version, boot ID, message sequence, session ID,
request ID and payload length. CRC covers header and body. A wrong kind or
version is rejected before postcard decoding. Host and target treat decode
errors, sequence gaps and bounded-queue loss as protocol failure.

The optional `async-io` feature exposes `write_frame` for an already encoded
frame. It writes all bytes and flushes before returning success. Callers must
bound the complete operation, including flush: accepting bytes alone does not
complete a USB transfer whose final packet is full.

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

UDP `ServiceReady` is published after the socket is bound. The host waits for
both this declaration and `NetworkReady` for the same interface before sending
an unmeasured `UdpProbe` challenge on the exact TX flow. Only its matching
nonce response confirms the reverse path; a successful host `send()` does not.
The target services probes while idle, before `Start`, using its bound TX
socket. Requests retry on a deadline within an absolute failure bound. Probe
responses never contribute to measured sequence counts, including late copies.
The host starts its socket collectors before `Start`. `SessionReady` confirms that the requested
workers and link preconditions are ready for the measured session. Readiness
has no fallback IP, settle delay or success-on-timeout path. USB serialization
completion and BlockAck readiness wake their waiters on state changes.
The host serial reactor wakes on descriptor readiness, queued commands or
shutdown. Protocol waiters also receive cancellation notifications; their
timeout is a failure bound rather than a periodic readiness poll.

`Finished` counts stack admissions, not on-air delivery. Host UDP collectors
use its TX count to complete as soon as every datagram arrives, or record a
delivery deadline and the remaining deficit. A missing terminal result and a
fully delivered stream are distinct outcomes. Each `*-reception.json` records
Linux socket drop deltas (`SO_MEMINFO`), including losses with no later packet.
A nonzero delta invalidates the measurement as `host-overflow`; unavailable
accounting on another platform is `null`, never an asserted zero.

The target retains the complete result before its first publication, including
fixed link and stack snapshots. `ReplayResult` changes only the response
envelope identity and sequence, not evidence or its digest. The host verifies
the replay before acknowledging removal of the retained result.

Evidence is typed. Every session includes transport, UART link health and CPU
stack watermarks. UDP adds the radio facts needed for qualification; the RX
diagnostic image also adds delivery-frontier evidence. Aggregate histograms and
TX timing have typed records; supplemental text diagnostics cannot establish
readiness or completion.

TX evidence covers a live interval, not a guessed queue drain. The radio executor
collects the two aggregate lifecycle snapshots between its polls. Publication
and prepared-standby owners still outstanding at each boundary are explicit.
For a failure-free interval, `publications + pending_start = block_ack_samples +
pending_end`; likewise, `standby_prepared + standby_pending_start =
standby_published + standby_cancelled + standby_pending_end`. Terminal hardware
failures remain failures, and a missing completion cannot be replaced by a
numeric tolerance. TCP and UDP derive text and typed aggregate evidence from
the same frozen snapshot.

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

Station `TxRadioEvidence.station_terminal` reports logical aggregate receipts
only after BlockAck retries and any detached ordinary retry have terminated.
`acknowledged + unacknowledged == mpdus` uses wrapping interval counters;
invalid normalized statuses are counted separately and invalidate evidence.
These counters exclude live and quarantined exchanges. A missing ACK does not
prove that the peer failed to receive an MPDU. They are neither UDP sequence
accounting nor a radio-drain barrier. The host retains them independently in
`station-tx-terminal.json`; aggregate publication/completion counters retain
their earlier per-publication meanings.

PHY child poll evidence distinguishes `pending`, `suspended_micros` and
`maximum_suspension_micros` from time spent inside polls. A suspension starts
when a child returns Pending and ends at its next poll entry. No wake timestamp
or hardware-readiness timestamp is implied. Successful timing requires one
Ready poll per completed child and disjoint poll/gap totals fitting inside
its enclosing operation. Cancellation leaves an incomplete operation; gaps
between independent invocations are excluded.

`PhyTimingEvidence.dcode_waits` separates direct runtime Dcode I2C waits,
nested RFPLL I2C waits and explicit RFPLL settling/lock delays. Each timing
counts completed waits with requested/elapsed totals and maximum per-wait
lateness. Each I2C group's `bus_busy` is a subset of `timing.count`; other waits
precede completion reads. PLL locked/unlocked counts reuse existing analog lock-status reads in Dcode,
without extra MMIO reads or a timestamp of physical lock acquisition.
Unsupported measurement, overflow, unmatched events and cancellation invalidate
complete timing. All three wait totals must fit together inside Dcode time.
They overlap the enclosing operation/poll/gap measurements and must not be
added to them. This evidence does not cover all PHY waits.

### TX calibration wait detail

`StationPhyTxWaits` precedes the correlated `StationPauseCompleted` when PHY
operation timings are available. Both events use the pause request ID and the
reliable event stream; no per-sample events are emitted. The separate detail
keeps the existing 480-byte body limit, including worst-case integer encoding.
The runner retrieves already received detail after completion and rejects
missing detail or sequential wait totals exceeding the TX operation interval.
An access-only pause has zero TX wait and SAR counts. With no timing observer,
neither detailed nor aggregate timing evidence is present.

PBus, search settling, tone arming, SAR triggering and root setup/cleanup retain
counts, requested/elapsed microseconds and maximum lateness. `sar_ready` and
`sar_not_ready` count existing status reads, not the time of physical readiness.
The driver timing-invalid flag covers unsupported, unmatched and overflowing
observations. Timer lateness includes dispatch, polling and resumption; it is
not pure executor latency. These intervals overlap operation/poll accounting
and must not be added to it. `station-pause.json` stores the correlated detail
under `tx_waits`; the raw stream remains in `protocol.jsonl`.

### Platform timer window

`overlapping_interrupts` explicitly accounts for IRQ entry overlapping alarm
programming before acknowledgement. These events have no attributable alarm
latency or deadline lateness; they remain part of IRQ/ack/dispatch counts.

`StationTimerObserved` includes due-deadline counts at registration and alarm
programming boundaries, plus IRQ acknowledgment timing. These refer to all users
of the shared platform timer, not exclusively PHY tasks. The event is separate from the PHY-specific wait detail. With
`driver-observation`, HIL opens one exclusive shared-timer observation window
around the station pause request and emits its report before the correlated
`StationPauseCompleted`. The runtime feature is absent from ordinary performance
images. The host retains the report under `timer` in `station-pause.json`,
requires its presence for a measured pause and validates alarm/IRQ/dispatch
count reconciliation. Duplicate, unrelated or late records cannot satisfy the
request. The existing frame-size bound applies to this event independently.

The [platform timer contract](../../crates/adapters/embassy/esp32s31/runtime/src/timer_observation/README.md)
defines timestamp boundaries, partial windows and observer overhead. These are
shared-timer measurements; they do not attribute every IRQ to PHY or measure
wake-to-PHY-poll latency. A control pause may contain legitimate timer work from
other tasks even when PHY wait counters are zero.

The `StationTrackingService` detail precedes its correlated `StationPauseCompleted`
event for the bounded automatic-service window. It counts completed observations
and operations, actual common/TX calibration commits, physical pause durations
and invalid/fault/suspension flags. The host requires repeated observations in
the specified window and rejects missing or unrelated detail. Standalone
operation requests distinguish temperature, power, I2C, common and TX diagnosis.

The `rfpll` station-pause operation requests one zero-threshold measured RFPLL
correction within the retained station epoch. Its normal pause result and PHY
timing frame are required; a resumed result without exactly one completed RFPLL
operation is insufficient. The automatic-service operation counters use the
order temperature, Wi-Fi power, analog I2C, common calibration, Wi-Fi TX and
RFPLL. A counter slot does not enable that operation in automatic policy.

`StationRfpllObserved` is a separate detail frame preceding the matching
`StationPauseCompleted`. The host requires matching boot, session and request
identities and rejects duplicate detail. RFPLL completion timing requires one
valid terminal detail; absence cannot stand for zero correction. The values
distinguish skipped evaluation (`correction = None`), completed zero correction,
and nonzero correction with memory publication. The `rfpll-check` operation uses
the retained sample and ordinary thermal threshold; `rfpll` explicitly uses zero
threshold. Neither command fabricates a temperature or certifies RF lock.

Both `rfpll` and `rfpll-observed` require a dated sample no older than
`STATION_RFPLL_SAMPLE_MAX_AGE_MICROS` after physical admission. Each scenario
first waits for a separate `temperature` pause completion and restoration;
the target command itself does not implicitly acquire a new sample. RFPLL detail
reports `sample_age_micros` from acquisition start at RFPLL entry. Missing or
over-age detail fails the scenario. Temperature and RFPLL frames retain their
own request correlations and operation timings.

`UdpRxStarted` reports the first 256 valid single-flow UDP datagrams consumed
inside a session. It is correlated by boot/session and is distinct from
`SessionReady`: readiness alone does not prove delivery.

Transport and per-flow evidence optionally retain `rx_maximum_silence_micros`
for a complete single-flow UDP receive window, including trailing silence.
Missing observation is `None`, never inferred as zero from average throughput.
Concurrent RX flow windows are not projected into one session continuity value.

`StationPhyRxGain` carries disjoint RX gain executor intervals before the same
request's pause completion. The host checks correlation, terminal counts and
containment in the parent interval; missing detail invalidates timed evidence.

RX gain detail carries disjoint DC, publication and control region timings even
without per-edge timing. Minimum-search timings are nested in the DC region;
they must not be added to region totals. Fine prepare/external/advance timings
remain optional and form a separate decomposition. Region time includes waits,
interrupts and observer overhead, and does not identify pure CPU time.

RX detail also samples the first and every sixteenth subsequent minimum call
and non-minimum DC executor step. Sampled minimum preparation, MMIO, settle,
readiness and advancement are disjoint inside minimum_sample; outer preparation,
execution and advancement are disjoint inside outer_sample. These counts/times
cover only the selected calls, not the whole calibration or a random sample.
Timing includes observer overhead and waits. Do not extrapolate sample totals as
measured whole-operation cost. All region totals remain complete.
