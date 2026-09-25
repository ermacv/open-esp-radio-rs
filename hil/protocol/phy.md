# PHY HIL evidence

## Fault injection

`phy_fault_injection` advertises destructive checkpoints in real PHY maintenance.
`PhyFault(Arm(mode))` is one-shot per boot. In Bluetooth it requests idle forced
calibration after a peripheral cycle; Wi-Fi uses the normal connected
`PauseStation` calibration command after arming. `Status` reports `Reached` only
after the selected physical frontier. `Release` is accepted only then, and its
acknowledgement is serialized before injecting the fault. There is no disarm or
deadline renewal command. `Cancelled` means the actual child future was dropped,
not merely that the Host stopped waiting. A fresh boot must report `Idle` and
MWDT1 reset; RF-off timing is not represented by that observation.

## Placement diagnostic

`phy_rx_hot_sram` identifies the paired placement experiment. It requires the
RX-delivery diagnostic image and moves the direct RX-gain transaction into
internal SRAM without changing its graph or ROM short-delay policy.

## Timing scopes

`StationPauseEvidence.timeline` partitions one connected Wi-Fi maintenance
transaction into adjacent software ownership intervals, including TX drain
and return of the restored worker. `exclusive_intervals()` rejects reversed
timestamps. Compact images and aggregate service reports omit this field;
diagnostic physical transactions require it. See the
[station timing boundaries](../targets/esp32s31/phy.md#same-connection-pause).
The inclusive `elapsed_micros` and nested PHY timings are not added to that
partition. Worker release is a scheduling handoff, not an RF or first-poll edge.

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

The [platform timer contract](../../crates/adapters/embassy/esp32s31/executor/src/timer_observation/README.md)
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
