# HIL protocol

The `bluetooth_gatt` capability identifies the separate plaintext Trouble
application image. `QueryBluetoothGatt` returns `BluetoothGattEvidence` with
the Controller address, application connection/read/write observations and
the CPU0 stack measurement. All observations belong to the envelope's boot;
the query neither resets the Controller nor submits ATT/HCI work. Counters are
not independent proof of ATT delivery. The Linux peer validates actual replies
and reconnection. This capability does not imply pairing or secure GATT.

The separate `bluetooth_secure_gatt` capability identifies the actual secure
application with one caller-owned RAM bond slot. `QueryBluetoothSecureGatt`
reports traffic counters, the current unanswered Numeric Comparison challenge,
independent confirmation/bond/reconnect/notification counters and application
failure. It does not export keys. `ConfirmBluetoothGatt` requires the exact boot,
session zero, challenge ID and six-digit number. Stale/duplicate replies are
rejected. `BluetoothGattDecisionRecorded` acknowledges a queued UI decision,
not successful pairing or protected access. Cancellation of the application
prompt revokes even a queued answer. These commands cannot submit HCI or ATT
traffic, clear bonds or force a security transition.

`RestartBluetoothGatt { epoch }` accepts only the currently running secure
application epoch, on session zero and the discovered boot. Duplicate, stale
and stopped-application requests are rejected. Its initial snapshot acknowledges
the request, not completed retirement. Completion requires a new `epoch`,
`restarting = false`, a physical `cold_releases` increment and `old_hci_closed`:
both command and read operations on the retired handle must reject access.
The target rebuilds Host/Controller without resetting the SoC and retains only
the caller's RAM bond store and comparison history, not connections or ATT state.
Independent encrypted peer traffic without another pairing establishes key reuse;
these lifecycle counters alone do not.

`BluetoothGattResetReadGate { epoch, release }` controls a one-shot secure HIL
reader gate on the discovered boot and current epoch. Arming (`release = false`)
requires an advertising, disconnected application and does not send Reset.
An explicit restart drops the old producers and submits its real final Reset;
the wrapper suspends the sole new reader before it consumes a response.
`reset_read_gate = ReaderHeld` is the reached checkpoint, not merely an arm ACK.
Release is accepted only at that checkpoint during the same restart. It wakes
the same reader; no response is fabricated, discarded or matched to a new Reset.
The hardware runner remains polled. Cancellation does not open the gate.
The gate does not report RF cessation or model a hung silicon operation. While
held, the unchanged epoch and cold-release count must be observed; release must
be followed by actual retirement/restart and independent encrypted peer traffic.

`FailBluetoothGattResetRead { epoch }` selects failure instead of release at the
same reached checkpoint. Early, stale or duplicate requests are rejected.
`FailureRequested` acknowledges only the command; `ReadFailed` means the wrapper
actually returned its distinct injected error without reading or discarding the
real response. The epoch runner must report `InjectedReceiveFailure`, preserve
its original stop cause and retain physical execution. Release/restart are then
rejected. This tests a Host-facing I/O error, not a silicon or PHY failure; no
RF-stop, physical-close or autonomous-recovery claim follows from retention.

`FailNextBluetoothGattBondLoad { epoch }` is a one-shot secure HIL diagnostic.
It requires a connected, bonded application on the current boot/epoch, rejects
duplicates and excludes a concurrent restart. Arming it does not stop the Host.
The next actual application store load returns a backend failure without
changing the RAM record. The scenario causes that load by disconnecting its
peer. `bond_load_failures` counts consumed injections; `shutdown` reports the
redacted cause and Reset outcome from the completed Host epoch. Neither proves
physical release: `cold_releases` and `old_hci_closed` remain separate checks.
An unresolved Reset may retain physical execution. The diagnostic does not
inject silicon failure, measure RF cessation or introduce a shutdown timeout.

Secure and plaintext capabilities are mutually exclusive. Both snapshots carry
common traffic observations, which alone never establish security. Notification
counters mean Host queue acceptance; the independent peer must receive the value.

Host and firmware must use the same wire version, defined by
[`PROTOCOL_VERSION`](src/message.rs). Other versions are rejected before
interpreting their command and evidence layouts. Capabilities describe which
operations the selected image implements; they do not replace wire-version
validation.

`phy_fault_injection` advertises destructive checkpoints in real PHY maintenance.
`PhyFault(Arm(mode))` is one-shot per boot. In Bluetooth it requests idle forced
calibration after a peripheral cycle; Wi-Fi uses the normal connected
`PauseStation` calibration command after arming. `Status` reports `Reached` only
after the selected physical frontier. `Release` is accepted only then, and its
acknowledgement is serialized before injecting the fault. There is no disarm or
deadline renewal command. `Cancelled` means the actual child future was dropped,
not merely that the Host stopped waiting. A fresh boot must report `Idle` and
MWDT1 reset; RF-off timing is not represented by that observation.

