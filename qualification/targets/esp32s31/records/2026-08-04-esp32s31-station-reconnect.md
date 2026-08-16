# ESP32-S31 controlled STA epoch reconnect

Date: 2026-08-04  
Board: ESP32-S31 revision 0  
Scenario: `radio` / `open-radio-hil`  
Profile: `psram-code-psram-data`  
Latest qualified runtime CRC32: `3b6658eb`

Qualification ID: `HIL_ESP32S31_STA_RECONNECT_2026_08_04`

## Claim

Healthy connected epochs can be stopped repeatedly at a production runner
transaction boundary, fully torn down, scanned again and reconstructed on a
selected same-SSID candidate without another PAC singleton or static
allocation. The returned cooperative hardware, RX DMA frontier, network
stack, TX/A-MPDU resources, control mailbox, PMK, nonce and sequence owners
completed three consecutive fresh Open Authentication, Association, WPA2
four-way handshake and connected-runner transitions. The available candidate
happened to be the same peer in this cell.

This does not claim recovery from AP disappearance. The latest controlled run
performs a full running scan and feeds the selected candidate into fresh Open
Authentication before Association. It intentionally distinguishes
`ConnectedRunnerExit::Stopped` from beacon-loss `Disconnected`; the HIL adapter
names the former `CycleRequested`, and the outer service enters
`RunningScan` with `refresh_candidate=1`. The trigger is still a healthy
host-requested cycle rather than AP disappearance.

## Procedure

