# HIL PHY workloads

## Functional maintenance continuity

`station-phy-maintenance-continuity` uses the correctness image and production
station/PHY owners with a managed HT20 peer. Each of three repetitions receives
UDP at a 2 Mbit/s offered load, waits for the target's 256-datagram progress
marker, then requests combined calibration after one second of preconditioning.
The complete 12-second UDP session must meet a 100 kbit/s liveness floor; loss is
allowed. After successful maintenance and completion of that session, a fresh
ICMP socket exchanges three requests/replies with the target (two-second failure
detection timeout each). The station lifecycle cursor covers the original
session and this new exchange, so reconnect, restart or beacon loss fails it.

The runner records `wifi.maintenance.transaction-valid`,
`wifi.maintenance.same-link` and `wifi.maintenance.ip-exchange-resumed` separately.
`post-maintenance-echo.json` retains the final exchange's outcome. A PASS requires
the entire repetition, not a successful early check. This establishes retained
connection and later IP reachability; it does not establish RF quality, a
calibration-time bound or resumed UDP throughput. Existing high-load and RFPLL
scenarios retain their independent acceptance criteria.

```console
cargo hil doctor station-phy-maintenance-continuity
cargo hil plan station-phy-maintenance-continuity --out target/hil/maintenance-continuity-plan.json
cargo hil run-plan target/hil/maintenance-continuity-plan.json --check
cargo hil run-plan target/hil/maintenance-continuity-plan.json
```

A fresh execution captures the source snapshot before building. Supply explicit
`--source-include <file>` arguments for nonignored untracked inputs as described
in the [host guide](../../host/README.md). This scenario needs only the DUT and
managed station AP; it does not require monitor interfaces or a Bluetooth peer.

## Same-connection pause

`cargo hil run diagnostic-station-pause --network patched-xarxa` requests one
explicit MAC/RX/IRQ pause during a 12-second station UDP TX window. The host
waits for at least 256 measured datagrams before sending `PauseStation`; it does
not use a post-start sleep. The target returns `StationPauseCompleted` with the
same request ID and an explicit success or failure stage. `RxBusy` is not a
successful pause. The host retains `station-pause.json`, protocol records and
ordinary delivery/link evidence. Reboot, disconnect or reassociation during
the workload fails the test even if delivery recovers. This tests preservation
of the connected runner and radio owners, including register withdrawal from
the arena, checked PHY-access admission/release and publication back into the
same arena. Reclaim, admission, release and republication have distinct failure
results. It performs no calibration or shared-RF arbitration.

With a managed OpenWrt fixture, these pause diagnostics automatically start
wireless-ingress and host-facing-egress captures before the session Start.
They stop after host delivery collection and retain both PCAPs and
`openwrt-tx-delivery.json`. Existing AP interfaces are borrowed, not replaced
by monitor interfaces. Capture uses immediate delivery, and tool readiness,
process failure and kernel drops are checked. The host invokes `tshark` itself.
Linux GRO can combine several UDP datagrams into one captured packet. The
report therefore distinguishes packet counts from UDP payload units computed
using the configured datagram size. Fragmented, undecodable or non-integral
payloads are rejected; capture payload units below measured host delivery are
inconsistent evidence. Equal totals do not establish packet identity or RF
ACK status. The 128-byte snapshots retain headers and the first payload bytes,
not complete contents of coalesced datagrams.

`cargo hil run diagnostic-station-phy-tracking --network patched-xarxa` uses
the same load and pause boundary with explicit due PHY tracking. The host
requires an uninhibited executed tracking outcome and continued delivery in
the same connection. `station-pause.json` distinguishes committed common and
Wi-Fi calibration branches; a false flag means that branch was not performed,
even if the tracking wrapper completed. No temperature threshold or scheduler
timestamp is fabricated to force a branch. MAC stop is reestablished and the
station receive-policy snapshot checked before the retained RX/IRQ resumes.
This single request does not enable automatic periodic tracking or qualify
temperature-triggered branches that were not selected during the run.