`BluetoothDtmEvidence::reset_reason` classifies the current platform boot as
software reset, MWDT1 reset or another cause. HCI Reset does not change this
value. The `bluetooth_watchdog_reset` capability identifies the dedicated
DTM fault-injection image; it is mutually exclusive with automatic
`bluetooth_phy_maintenance`. Each reset scenario requires its exact mechanism.
Archived captures retain their original wire schema and evidence layout.

`GetBootStatus` returns `BootEvidence` for the boot identified by the envelope.
`ResetReason` is a platform value shared by all images; querying it does not
issue HCI Reset or change radio state.

`SystemWatchdogTest` requires the independent `system_watchdog` capability.
The dedicated radio-free image's one-second engineering budget exercises completion, a synchronous
poll that never returns, cancellation, missing completion and late restoration
of the SoC deadline service. The correlated event acknowledges the selected
mode; only `Complete` acknowledges disarming. Other modes must produce a new
boot and MWDT1 reset reason. This does not inject a PHY failure or measure RF
cessation. The separate DTM reset scenario retains its independent peer gates.

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
`ADV_IND` on channel 37 through production HCI and selects peer Reset, verified
peer rfkill, target Host Disconnect or target HCI Reset as the cycle
termination. Its hold is
bounded to 0..5000 ms. `Snapshot` returns boot-lifetime
advertising/peripheral RUN and disconnection counts, retries, the first terminal
reason, and separately decoded Host-side Connection Complete, Connection
Update Complete and Disconnection Complete counts. A separate counter records
Channel Map Update instants applied by the Link Layer. The connection-update counter accepts
only the recovery profile's 120-ms interval, zero latency and 2-second
supervision timeout while the sole connection is live. Successful target
Disconnect Command Status and Reset Command Complete responses have separate
counters. It also reports each
nonempty Controller-to-Host ACL fragment, each
credit returned by the target Host, each deliberately held first-fragment
credit, each fully reassembled echo accepted back from the target Host, and ACL
profile or queue faults. A distinct Host-to-Controller completion count proves
that each queued echo reached Link Layer acknowledgement. The peripheral workload declares one 27-byte Host ACL
buffer, enables Controller-to-Host flow control and uses two distinct,
sequenced deterministic 251-byte packets around a live Channel Map Update. Each packet requires
ten legacy fragments. Invalid,
out-of-order or profile-mismatched lifecycle events increment
`host_event_faults`; the last decoded disconnection reason is retained. These
are software publication observations; a correlated peer observation is
required to establish RF delivery. The start response includes the public address.
HCI rejection stages are Reset (0), base event mask (1), LE event mask (2),
Host Buffer Size (3), Controller-to-Host flow control (4), address read (5),
parameters (6), data (7) and enable (8).

`Retire` ends an admitted peripheral probe: the target completes HCI Reset with
its Host event pump running, waits for live runner handoff, then retires timer,
IRQ and HCI ownership. It extracts the shared primary/NRT register owner,
checks scheduler inactivity, empty hardware heads and absence of primary faults,
and releases Controller output before joining the exact platform reservation.
The last PHY client then closes RF, powers down temperature, resets Bluetooth
and restores retained clocks and the shared power baseline into a cold owner.
`Retired` requires `radio_cold` and separate closed-channel probes for Host
commands, Controller events and Host ACL credits. Every field must be true,
with no terminal fault, saturation or Host event/ACL fault. An incomplete
transition produces no successful retirement response. Old HCI and static ISR
storage remain closed/reserved in this terminal mode. The HIL console stays available for capability and link-health
queries; further radio operations are rejected.

`Restart` uses the same physical shutdown sequence, then reinitializes the
actual returned radio and original storage without resetting the board.
`Restarted` requires a positive cycle count, Reset through the new Host and
closed-command/event/ACL-credit probes through the old Host.

`Maintain` instead joins the idle task and retired timer with the unrouted IRQ
bank for shared-PHY tracking. HCI and the powered epoch remain intact.
`Maintained` requires a positive cycle count, a due completed tracking request,
no tracking inhibition and Reset through the same Host after resumption.
Calibration flags report the actual common/Bluetooth work selected by tracking;
they are not forced true. A not-due window cannot satisfy this diagnostic.
Both lifecycle operations use the same zero-fault gates as `Retired`; another
advertising start reinitializes the bounded Host settings after Reset.

The Bluetooth image advertises `bluetooth_peripheral` for this interface.
DTM is rejected once the peripheral probe starts. Another advertising start is
accepted only after the previous connection has reported idle restoration;
that restart omits HCI Reset. The diagnostic detail includes the cumulative
closed-connection count and last disconnect reason (including `0x3e` for
failed establishment). The host ends the probe with a
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

The cold-restart and retained-cycle reports carry the radio actor's PHY
registration generation immediately before and after the operation. Cold
restart requires wrapping increment by one and `RestoredCache`; retained wake
requires exact equality, including its first cycle. Both also require the next
radio-role generation after station stop. These logical generations do not by
themselves prove physical RF restoration or quiescence. Station lifecycle
generation is a separate link-epoch counter: a stopped connected STA must
publish a fresh `LinkPolicy` disconnect for the previous link epoch, then a
new `Connected` edge in the next link epoch. A connected edge includes the
actually negotiated association width and security from the production station
status snapshot. Missing metadata cannot establish HT40/WPA2-Personal.