The image was built and flashed with:

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
```

Credentials remained host-owned and were provisioned over the framed UART
protocol. The lifecycle qualification was then run with:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

The host accepts the cell only when protocol v5 advertises station epoch
control and returns `StationEpochCompleted` with the same request ID. The
target constructs that acknowledgement only after the production runner and
teardown return ownership, a complete running scan returns its owners, fresh
Authentication/Association/WPA2 completes, and the next connected runner is
started. Terminal lifecycle/exhaustion reports still fail immediately;
retryable scan/join reports remain under lifecycle policy. Text markers are
diagnostics and no longer decide PASS because the UART text writer is
deliberately lossy. Evidence from an earlier request ID cannot satisfy a later
cycle. The local transcript is written to
`target/hil/esp32s31/qualification/station-reconnect/uart.log`.

## Observed evidence

| Transition | Result |
| --- | --- |
| Initial Open Authentication | PASS, 168 ms |
| Initial Association | status 0, AID 18, HE20/WMM, 21 ms |
| Initial WPA2 retry | Message 1 timeout, 100 ms bounded backoff, reassociation in 24 ms |
| Initial WPA2 M3 | attempt 2, one M2 transmission, replay 10, 9 ms |
| Initial WPA2 M4 | TX status 0 |
| Initial protected data | ARP TX/RX PASS with CCMP RX |
| Controlled runner stop | PASS, source `host-station-epoch-cycle` |
| Connected owner return | PASS, halted RX queue empty |
| Outer candidate refresh | generation 1, attempt 1, `refresh_candidate=1`, `RunningScan` |
| Running candidate scan | 13 channels, 13/13 Probe TX, target on channel 1 |
| Running owner return | halted RX/control TX returned, selected record transferred |
| Second Open Authentication | PASS on cooperative hardware, 52 ms |
| Second Association | status 0, AID 20, one response frame, 21 ms |
| Second peer programming | HE20, QoS, noise floor -89 dBm, metric 62 |
| Second WPA2 M3 | one M2 transmission, replay 12, 8 ms |
| Second WPA2 M4/key install | TX status 0, pairwise slot 4, group slot 1 |
| Second connected entry | PASS |

The image also passed the SRAM/PSRAM placement audit and autonomous source
graph audit. Host tests prove that a stop already ready at idle publishes
link-down and returns `Stopped`, while a stop raised during an active TX waits
for the normal completion path before returning ownership.

## Repeated-cycle evidence

The latest `3b6658eb` image completed one initial connection followed by three
host-requested lifecycle cycles in a single boot. Every cycle returned its own
correlated typed completion and reached a new connected task topology with
`network_started=0`, confirming that the persistent network stack was reused
rather than initialized again.

| Generation | Scan | Authentication | Association | WPA2 M3/M4 | Connected topology |
| --- | --- | --- | --- | --- | --- |
| 1 | 5,494 ms, 13/13 Probe TX | PASS | status 0, AID 24 | replay 4, M4 status 0 | PASS |
| 2 | 5,663 ms, 13/13 Probe TX | PASS | status 0, AID 21 | replay 6, M4 status 0 | PASS |
| 3 | 5,445 ms, 13/13 Probe TX | PASS | status 0, AID 28 | replay 8, M4 status 0 | PASS |

All three typed acknowledgements were complete. The captured text retained
only one of the three runner-stop markers while reporting 34 dropped text
records; all three scan-owner-return and connected-entry diagnostics happened
to survive. The typed PASS therefore remains independent of lossy text. No
station, task-stop or other `result=FAIL` marker and no panic occurred. The
UART transcript SHA-256 is
`7780d607cd2e3e31d38a1b7d61b1aa2083bececf8938f9d5cde78925569f57c8`.
This closes the repeated healthy-cycle qualification gap, but still does not
simulate loss of the AP.

The connected benchmark task now initializes its scenario-selected static
scratch exactly once and remains alive across epochs. This removes the former
third-cycle `StaticCell is already full` ceiling without allocating every
scenario's buffers. After each runner return, HIL requests both the benchmark
and RX-protocol tasks to stop and requires both acknowledgements under one
two-second deadline. A missed acknowledgement is reported as
`production-connected-task-stop`, declares `reset_required=1` and preserves
owners rather than pretending that the epoch can be safely reused. No such
timeout or panic occurred in this run.

The immediately preceding `57778894` image completed four cycles in one boot,
so removing the repeated `StaticCell` initialization is also qualified beyond
the former third-cycle failure point. Its transcript SHA-256 was
`8385d0f7f2bead8358cbfdc7ba3d059d5605654074c70d4c67a2f5b21d61b3f6`.
The latest image then moved the common stop/deadline rule into
`stop_esp32s31_connected_task_group`; HIL now implements only the concrete
benchmark/protocol signal adapter. Host tests prove both exact owner return
and the distinct reset-required deadline outcome.

This repetition also qualifies the extracted
`Esp32s31ScanPort`. PHY retune, cooperative register access, stopped RX
restart, polling Probe TX, one-millisecond Embassy dwell ticks, scan-table
observation and exact-SSID selection now live in the reusable integration
crate. `radio_hil.rs` supplies the returned owners and HIL evidence observer
but no longer implements the running `Esp32s31StaScanPort`. Removing its
synchronous per-channel UART diagnostics reduced the observed 13-channel scan
from roughly 5.8 seconds to 5.3--5.4 seconds without changing the 200-tick
dwell policy. The latest release image is 1,194,640 bytes and passed both
placement and autonomous-source-graph audits.

The same cell qualifies the extracted `Esp32s31RxFrontier` owner. HIL no
longer defines its own `Halted/Prepared/Live/Vacant` enum or its walker settle
transition. The production owner carried the exact RX frontier through fresh
Authentication, Association and WPA2 in every generation, including a valid
1,003 ms Message 3 wait in generation one, and returned to the identical
descriptor base before the next scan.

It also qualifies the production station epoch transition. Connected teardown
now returns `Esp32s31DisconnectedStaEpoch`; running scan can move only its
hardware and stopped RX while the persistent network, A-MPDU and control
owners remain sealed in a retention value. `prepare_reconnect` consumes the
restored epoch and returns `Esp32s31ReconnectedStaEpoch`. All three generations
crossed that exact transition and reused descriptor base `0x2f03ea50`.

The same image routes both management-frame and EAPOL descriptor walks through
`Esp32s31RxFrontier::service_completed`. HIL no longer forms unsafe DMA
buffer references or rearms descriptor halves in either backend. A terminal
frame still stops before recycle and transfers the observed live-ring frontier
to the next protocol phase. The release image remains 1,194,640 bytes.

## Remaining qualification

- route real beacon loss through scan/candidate selection and Authentication;
- qualify AP loss and recovery, retry/backoff exhaustion, and an injected
  TX/RX hardware failure;
- move both scan modes into the reusable allocation-free STA service which now
  owns Open Authentication, initial Association/WPA2 and reconnect attempts.

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
tests also cover a drain failure followed by stop, stop-failure precedence and
a fatal active-probe failure followed by stop. Passive fallback is now allowed
only when the control-TX error proves that the descriptor owner is quiescent;
reset-required or uncertain TX ownership returns to the lifecycle instead.
`Esp32s31ScanRx` additionally retained the descriptor capability through
`Prepared -> Live -> Halted`, recycled completed prefixes without the former
full-ring mask, and transferred the exact halted ring into Authentication.
`RadioHilJoinRx::Initial` and both raw-address recovery loops were absent from
this image. `Esp32s31ScanPhy` carried the persistent PHY state, platform,
delay and observer outside HIL. `Esp32s31ColdScanTx` carried the control
descriptor, TSF/interrupt preparation and passive-fallback decision; the old
HIL Probe Request helper and raw queue cleanup were absent. Runtime CRC32
`9770bd1d` passed placement and autonomous-source audits; all 13 board channel
transactions completed without a scan-service failure in 5,620 ms.

The first generation then supplied unplanned but valid failure-path evidence:
Association returned AID 24, the first WPA2 Message 1 wait expired after its
bounded 3,000 ms deadline, and the lifecycle retained the join owner across a
100 ms `AttemptFailed { stage: Security }` backoff. Attempt two reused that
owner, associated with AID 24 and completed M3/M4 at replay 6. The host-driven
epoch stop then completed same-peer reassociation (AID 24) and WPA2 at replay
8 before entering the second connected epoch. The overall
`station_reconnect=PASS` result therefore covers a real security retry as well
as the controlled reconnect. It does not replace deterministic injected-fault
qualification. This qualifies the shared transaction and cold PHY/RX/TX
owners, not a running scan port or a rescan after AP loss.

## Persistent PHY and running-RX ownership addendum

The cell was repeated with runtime CRC32 `912f48a3`; the image remained
1,129,104 bytes and passed both placement and autonomous-source-graph audits.
`PhyColdState` is now a field of the fixture returned by every connected
epoch, rather than a join-only reference dropped after Authentication. The
same owner is therefore available to retune a later candidate scan.

The disconnected RX restart was also changed to consume
`Esp32s31RunningScanRx`. That production owner separates the halted ring from
the connected epoch only while RX is prepared/live, retains the staging pool,
queue sender, reload delay and telemetry binding, applies the qualified 5 us
walker-enable settle edge, and returns the exact `Esp32s31StoppedRx` after
stop. The board emitted `production-rx-restart` with the same descriptor base
`0x2f03e910` and an empty retained queue.

The same finite epoch now also consumes `Esp32s31RunningScanTx` after the MAC
IRQ routes are disabled. It publishes through the ordinary descriptor returned
by connected teardown; it retains a borrow of the quiesced
`MacInterruptSetup` for its complete lifetime. It applies the same fail-closed
active/passive classifier as cold scan and returns that exact
`Esp32s31ControlTx` before Association.
The board emitted `production-running-probe channel=1 status=0`, reported one
completion and zero probe failures at `production-rx-restart`, then completed
Association/WPA2 and `production-reconnect-connected-enter`.

This addendum qualifies persistent PHY ownership and the running-scan RX/TX
sub-owners on hardware. It deliberately does not claim a channel scan after
disconnect: PHY retune and observation/candidate ownership still need to be
composed into an `Esp32s31StaScanPort` and routed through the outer lifecycle
before AP-loss rescan can be qualified.

## Multi-channel running-scan addendum

The latest cell used runtime CRC32 `1774ee7a`; the image grew from
1,129,104 to 1,203,712 bytes after both cold and running concrete scan/PHY
compositions became reachable. It remained within the image budget and passed
the SRAM/PSRAM placement and autonomous-source-graph audits. The approximately
73 KiB increase is a code-size deduplication target, not a RAM reservation or a
qualification failure.

The controlled disconnected epoch assembled `Esp32s31ScanPhy`,
`Esp32s31RunningScanRx`, `Esp32s31RunningScanTx` and the persistent scan table
into a concrete HIL `Esp32s31StaScanPort`. The unchanged production
`StaCandidateScanService` and `Esp32s31StaScanBackend` completed all 13 channel
transactions in 5,850 ms. Active Probe Requests completed 13/13 with no TX
failure. The target was observed on channel 1 at -27 dBm; the scan processed 11
raw management frames, retained one Probe Response and crossed 9 ring-recycle
epochs. The exact halted RX and control-TX owners were returned, the PHY was
retuned from the end of the scan plan to the selected candidate, and fresh
Open Authentication on `CooperativeTxHardware` completed in 52 ms. The
subsequent Association/WPA2 transaction reached
`production-reconnect-connected-enter`.

This qualifies a real multi-channel running-scan transaction and its ownership
round trip, including transfer of its result into a fresh Authentication
transaction. Generation 1 entered a distinct outer `RunningScan` owner with
`refresh_candidate=1`; recoverable failure exits return the disconnected
hardware frontier to lifecycle policy. It still does not qualify AP-loss
recovery: the fixture initiates the scan after a controlled healthy cycle and
the AP remains available. Actual AP disappearance/recovery and an injected
RX-stop failure remain separate cells.