Diagnostic pause results also carry named PHY timings: selected attempts,
accepted completions, failures, total duration and maximum duration for outer
tracking and calibration children. Durations include waits and nested work;
they are not CPU load and parent/child totals must not be added. The host rejects
invalid or incomplete timing in a resumed result. The wire bound is unchanged.
DCODE, RX gain and TX DC/PWDET additionally report poll count, time inside polls
and maximum poll duration. The difference from operation duration includes
suspension and scheduling; poll intervals include interrupts and observer cost.
These measurements do not separate hardware delay from executor latency.
A separate correlated `StationPhyRxGain` message records action preparation,
DC execution, gain-bank publication, control execution and state advancement.
These stages are disjoint within RX gain; execution includes nested waits.
The host requires complete detail within the parent interval. Detail is sent
only after the maintenance round trip, never as per-action console output.
`timings: null` means no timing report was available, not zero hardware cost.
Physical failures return their stage without a timing snapshot; diagnostics-off
images have no PHY timing observer. See the
[observation contract](../../../crates/hardware/esp32s31/phy/src/tracking/README.md#timing-observations).

`cargo hil run diagnostic-station-phy-calibration --network patched-xarxa`
selects `station_pause = "calibration"`. It requests one due pass with an explicit
zero calibration threshold, retaining the actual sensor readings and scheduler
time. Success requires both common and Wi-Fi calibration completion, restored
station policy and continued UDP delivery without reconnecting. The ordinary
`station_pause = "tracking"` leaves the registered temperature policy intact;
`station_pause = "access"` checks the ownership round trip without tracking.
The protocol carries this choice as one operation, not independent booleans.

Separate PHY operation scenarios use the same correctness image:
`diagnostic-station-phy-temperature`, `diagnostic-station-phy-wifi-power`,
`diagnostic-station-phy-wifi-i2c`, `diagnostic-station-phy-common-calibration`,
`diagnostic-station-phy-tx-calibration` and
`diagnostic-station-phy-tracking-service`. Each runs a twelve-second UDP TX
workload. The common/TX scenarios force only their named measurement branch.
The service scenario enables a one-second observation cadence for a measured
four-second window, disables it, and waits for a final restoration barrier.
It requires a separate correlated service report; no operator-side delay or
manual router configuration is required. See the
[service ownership contract](../../../crates/hardware/esp32s31/phy/src/tracking/service/README.md).

`cargo hil run diagnostic-station-phy-rfpll --network patched-xarxa` requests
one measured RFPLL correction through the same exclusive station pause. The
host first completes a separate fresh temperature acquisition. The RFPLL
request rechecks sample age after admission and sets only this operation's
thermal threshold to zero; temperature,
capacitor delta and hardware statuses are not fabricated. It does not enable
registered periodic RFPLL, advance the periodic evaluation deadline or execute
power/RXCAL/TXCAL branches. The runner requires exactly one completed RFPLL
operation with nonzero timing, checked restoration and continued delivery in the
same connection. The correlated `StationRfpllObserved` detail distinguishes skipped evaluation,
zero correction and nonzero frequency-memory update. It retains the request's
sensor value and reference, selected threshold, committed reference, initial and
selected capacitor inputs, accepted sample count, updated entry count and
restored frequency index. These are procedure values, not RF lock evidence or
capacitor readback. The result is emitted after hardware-control restoration and
semantic commit, and serialized after the physical round trip unwinds.

`diagnostic-station-phy-rfpll-check` uses the same image and radio admission but
retains the ordinary 15-sensor-unit temperature threshold. Its result may be
skipped; the host verifies the branch and reference update against the reported
inputs. This conditional scenario uses the retained sample without refreshing it.

`diagnostic-station-phy-rfpll-observed` first requests a separate temperature
acquisition during UDP transmission. Only its correlated completion and checked
restoration allow the host to issue `rfpll-observed`. That operation retains the
ordinary threshold and rechecks sample age after physical admission, accepting
at most `STATION_RFPLL_SAMPLE_MAX_AGE_MICROS` (one second). Missing, stale or
invalid-clock samples do not execute RFPLL. No timed host sleep supplies readiness.
The host retains the first operation in `station-temperature.json` and RFPLL
in `station-pause.json`; a missing/deferred result fails the scenario.

`diagnostic-station-phy-rfpll-thermal-observed` keeps the same station and PHY
epoch under a 20 Mbit/s TX workload. After ten seconds of cold traffic, the
runner performs at most 45 fresh-temperature/RFPLL transactions at one-second
intervals and stops at the first nonzero correction. The attempt count, cadence
and initial delay are part of the scenario manifest; each attempt is retained
separately, while the canonical temperature and pause files contain the latest
attempt. The operator supplies only the thermal stimulus. The runner still owns
router setup, capture, maintenance requests and cleanup. The typed RFPLL
validation requires a nonzero capacitor delta, frequency-memory updates,
current-index restoration and publication of the fresh temperature as the new
reference.

`diagnostic-station-phy-temperature-thermal-control` uses the same 90-second
traffic load and a fixed 60-second exposure, but performs only temperature
acquisition. It characterizes degradation caused by sustained thermal stimulus
without an RFPLL correction; it never updates RFPLL frequency memory or the
retained RFPLL reference. Manual heating profiles are not assumed identical.

RFPLL detail includes `sample_age_micros`, measured from sensor acquisition
start to RFPLL entry, including acquisition waits and the subsequent handoff.
`None` means usable acquisition/observation timing is unavailable. The observed
and measured scenarios require a reported age within their bound. This bound is a diagnostic
freshness policy, not an established safe RF maintenance interval. These
scenarios do not enable automatic tracking or constitute a physical thermal sweep.

`StationPauseEvidence.elapsed_micros` measures the physical maintenance round
trip after the TX worker returns its paused owner and before the parent resumes
that worker. It excludes the earlier TX drain/admission wait and subsequent
worker scheduling. RFPLL timing is nested inside this interval; neither value
alone measures end-to-end traffic interruption.

Diagnostic physical transactions additionally return `timeline`, from selection
of the maintenance request through release of the restored worker into its
mailbox. Its adjacent intervals cover TX drain, MAC/RX/IRQ quiescence (including
optional PM=1), exclusive access acquisition, work, hardware restoration,
protocol restoration (including optional PM=0), and worker release. The runner
retains these as `exclusive_intervals` in `station-pause.json` and requires a
complete monotonic timeline when driver observation is advertised. Their sum
is the full software transaction duration; nested PHY timings are not added.
Worker release does not imply an executor poll or an over-the-air completion.
Neither this timeline nor its timestamps qualify RF-off or worst-case bounds.
Compact images and aggregate tracking-service windows omit the timeline;
physical failures do not manufacture a completed report.

Station PHY maintenance workloads support UDP RX, TX and bidirectional traffic.
`diagnostic-station-phy-calibration-rx` and `-bidirectional` execute the same
combined calibration request as the TX scenario. RX waits for a session-correlated
`UdpRxStarted` event after 256 valid datagrams; bidirectional also waits for host
reception. The paced host sender continues through maintenance and the original
measurement window is not shortened. `station-pause.json` retains the admission
milestones and correlated operation evidence. Completion counts establish child
coverage, not register readback, RF quality or event ordering. Radio restoration
must preserve the station epoch; transport results retain any delivery loss.

`diagnostic-station-phy-calibration-task-poll` executes the combined calibration
under the same sustained RX offer with the `diagnostic-task-poll` image. It is the
correlation profile for synchronous maintenance residence: the radio task's boot
maximum and `>5000 us` count identify the poll containing the transaction, while
`station-pause.json` retains the physical round-trip, child timings and timer
IRQ-to-dispatch interval. Network and UDP RX poll maxima come from the CPU1
executor in the split data plane and show whether the other core continued to
run. The boot maximum is a lifetime maximum, so it is evidence for the workload
interval only when the interval contains a long-poll increment and the value
correlates with the maintenance timing. This diagnostic does not establish an
acceptable production task-residence budget.
`diagnostic-station-phy-baseline-task-poll` is its same-image, same-load control
without a maintenance request.

With driver observation enabled, single-flow RX sessions retain the first eight
legacy/unknown PHY observations as `ORX_ANOMALY`, with UDP sequence (including
negative terminal markers), IP length, QoS identity when present, copied PHY
signal words and acquisition time. `ORX_ANOMALIES` reports the total even when
storage is full. Records are printed after collection, never from the RX hook.
`ORX_MAINTENANCE` brackets the control request and terminal response; its phase
is not a claim about the exact MAC/DMA exclusion interval. The baseline,
access-only, common-calibration, TX-calibration and combined-calibration RX
scenarios keep a 12-second 65-Mbit/s offer for isolating maintenance effects.

Delivery telemetry retains the first 16 forward UDP gaps with adjacent valid
QoS identities in the same TID. `ORX_GAP` reports the UDP and MAC sequence pair;
`ORX_GAPS` reports the correlated count so truncation remains visible. Missing
or incompatible metadata is not reconstructed. These bounded records survive
session completion, reset at the next session, and are printed after collection
outside the RX observer and its critical section. They distinguish UDP gaps
with continuous MAC numbering from losses of already numbered MPDUs; late
recovery and the delivery ledger remain separate evidence.

The correctness image also retains up to 32 ARP observations per UDP RX
session. `ORX_ARP` records decoded Ethernet/IPv4 ARP identity and radio,
network-admission or explicit rejection edges. With original/patched Xarxa,
the observer additionally records stack consumption and the result of the
stack's TX call (`TxAccepted`/`TxRejected`). Acceptance is queue admission, not
radio completion. `ORX_ARP_SUMMARY` includes the total so a truncated sample set
is visible. Records are frozen at session end and published through the console
capacity event after measurements; no USB wait enters packet processing.
These observations do not generate replies, reserve packet storage or change
stack backpressure. Pair them with `host-wire.pcapng` to distinguish neighbor
resolution stalls from radio delivery pauses.

## PHY RX timing profiles

`diagnostic-station-phy-rxcal-delivery-rx` uses ordinary diagnostic RX delivery
with whole-child, one-poll execution and disjoint DC, publication and control
phase timing. The direct transaction uses the same short ROM delays and bounded
status polling as the recovered vendor graph. Run it through
`cargo hil run <scenario> --network patched-xarxa`; router preparation, capture,
image selection and cleanup are part of the run.

`diagnostic-station-phy-rxcal-hot-sram-delivery-rx` is the paired placement
experiment. It places the direct source-owned RX-gain transaction in internal
SRAM without changing the graph, waits, workload or evidence contract. Compare
it with the ordinary profile to isolate code placement after timing parity.