The lifecycle scenarios require fresh station `NetworkReady` after each
reconnection, then a bounded bidirectional UDP application session before and
after each radio cycle. Its optional `SessionFlowConfig.payload_identity`
binds the payload to the current boot and session ID. The target counts only
consumed RX datagrams with matching identity and fill; the Host checks each
received target TX datagram's source, length, identity, fill and sequence, and
reconciles both directions with retained session evidence. `ServiceReady`,
`SessionReady`, a probe response or a previous network address cannot replace
that payload proof. Other sessions leave `payload_identity` unset and retain
their existing traffic format. Hardware execution and dated link evidence are
still required for radio-cycle qualification.

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

`BluetoothPhyMaintenanceEvidence` carries ordered admission, IRQ/register/timer
retirement, PHY entry/exit, physical return and guarded RUN timestamps. Its
`exclusive_intervals()` partitions the measured admission-to-RUN span without
counting nested operations twice. Missing/reordered edges, failed observations,
or equality with either deadline invalidate that partition. IRQ retirement is
not an independent RF-off measurement; admission starts after protocol-level
window selection, so the span is not end-to-end request latency. Boot aggregates
retain failures even when a later idle transaction becomes the latest snapshot.

`PhyTimingEvidence.tracking` measures the common PHY executor as an inclusive
parent. Its children must not be added to it. These observations include
preemption and instrumentation overhead; they do not establish WCET, thermal
coverage or RF quality.

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
keeps each body within `MAX_POSTCARD_BYTES`, including worst-case integer encoding.
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


### Encrypted ACL diagnostic

`BluetoothPeripheral::EncryptedAcl` selects the fixed-key diagnostic Host before
advertising in a fresh boot. It is exclusive with the calibration-traffic and
ACL-backpressure Hosts. `BluetoothEncryptionEvidence` reports key requests,
successful HCI key replies (including negative and deliberately wrong-key replies),
Encryption Change, Key Refresh Complete events and faults. It never carries
key bytes. The public `BLUETOOTH_TEST_*` and `BLUETOOTH_REFRESH_*` constants identify fixture material,
not a pairing or bond-storage policy. Runner and firmware must match
[`PROTOCOL_VERSION`](src/message.rs). The body bound is defined by
[`MAX_POSTCARD_BYTES`](src/framing.rs), including the largest combined
peripheral evidence record and worst-case integer encoding.

The diagnostic Host accepts the initial identity once per connection, then the
distinct refresh identity only after initial Encryption Change. During the
pending refresh it rejects application data and requires the refresh event,
not another initial Encryption Change. Disconnect clears this phase before
reconnection; counters remain cumulative for the current boot.

The optional `failure` field selects `missing-key` or `wrong-key` for the first
initial LTK request, or `missing-refresh-key` for the first replacement LTK
request after successful initial encryption. Missing keys use the standard HCI
negative reply; `wrong-key` supplies a fixed public key differing by one bit.
The refresh injection survives the initial valid reply and requires termination
with PIN or Key Missing, without successful Key Refresh Complete. After disconnect,
subsequent connections receive the correct key. The Controller and its RF/CCM
path are unchanged. Expected injections have separate counters; malformed
requests, failed HCI replies, unexpected encryption success and application
data before encryption still invalidate the scenario.

`active-data-mic` instead arms the diagnostic `rx-fault-injection` feature.
The initial key and encryption handshake remain valid. Exactly one active
encrypted data PDU has the final MIC bit flipped in its copied RX input before
the production authenticator; control, plaintext and empty packets do not
consume the request. `mic_injections` records actual corruption and
`mic_injection_armed` identifies a pending request. Disconnect disarms it;
counters remain cumulative so recovery must prove that no further injection
occurred. The stimulus is after RF reception, not a malformed over-air packet.

## Stack measurements

`StackUsage` carries task watermarks and optional dedicated IRQ watermarks for
both running Wi-Fi harts. CPU1 samples its own stacks through a bounded
request/response task; CPU0 never scans its live stack storage. A missing
response fails rather than publishing a cached measurement. Images with shared
task/IRQ stacks report `None` for both dedicated IRQ fields.

`QueryInterruptStackUsage` provides a separate short response without enlarging
Bluetooth peripheral snapshots or the wire-frame limit. Bluetooth reports its
CPU0 dedicated IRQ watermark and `None` for its inactive CPU1. The runner checks
this response after peripheral commands, including physical retirement. Wi-Fi
accepts this query under the same idle-session conditions as `QueryStackUsage`.

IRQ sampling runs in thread mode with local interrupts masked during the SRAM
scan. It therefore adds a short interruption to RF servicing; these diagnostic
runs are not a timing baseline with instrumentation removed. The image's stack
policy supplies the nonzero required reserve. These are observed boot-lifetime
watermarks, not worst-case bounds or evidence of every possible nested IRQ.
