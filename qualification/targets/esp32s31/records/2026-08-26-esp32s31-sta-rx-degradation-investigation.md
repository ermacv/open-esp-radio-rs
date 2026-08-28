# ESP32-S31 STA RX degradation investigation checkpoint

Date: 2026-08-26 through 2026-08-28. Investigation started at `e5fbc5e0` and
continued after the reviewed squash at `fd207a57`.

This is an investigation checkpoint, not a qualification claim. Most runs
below used dirty sources or diagnostic images. The purpose of this record is
to preserve what was already tested, separate facts from hypotheses, and stop
repeating throughput-only A/B runs that no longer distinguish causes.

## Executive status

The historical STA RX expectation is valid: the 2026-08-13 qualification
record measured `108.752..116.172 Mbit/s`. One reproducible `7..10 Mbit/s`
loss in this investigation has a causal fixture-level explanation. A
byte-identical archived application that had produced `102.378..104.699
Mbit/s` measured only `94.559..96.798 Mbit/s` on OpenWrt channel 6, then
measured `101.772..103.119 Mbit/s` after changing only the AP primary channel
to 11. The application, runtime ELF, host route, HT40 width, MCS7 vector,
offer, and checksum policy were unchanged.

Channel 6 had `31.5%` idle CCA busy time versus `23.6%` on channel 11. An
independent capture identified a `FRITZ!Box 7530 FN` on channel 6 at
approximately `-28..-37 dBm`, close to the laboratory AP's signal. The
`7.9` percentage-point idle-busy difference matches both the direction and
magnitude of the recovered throughput. That exact archived-image degradation
was therefore an uncontrolled radio-cell precondition, not a DUT code
regression.

Current source independently reaches `109.310 Mbit/s` in the final
observer-free production image. Its normal saturated Core0 radio-poll residence is about
92--95%, not 50%, but the AP-to-target delivery frontier can still be the
immediate air ceiling. A separate 512-byte packet-rate control exposed and
then closed a Core1 scheduler defect: unbounded Embassy polling lost UDP
datagrams after successful driver enqueue even at only 40 Mbit/s. That defect
is distinct from the saturated full-size-frame ceiling.

The investigation also found real engineering concerns: exact ELF placement
can affect cache behavior, and retained RX credits are finite under overload.
Neither explains the identical-binary channel A/B or has been shown to cause
the latest intermittent underfed state. No current evidence supports a broken
A-MPDU/BlockAck agreement, a laptop WLAN route, checksum policy, or the OpenWrt
Ethernet path as the saturated ceiling cause. Removing the bounded Embassy
runner did cause a separate, now proven packet-rate delivery defect on Core1.

## Current working state

The former mixed dirty tree described by the early chronology below has been
split, reviewed and merged. The final scheduler delta is one commit over
`ed5cf11f`. Its shipping topology includes the synchronous ordinary RX path,
same-Core0 combined DMA/protocol owner, bounded affine SPSC and fixed
ring64/retained32 policy. The final delta also restores a production network
poll bound after delivery evidence proved that native unbounded polling can
starve the UDP consumer on Core1.

The production ceiling scenarios retain three repetitions and their published
acceptance thresholds. The new cache controls are explicitly diagnostic,
one-repetition scenarios with a low correctness floor; they are not
qualification gates. Laptop WLAN remains soft-blocked, its optional monitor is
disabled in these controls, and the validated host data route is Ethernet.

Cargo manifests and locks now use one minimal Embassy fork commit
`f7f09eb6`, rebased as a single commit over upstream `645068a3`. The fork adds
only bounded, directionally fair network polling and its tests; Embassy
support crates remain the published crates.io versions. The production build
is observer-free and passes the linked-image audits; HIL
cache/reorder/performance symbols are feature-eliminated from that image.

## Reconstructed chronology and useful A/B results

### Historical and scheduler boundary

| Source/image | Exact change or boundary | RX result | What it proves |
|---|---|---:|---|
| `54a84e33`, recorded 2026-08-13 | qualified split-core production RX | `108.752..116.172` | RX historically exceeded current TX-like rates |
| `030fd552`, clean | older production image | `93.969, 94.829, 94.734` | a later low baseline already existed before current `main` |
| `030fd552`, dirty archived variants | historical image overrides/layouts | `98.968..100.642`, then `96.256..96.452` | image identity mattered before the latest commits |
| `3715c848`, clean | fork `run_work_conserving()` | `87.007..88.157` | the custom cooperative wrapper was slow in this RX workload |
| `3715c848`, one-line native `Runner::run()` variant | same source base, native xarxa batch poll | `103.849..104.660` and `102.378..104.699` | native `run()` restored more than 15 Mbit/s in the preserved A/B |
| `3e6710d5` | committed native `Runner::run()` and route validation | historically intended fix; later clean rebuild `84.721..86.958` | source logic alone does not reproduce the old fast image |
| `9ab10458`, clean pre-`e5` | native `run()`, old Embassy fork dependency | `89.594..90.007` | pre-`e5` is not automatically fast |
| `e5fbc5e0` | upstream Embassy dependency | dirty builds described below reached `100+`; later builds ranged lower | switching away from the fork is not sufficient to explain the regression |

The preserved high native-run application is SHA-256
`11a3e4655016869edfac87babfc4066fe725d96cc6dbc9ac0da3f050b0b5e4d0`.
Its matching runtime ELF is SHA-256
`3d2bca832fa0a09f6bf304b590f158aa0ded5cbdb7e13783b8451a89064d22de`.
The source and build output still exist locally in the sibling
`open-radio-rx-embassy-ab` worktree. These artifacts must be preserved; they
allow an identical-source fresh rebuild rather than another approximate
commit comparison.

### Identical-binary and channel-only control

A fresh build from the preserved source was first performed from another
absolute worktree path. It differed from the archived image because Rust
embedded the longer absolute source root 177 times; `.rodata` grew by 720
bytes and shifted subsequent placement. Rebuilding from the original source
root into a separate output directory produced an application and runtime ELF
that were byte-identical to the archived fast artifacts. This proves the build
is deterministic when the absolute source root is held constant and explains
why a sibling-worktree rebuild was not initially an identical-source control.

The exact archived application SHA-256 `11a3e465...` and runtime ELF SHA-256
`3d2bca83...` were then rerun without rebuilding the DUT logic:

| AP condition | RX results, Mbit/s | Pre-air AQM drops | Reported retries/failed |
|---|---:|---:|---:|
| channel 6, laptop WLAN disabled | `94.559, 96.798, 95.302` | `6380, 18938, 6367` | `0, 0, 97` |
| channel 11, only AP channel changed | `101.772, 103.014, 103.119` | `6376, 6357, 6359` | `144, 256, 224` |

Run IDs are `1787781076298-002706c0` and
`1787781409278-00270960`. Both archived the same application and runtime
hashes. The faster channel reported more retries, while AQM drops were
essentially unchanged apart from one channel-6 outlier. AQM, retry, cache,
ring, checksum, and code-layout theories therefore cannot explain this A/B.

A clean detached `e5fbc5e0` current-main worktree was then built at an
absolute path of the same length as the archived fast worktree. This avoids
the already-demonstrated section-size change caused solely by a longer source
root. Its upstream-Embassy application SHA-256 was `961dc049...` and runtime
ELF SHA-256 was `ed3d8537...`. On channel 11, after an idle survey of
`2248/12002 ms` (`48/255`), it measured `101.330, 101.123, 100.602 Mbit/s`.
The former `90 Mbit/s` result therefore does not reproduce on clean current
main under the accepted RF precondition. The remaining mean difference from
the archived native-`run()` image is approximately `1.6 Mbit/s`, not the
original `7..10 Mbit/s`, and is not yet assigned to code or layout.

The first current-main repetition was formally failed despite
`101.330 Mbit/s` because its single post-load UDP terminal marker was lost;
the target reported `terminal=0` and timed out with otherwise valid transport
evidence. The next two repetitions saw `terminal=1`. This exposed a separate
HIL control-plane reliability bug: one lossy UDP marker was being treated as
a mandatory session terminator. The host sender now publishes a bounded
series of 16 four-byte terminal markers after the measured interval.

OpenWrt channel survey deltas measured at idle were:

| Channel | active | busy | busy fraction |
|---|---:|---:|---:|
| 6 | `12002 ms` | `3790 ms` | `31.5%` |
| 11 | `12002 ms` | `2837 ms` | `23.6%` |

The AP was returned to its original channel 6 after the controls. The runner
now samples hostapd's cumulative channel-active and channel-busy counters for
12 idle seconds before each ceiling repetition, scales the delta to 0..255,
and rejects a value above the explicit `64/255` threshold. A single hostapd
snapshot was deliberately rejected as insufficient because it varied across
survey intervals. The live channel-6 12-second sample was `3755/12002 ms`, or
`80/255`, so the robust gate rejects the contaminated condition before traffic
starts. All 158 host-runner unit tests and the complete scenario catalog
validation pass with the utilization gate and redundant terminal marker.

### Cache trace and layout

Three dirty `e5fbc5e0` performance images containing temporary cache-counter
instrumentation produced:

| Run ID | Cache trace enable | Application SHA prefix | RX result |
|---|---:|---|---:|
| `1787749970242-001c52b1` | `1` | `88aa0461d4ce` | `101.708, 101.890, 96.126` |
| `1787750287656-001c7c45` | `1` | `83811d5edd2d` | `100.126, 101.193, 100.877` |
| `1787750626735-001c88d7` | `0` | `b10214ec9faf` | `101.171` |

`trace=0x1` here is a hardware cache-counter enable bit, not a Cargo feature
and not Rust `log` trace. The `trace=0` image still reached `101.171 Mbit/s`.
Therefore enabling the counter peripheral is not the speed fix. All three
protocol `Hello` records used the ordinary performance capability set:
driver, task-poll, RX-delivery, MAC-IRQ, and network-scheduler evidence were
all disabled.

The cache-instrumented images were nevertheless different binaries. Their
runtime text/data boundaries were about 3 KiB later than ordinary current
builds. For example, the `101.171` image ended initialized payload at
`0x502bec41`, while a later upstream performance image ended it at
`0x502bdf71`. Thus the temporary instrumentation could improve performance by
changing placement even when its counters were disabled.

The external JTAG A/B reported during this investigation showed roughly
`3.9x` more Core 1 I-cache next-level traffic per useful byte in a slow image,
with Core 0 I-cache traffic per byte approximately unchanged. Shared D-cache
conflict/next-level events also rose about `2.6x..3.4x` per useful byte. That
is strong evidence for a network-core/layout problem, but the complete
slow/fast JTAG pair is not sealed in the HIL bundles in this tree. In
particular, `ACS_CONFLICT_CNT` must not yet be called a proven set-associative
aliasing miss; hardware exposes a separate miss counter.

Cache/layout remains a candidate for residual image-dependent CPU-ceiling
differences after radio utilization is controlled. It is not the cause of the
identical-binary channel A/B and not a justification for a magic padding
value.

### Feature and dependency A/B

Performance images are built with exactly:

```text
open-radio-hil,psram-task-stack,code-psram,profile-psram-data
```

The resolved feature graph contains no diagnostics, driver observation,
task-poll telemetry, MAC-IRQ telemetry, or RX-delivery telemetry. The
`max_level_trace` log dependency is paired with `release_max_level_info`, so
release trace logging is also absent from the hot path.

On current `e5fbc5e0`, two clean feature builds produced `95.013..98.261
Mbit/s`. Removing unused socket/protocol features (`raw`, DNS, ICMP, SLAAC)
did not restore the ceiling; the trimmed variants produced about
`94.071..94.839 Mbit/s`. This rejects the simple theory that an accidentally
enabled unused Embassy feature is consuming 10 Mbit/s. It does not reject a
layout change caused by feature selection.

The current-code dependency A/B was:

- upstream Embassy: same-boot samples `92.963, 96.455, 97.362 Mbit/s`;
- old Embassy fork: `89.955, 92.533, 89.181 Mbit/s`.

The old fork is not a cure in the current layout. `run_work_conserving()`,
`run_work_conserving_observed()`, `CooperativePollReport`,
`CooperativePollExit`, and `cooperative-scheduler-telemetry` are not required
by the current production runner. Native upstream `Runner::run()` calls the
batched xarxa interface poll and was the faster path in the preserved
`3715c848` A/B.

### Same-boot variability and startup gate

With one current upstream image and the ownership-correct ring policy,
same-boot sessions measured `92.963, 96.455, 97.362 Mbit/s`. Temporarily
restoring multiple concurrent `wait_config_up` callers measured `94.314,
96.678, 90.759 Mbit/s`.

The one-waiter startup gate fixes a real waker-ownership problem and improves
the worst case in this small sample, but it does not explain a stable 10
Mbit/s regression. Same-binary, same-boot spread also proves that ELF layout
cannot be the only source of variance.

Unsealed/no-firmware replay runs ranged from `93.485` to `103.327 Mbit/s`.
They are useful evidence that the cell can still exceed `100 Mbit/s`, but they
must not be assigned to a commit because the manifest did not archive the
flashed firmware.

## RX ring, BA16, and delivery localization

### Former discard policy

Under a 120 Mbit/s observed RX workload, the former policy exhausted 32 upper
staging credits, discarded `38k..42k` completed bulk units, and immediately
recycled their descriptors. It reported zero hardware `BUFFER_FULL`, because
software kept freeing the DMA ring by dropping already-received data. This
explains why earlier error counters could be zero while application packets
were missing: the loss was a deliberate software overload path, and ordinary
performance images did not publish driver observation.

### Ownership-correct preservation policy

The current dirty fix preserves the completed head until staging capacity
wakes. In the representative diagnostic run
`1787777822934-0026a816` it produced:

- software overload discard: `0`;
- DMA frontier maximum: `64` descriptors;
- admitted maximum per service: `32` frames;
- hardware `BUFFER_FULL`: `21` in the final interval, `33` over the three
  same-boot sessions;
- FIFO overflow: `0`;
- post-reorder to enqueue and enqueue to consumer defects: `0`;
- only `101774` of `163045` host datagrams reached post-reorder in the final
  interval.

This policy is ownership-correct, but it does not increase capacity. It moves
the visible loss boundary from "software discarded a completed frame" to
"hardware could not publish more completed frames while the ring was full".
The unresolved question is what stalls the upper consumer long enough to fill
the ring.

### BlockAck is not currently broken

The same diagnostic run showed:

- negotiated reorder window `16`;
- maximum reorder occupancy `15`;
- reorder missing `0`, gap expiry `0`, discarded `0`;
- `15` stale frames and no backward UDP observations;
- all benchmark data marked as A-MPDU by hardware provenance;
- the independent observer decoded `12638` full and `1212` partial BlockAck
  frames, `101830` unique acknowledged MPDUs, and zero backward BA starts.

This is incompatible with a lost BA agreement or a BA window accidentally
changed away from 16. The ring/consumer can still fail to absorb a valid BA16
burst.

## PHY, radio, and OpenWrt evidence

The laptop route was validated for every accepted run. A representative
artifact used interface `enp0s20f0u2u4c2`, route source
`192.168.178.129`, and the same bound socket source. Laptop WLAN is not the
benchmark route. The host offer stayed approximately `119.98..120.001
Mbit/s`, with sub-millisecond pacing lateness and no systematic catch-up.

The performance runner also rejects an HT40 ceiling sample unless the managed
AP snapshot reports MCS 7 and 40 MHz at either the 135 Mbit/s long-GI or 150
Mbit/s short-GI vector. Guard interval remains reported rather than requiring
one value, as requested after the original GI check proved too strict.

The OpenWrt Ethernet and host pacing paths are not the bottleneck. In the
channel-only A/B the host offered the same `120 Mbit/s`, OpenWrt admitted about
`163k` egress packets, and MCS7/HT40 remained unchanged. What changed was the
fraction of time for which the AP could obtain the medium. During channel-6
load, AQL normally sat at its configured `12000 us` high limit, but the same
behavior and approximately the same TID-0 AQM drops were compatible with
`103 Mbit/s` on channel 11. Those counters describe a saturated sender; they
are not the cause of the channel-dependent loss.

An external passive capture of the exact archived binary found that almost
every accepted aggregate advanced the BA start by 16 and carried a full
64-bit BA bitmap. Across the three intervals, the uniquely BlockAcked MPDU
counts agreed with target UDP delivery to within approximately twelve packets.
The missing offer was therefore not lost after DUT MAC acceptance. The slow
channel-6 sessions delivered fewer aggregates because their average BlockAck
cadence was about `2.01..2.06 ms`, rather than because BA16 or the DUT RX ring
discarded already acknowledged traffic.

The capture saw the same-channel `FRITZ!Box 7530 FN` at about `-36.6 dBm`, and
a later OpenWrt scan saw it at `-28 dBm`; the laboratory AP was about `-31 dBm`
at the independent monitor. This peer does not participate in the IP route,
but its transmissions and energy are necessarily included in the AP's CCA.

Expanded hardware counters are not zero. Representative observed intervals
contained FCS, abort, signal-field, baseband-restart, and unsupported-format
events, while hang/panic and FIFO-overflow counters remained zero. These PHY
counters observe the receiver/channel globally and cannot be attributed
one-for-one to the benchmark station. They prove that the former zero report
was an observation limitation, not that no radio events existed.

The PHY/Bluetooth ownership commits are present in the dirty `e5fbc5e0`
images that reached `100+ Mbit/s`. The Wi-Fi HIL does not schedule the BT
runtime or periodic PHY tracking, and shared modem clock leases remain held by
the Wi-Fi radio object. Those commits may still perturb final code placement,
but no active PHY/BT logic difference has been found that explains the
directional RX regression.

## Executor residence observation

The task-poll diagnostic run `1787778086807-0026ac38` measured approximately:

- network: `12575` polls, `668 us` average residence;
- protocol/reorder: `6858` polls, `1245 us` average;
- radio: `7822` polls, `912 us` average;
- UDP RX application: `5602` polls, `77 us` average.

These wall-time samples include interrupt preemption and heavy diagnostic
accounting. They show millisecond-scale residence and a network/radio service
capacity problem, but they cannot be used as production CPU cost. In the same
run the diagnostic image reached only `74.132 Mbit/s`, the RX frontier reached
64, and hardware `BUFFER_FULL` incremented 19 times.

## Checksum status

The production network-driver capabilities still use the established
`Checksum::Both` policy; xarxa performs the expected IPv4/UDP receive
verification. The same policy is present in the archived `100+ Mbit/s`
images and predates the current regression. There is therefore no commit-level
evidence that the former lwIP-compatible checksum scheme was removed. A
checksum A/B has not been run in the current dirty tree because it would mix
another code-layout change into an already uncontrolled image.

## Core 0 cycle localization

The 2026-08-27 `task-poll-telemetry` investigation replaced wall-clock
attribution with RV32 `mcycle` samples at synchronous ownership boundaries.
The instrumentation is intrusive and is not a production throughput baseline,
but its phase totals and per-frame normalization identify where Core 0 time is
spent.

In run `1787801699114-00287ce3`, the 16.061-second traffic interval delivered
143,101 UDP datagrams and serviced 143,321 RX MPDUs. Exact task residence was
8.917 seconds in the radio task and 5.653 seconds in the protocol task, or
about 90.7% of one Core 0. MAC ISR handling accounted for only 46.9 million
cycles, about 0.147 seconds, so interrupt execution is not the CPU limiter.

The largest normalized radio costs were:

- complete radio poll: about 19,765 cycles or 61.8 microseconds per MPDU;
- driver service inside the runner: about 10,655 cycles or 33.3 microseconds;
- fixed-pool stage/MPDU copy: about 5,144 cycles or 16.1 microseconds;
- recycle, publish, setup, frontier and reload together: about 5,300 cycles;
- scheduler entry before the runner plus yield/unwind after it: about 8,200
  cycles per MPDU in aggregate.

The largest normalized protocol costs were:

- complete protocol poll: about 12,487 cycles or 39.0 microseconds per MPDU;
- one ordinary frame body: about 6,770 cycles or 21.2 microseconds;
- dispatcher before publication: about 3,276 cycles or 10.2 microseconds;
- publication tail: about 2,604 cycles or 8.1 microseconds;
- frame-end to next dequeue plus dequeue to frame entry: about 4,325 cycles or
  13.5 microseconds.

The scheduling granularity is therefore a first-order architectural cost. The
same run performed 48,243 radio services for 143,321 MPDUs, only 2.97 MPDUs
per service, and every service ends with `yield_now()`. The protocol side also
reconstructs an outer `select(stop, dispatch_next)` and an inner ordered
`select(reorder_command, frame)` for each result. Core 0 consequently
ping-pongs between the radio and protocol state machines roughly once per
three MPDUs even though the explicit protocol fairness budget is 32.

Several controlled A/Bs bound smaller candidates:

- direct BlockAck alarm-slot traversal plus internal placement reduced the
  control-readiness predicate from about 2,710 to 2,144 cycles per scheduler
  pass, saving only about 0.069 seconds per traffic interval;
- moving the measured 2,524-byte staging ownership/copy function from cached
  PSRAM to internal executable SRAM reduced the stage phase from about 5,654
  to 5,223 cycles per MPDU, about 0.185 seconds per interval;
- a rotating staging-slot claim hint reduced that SRAM stage phase by a
  further 1.3%, about 0.029 seconds, proving that the linear atomic scan was
  not its dominant work under this load;
- a direct counter around two retained implementations measured the
  unconditional protected metadata decoder at about 817 cycles per MPDU from
  PSRAM and about 339 from SRAM, a maximum interval saving of about 0.214
  seconds.

These measured SRAM placements improve headroom, but together they recover
well below one second of Core 0 time and do not explain a 7--10 Mbit/s ceiling
change. The remaining large cost is the ownership/scheduling architecture and
the mandatory MPDU copy, not one missed hot helper. At the same time the AP
reported a 135 Mbit/s MCS7/HT40 long-GI transmit vector in every accepted
sample. A 115 Mbit/s UDP result would require 85.2% application goodput at that
PHY rate, so the current air vector remains the immediate throughput ceiling
while Core 0 scheduling remains the principal software-headroom constraint.

The follow-up run `1787803228892-0028afa5` split those remaining bodies at
additional synchronous boundaries. It passed at `103.569 Mbit/s`, delivered
141,430 UDP datagrams, negotiated BA16, classified all 141,444 benchmark MPDUs
as A-MPDU members, and reported zero hardware `BUFFER_FULL` or
`FIFO_OVERFLOW`. Normalized costs were:

- `stage_take`: about 1,254 cycles per MPDU;
- staging-pool allocation/copy: about 3,699 cycles per MPDU, 74.7% of the
  combined stage phase;
- the ordinary pairwise data body: about 3,198 cycles per MPDU;
- `view_protected_data`: about 2,329 cycles per MPDU, 72.8% of that data body;
- LLC decapsulation, replay, duplicate filtering and publication together:
  about 810 cycles per MPDU.

The separately measured normalized-metadata helper accounted for only about
327 of the 2,329 `view_protected_data` cycles. Static inspection found that
the SRAM entry returned to cached code for the shared layout and CCMP
validators. Placing only
`view_ccmp_data_with_fragment_admission` and `validate_ccmp_data` in the
existing RX hot section (about 1.1 KiB of code in this ELF) produced run
`1787803471600-0028bae2`: `data_view` fell from 2,329 to 1,660 cycles per MPDU
(-28.7%), protocol dispatch fell from 4,643 to 4,010 (-13.6%), and complete
protocol-poll cost fell 8.3%. Throughput changed from 103.569 to 104.596
Mbit/s. This proves a real, bounded instruction-placement cost but also proves
that it is not the missing 7--10 Mbit/s by itself.

Moving the inner generic `try_stage_unit` body to the hot section did not
reduce its phase (`3,699` to `3,868` cycles per MPDU in the paired run), so
that extra 2.5-KiB placement was removed. The remaining staging cost is real
lease/buffer work rather than an unqualified code-placement target.

Run `1787803723947-0028c03a` then tested the scheduler hypothesis by removing
the mandatory post-service yield only in the diagnostic image. That simple
work-conserving variant was worse: non-empty services increased from 46,629 to
83,998, MPDUs per service fell from 3.07 to 1.68, runner entries increased
from 49,753 to 131,273, radio residence increased from 8.73 to 9.50 seconds,
and throughput fell from 104.596 to 103.251 Mbit/s. The radio owner re-entered
before a useful frontier accumulated and created busy churn. The experiment
was reverted. A correct architectural change therefore needs bounded
coalescing or a combined producer/consumer turn; deleting `yield_now` or
restoring the older `run_work_conserving` wrapper is not such a change.

In the successful hot-validator run, radio plus protocol accounted for about
90.2% of Core 0. Approximately 14,398 of 32,441 measured cycles per MPDU
(44.4%) were outside the two useful service/frame bodies. Diagnostic atomics
inflate that absolute fraction, so the boundary totals alone do not prove that
all of the residual belongs to Embassy `select` or channel operations. MAC ISR
residence remained only about 0.146 seconds per interval.

Run `1787804325375-0028cccb` verified the final retained source after removing
the generic staging placement and restoring the mandatory yield. It passed at
`104.255 Mbit/s`, retained BA16, classified all 142,178 benchmark datagrams as
A-MPDU, and reported zero hardware `BUFFER_FULL` or `FIFO_OVERFLOW`. The first
attempt, `1787804079663-0028ca91`, never entered the datapath: station candidate
selection failed three times. The failure exposed a laboratory-configuration
mismatch rather than a radio regression. OpenWrt was actually on primary
channel 11 with HT40-below while `hil/local.toml` still declared channel 6 with
HT40-above. Updating the ignored local configuration to 11/HT40-below restored
`cargo hil doctor` and station association.

Run `1787804675545-0028d09f` then measured the deep profiler's own aggregate
updates. It passed at `105.418 Mbit/s`, serviced 143,970 MPDUs and measured
96.45 million cycles, or about 670 cycles per MPDU / 0.301 seconds per traffic
interval, in the explicitly timed DMA, runner, scheduler, dequeue, frame-entry,
frame-result, data-profile and publication counter updates. One final atomic
update per category and poll-wrapper accounting remain outside that self-cost,
so the subtraction is a lower bound rather than a claim of zero observer
effect.

After subtracting the directly measured self-cost, radio plus protocol still
accounted for at least 32,087 cycles per MPDU, or 89.9% of the 16.058-second
interval. The corrected split in that ELF was:

- radio DMA service body: about 10,081 cycles per MPDU;
- protocol frame body: about 8,492 cycles per MPDU;
- radio scheduler/wrapper work outside the service body: about 6,826 cycles;
- protocol poll work outside the frame body: about 6,688 cycles.

The largest radio service phases were the staging lease/copy at 5,191 cycles
(1,492 for take/bookkeeping and 3,699 for pool claim/copy), publication at
1,313, recycle at 1,148, setup at 1,025 and frontier discovery at 704 cycles
per MPDU. The pool/copy component reproduced the earlier 3,699-cycle result
despite the changed diagnostic layout.

The radio scheduler boundary was also localized. Resume/async re-entry cost
about 2,182 cycles per MPDU and polling `yield_now` plus unwinding from the
completed service to the executor poll boundary cost another 1,799. Stop
probing cost 407, housekeeping 810, TX-state checks 691 and final RX checks
234 cycles per MPDU. Removing the yield remains invalid because the earlier
A/B proved that it decreases useful batching; these numbers instead quantify
the cost of crossing that boundary roughly once per 3.44 MPDUs.

On the protocol side, corrected dequeue/select/state transitions accounted for
about 6,093 cycles per MPDU: about 852 from poll entry to first dequeue, 3,464
between a completed frame and the next dequeue after allocating the measured
frame-result observer cost, and 1,777 from dequeue to frame entry after
allocating the measured dequeue observer cost. This is about 91% of corrected
protocol work outside the frame body. It is therefore a real dominant boundary,
but not yet proof that one specific `select` implementation is responsible;
channel state, future reconstruction, wake bookkeeping and the unmeasured final
observer atomics share that interval.

Within the successful ordinary data body, the protected view was about 1,524
cycles, decapsulation 227, replay 74, duplicate filtering 46 and publication
429 cycles per completed data MPDU. MAC ISR work remained only 330 cycles per
MPDU / 0.149 seconds per interval. BA reorder, radio errors and the ISR are
therefore not the observed Core 0 cycle consumers in this sample. Checksum
work was not a separate Core 0 boundary in this profile and this run does not
replace the still-required checksum-mode A/B.

## Conclusions by confidence

### Proven

- Historical and archived current-cell RX can exceed `100 Mbit/s`.
- The laptop sends over the Ethernet route with the correct source address.
- Native Embassy/xarxa `Runner::run()` was faster than the old 64-ingress
  cooperative wrapper for the preserved full-size-frame A/B, but it is not a
  correct shared-executor policy: a 512-byte control proves that it can starve
  the socket consumer and lose already-enqueued UDP datagrams. The accepted
  replacement uses a 32-ingress bound against a 64-packet socket reserve.
- Hardware cache counter enable itself is not the speed increase.
- Performance does not accidentally enable diagnostic features.
- BA16 negotiation and reorder behavior are intact in the observed run.
- The final retained diagnostic source passes at `104.255..105.418 Mbit/s` on
  channel 11/HT40-below with zero hardware RX buffer/fifo errors.
- The local HIL channel geometry had drifted from the live OpenWrt AP; doctor
  now passes after correcting the ignored local config to 11/HT40-below.
- The exact same application/ELF is `6.2..8.6 Mbit/s` faster when only the
  OpenWrt HT40 primary channel changes from 6 to 11.
- Channel 6 currently has about `7.9` percentage points more idle CCA busy time
  and a same-channel FritzBox at near-laboratory signal strength.
- The current `94..97 Mbit/s` result is an invalid radio-cell ceiling sample,
  not evidence of a DUT RX regression.
- The old ring policy silently dropped completed bulk RX frames under staging
  pressure; the preservation policy exposes a real 64-descriptor hardware
  capacity limit instead.
- Neither the one-waiter gate nor unused network features explains the
  saturated full-size-frame ceiling. The upstream switch did independently
  remove a required packet-rate fairness boundary.

### Separate, strong but not yet causal for code robustness

- Network-core instruction layout/cache behavior determines a substantial
  part of the processing ceiling.
- Dirty diagnostics can saturate staging and the hardware ring when Core
  1/network consumption cannot keep pace with BA16 bursts.
- Diagnostic instrumentation can worsen AP/radio behavior and must not be used
  as a direct throughput baseline.
- Deep phase telemetry directly consumes at least about 670 cycles per MPDU;
  corrected Core 0 residence is still at least 89.9% in the diagnostic image.

### Still open

- How much of the corrected approximately 6,093-cycle protocol transition is
  channel/waker work versus reconstruction of the ordered stop, command, frame
  and gap-timer futures.
- Whether a bounded combined radio-producer/protocol-consumer turn can reduce
  the two approximately 6.7k-cycle non-body boundaries without violating stop,
  reorder-command, RX-before-TX and ring-ownership ordering.
- Which exact poll/future/queue/stack working set causes the excess Core 1
  cache traffic.
- Whether a clean current-main application differs from the archived native
  `run()` application when both are tested under the same accepted channel
  utilization precondition.
- How much residual same-binary spread remains after rejecting busy-channel
  sessions.
- Whether a small, architecturally selected IRAM/DRAM hot set can make
  performance robust after the working set is identified.
- Disabling the flash cache mapping after PSRAM handoff, a controlled
  IRAM/DRAM placement, and a checksum-mode A/B have not produced valid results
  yet. They are intentionally deferred until the identical-source control is
  established.

## Experiments that must not be repeated unchanged

- Any ceiling run whose pre-workload channel utilization exceeds the explicit
  laboratory threshold.
- Another raw pre-`e5` versus post-`e5` throughput run.
- Another comparison against the rejected 64-ingress fork without the
  512-byte delivery ledger.
- Another cache trace bit on/off run.
- Another broad feature-disable build without an identical ELF/source control.
- Another multiple-waiter versus one-waiter run as an explanation for 10
  Mbit/s.
- Another magic padding/alignment sweep presented as a final fix.
- Another diagnostic-throughput comparison interpreted as production speed.

## Next decisive experiment

Do not repeat the removed no-yield or broad SRAM experiments. The next Core 0
experiment must target an ownership boundary and preserve the existing
ordering contracts. The candidate is one bounded producer/consumer turn which:

1. retains stop-first and reorder-command-first polling;
2. services one finite DMA frontier;
3. drains only the frames produced by that frontier, with a hard BA16/ring64
   bound, before surrendering Core 0;
4. retains RX-before-TX fairness and yields at the existing external boundary;
5. is accepted only if the corrected radio/protocol non-body cycles fall while
   BA16, sequence evidence, zero buffer/fifo errors and throughput remain
   non-regressed.

Before interpreting that semantic A/B, move the deep counter aggregation out
of the intervals it describes or keep the explicit self-cost subtraction. A
magic layout or a raw throughput increase without lower boundary cycles is not
evidence for this architectural hypothesis. SRAM remains appropriate only for
the already proven small CCMP validator set or as a controlled causal test.

## 2026-08-27: descriptor-retaining zero-copy and Ethernet-style copy A/B

The subsequent ownership work replaced the old mandatory DMA-to-stage payload
copy with an affine external-buffer handoff. The physical ring still has 64
descriptors, and at most 32 original DMA buffers may be retained by the
protocol/network path. A descriptor is rearmed only after its exact buffer
lease returns. The standalone station DMA producer and 802.11 protocol
consumer now run in one bounded Core 0 service turn; the Ethernet frame is
published to the Embassy stack on Core 1.

An attempted descriptor-refill implementation mixed the new buffer mapping
with connected-epoch lifecycle changes and failed before traffic with a Rust
``async fn resumed after completion`` panic in the supervisor response future.
The same panic reproduced with copied network publication, zero spare buffers,
and an immutable target-address binding while that integrated patch remained.
After removing only the incomplete refill changes and restoring the known
descriptor-retaining owner graph, the panic disappeared. Run
`1787824506568-002985c4` passed at `104.606 Mbit/s`, BA16 remained active, and
hardware reported `BUFFER_FULL=0` and `FIFO_OVERFLOW=0`. This localizes that
startup failure to the incomplete integrated refill/lifecycle work; it is not
evidence that the split Core 0/Core 1 topology is invalid.

A clean one-line publication A/B then kept the same radio, protocol, stack,
feature set and task-poll image but copied each decoded Ethernet frame into the
Core 1 endpoint-owned queue. This makes the hardware descriptor independent of
Core 1 immediately, matching the vendor static-RX to dynamic-RX ownership
order, but pays one complete payload copy on Core 0.

| Handoff | Run | RX | Core 0 runner cycles | Protocol-poll cycles | Protocol publish-tail cycles | HW full/overflow |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| retained DMA buffer, shared zero-copy | `1787824506568-002985c4` | 104.606 Mbit/s | 3,272,627,562 | 1,783,451,977 | 486,337,768 | 0 / 0 |
| copied Ethernet frame, immediate DMA release | `1787824731710-002987bb` | 104.830 Mbit/s | 4,009,685,163 | 2,987,753,307 | 1,625,729,198 | 0 / 0 |

The `0.224 Mbit/s` throughput difference is noise at this offered load. In
contrast, the copied path added about 737 million runner cycles (+22.3%) and
1.139 billion publish-tail cycles, approximately 7,960 cycles per delivered
frame. Radio task residence also increased from 14.847 to 15.147 seconds of a
roughly 16.06-second interval. Therefore retaining up to half of the DMA ring
across Core 1 is not the present throughput limiter, while restoring the
vendor copy is a measured Core 0 regression.

The shared run also localizes the current Core 0 work. Of 3.173 billion cycles
inside the measured runner driver body, 1.193 billion belong to the DMA service
and 1.783 billion to the protocol poll. Within the DMA service, setup/frontier
cost 648 million cycles and detach/pool handoff cost 353 million. Within the
protocol path, per-frame bodies cost 1.134 billion cycles; queue/poll work
outside those bodies accounts for the remaining roughly 650 million. The
final Core 0 to Core 1 shared publication itself measured only 64 million
cycles. Moving the Embassy IP stack onto Core 0 would add the already measured
12.44 seconds of network-task residence to the saturated radio core and is
therefore architecturally opposite to the required direction.

The local ESP32-S31 GMAC comparison is not a zero-copy cross-core design. Its
`RxToken::consume` invokes the network-stack callback synchronously on the same
core and rearms the fixed descriptor only after that callback returns. It is
fast because there is no executor/core ownership boundary, not because it
lends a physical descriptor to another core. Espressif's documented Wi-Fi
design similarly copies a static hardware RX buffer into a dynamic upper
buffer and immediately returns the static buffer. Linux page-pool and DPDK
instead decouple the two without copying: a completed packet buffer leaves the
RX queue and the descriptor is replenished with another buffer from a pool.

The best long-term Wi-Fi design is the latter refill model, but it is a
robustness and CPU-headroom change, not a current 104-to-115 Mbit/s explanation.
It must be implemented as a separate typed buffer-pool owner: descriptor
identity and buffer identity must no longer be the same index; a live mapping
must be updated atomically with descriptor rearm; the released upper buffer
must return to a bounded free pool; and stopped-epoch normalization must reject
any outstanding lease. No supervisor future or role lifecycle should change in
the first hardware proof. The acceptance sequence is: cold permuted binding,
one live replacement with payload identity verification, bounded burst refill,
then full upper handoff.

Two observer-free performance attempts, `1787825097142-00298a79` and
`1787825238177-00298baa`, were correctly rejected before traffic because the
OpenWrt pre-workload utilization was `66/255` and `68/255`, above the unchanged
`64/255` ceiling threshold. They contain no throughput result and must not be
used to weaken the accepted diagnostic A/B.

The first refill prerequisite was then isolated from the connected lifecycle.
The diagnostic image cyclically bound descriptor `n` to physical buffer
`n + 1` while every CPU payload, guard, detach, release, validation and recycle
path used the explicit binding. Run `1787825639880-00298fe3` passed at
`101.566 Mbit/s`, delivered 138,512 CRC-checked UDP datagrams, retained BA16
and A-MPDU provenance for every benchmark MPDU, and reported zero hardware
`BUFFER_FULL` or `FIFO_OVERFLOW`. This is target evidence that descriptor
identity can differ from buffer identity; it is not yet a live-replacement
result. The forced rotation was removed from the ordinary diagnostic image
after the proof, while the storage mapping API and host regression remain.

## 2026-08-27: live-refill rejection and final RX ownership decision

The page-pool proposal was then implemented and measured instead of accepted
from analogy. A completed DMA buffer was detached, its descriptor received a
different free allocation, and descriptor publication remained owned by the
single Core 0 ring service. Successive variants removed obvious accidental
costs: one refill per MPDU, one batched prefix append/reload, an atomic free
bitmap instead of a 96-buffer scan, and immediate parking of the large
external owner in a static handoff slot.

| Ownership path | Run | RX | DMA service cycles / MPDU | HW full/overflow |
| --- | --- | ---: | ---: | ---: |
| live refill, one replacement per MPDU | `1787857807375-002a009a` | 94.451 Mbit/s | about 10,756 | 0 / 0 |
| live refill, batched plus free bitmap | `1787858651770-002a1346` | 96.036 Mbit/s | about 10,500 | 0 / 0 |
| live refill, static READY handoff | `1787859039400-002a208b` | 94.341 Mbit/s | about 10,602 | 0 / 0 |
| descriptor-retaining control | `1787859697839-002a3699` | 101.724 Mbit/s | about 8,918 | 0 / 0 |

The READY run split the added work further. Publishing the parked owner cost
about 960 cycles per packet, close to the retained path. The live descriptor
refill itself cost about 2,488 cycles per packet, while detach/handoff cost
about 2,287 cycles. Returning production to descriptor retention removed
about 1,684 DMA-service cycles per MPDU (15.9%) and recovered 7.38 Mbit/s in
the same current laboratory configuration. A preceding control attempt
`1787859529930-002a2ee4` failed during association before the datapath became
active; repeating the same image produced the accepted run above, so it is a
startup flake and not refill evidence.

This result supersedes the earlier statement that page-pool refill is the best
long-term Wi-Fi design for this target. It is a valid design in systems where
dynamic descriptor replenishment is cheap or where lending any RX allocation
is forbidden, but neither condition requires it in the measured workload. The
implemented S31 replacement path made descriptor preparation, guard
restoration, buffer claim/binding and list publication Core 0 work on the
already saturated radio owner. That implementation cost more than bounding
cross-core retention to half of the fixed ring; this does not prove that every
possible implementation of the hardware mechanism has the same cost.

The selected production design is therefore:

- Core 0 exclusively owns radio DMA and 802.11 protocol processing;
- Core 1 owns Embassy IP processing and sockets;
- a 64-entry fixed RX ring may lend at most 32 exact descriptor/buffer pairs;
- the retained cap guarantees at least 32 descriptor credits, a reserve equal
  to two BA16 MPDU windows in the current one-MPDU-per-descriptor model;
- returning the affine network lease marks that exact buffer released;
- Core 0 rearms only the contiguous released prefix in one bounded append;
- overload or malformed units are recycled immediately, without copying;
- descriptor-to-buffer identity remains explicit and is regression-tested by
  a cold permuted mapping, but the live address table is immutable.

The local GMAC design remains a useful same-core reference, not a template for
the cross-core boundary: GMAC invokes the stack synchronously and rearms after
the callback. Moving Embassy networking to Core 0 would combine its measured
load with the saturated radio/protocol owner. Copying, meanwhile, already
added 22.3% runner cycles. The bounded retained ring is the measured minimum
Core 0 ownership adaptation among the three implemented choices.

The live-refill pool, mutable address binding, replacement APIs and their
production allocation were removed after this control. The retained DMA,
MAC, and adapter ownership regressions pass 47/47, 168/168, and 14/14 tests
respectively. This resolves the newly introduced 94 Mbit/s architecture
regression; it does not by itself establish the residual ceiling above the
historically observed 104--105 Mbit/s.

## 2026-08-27: checksum, synchronous descriptor transaction and Core 0 SPSC

A same-runtime-ELF checksum-policy A/B then separated Core 1 checksum work
from code-layout changes. Run `1787861713642-002ae339` executed both policies
without reflashing:

| RX checksum policy | RX | Network-task residence |
| --- | ---: | ---: |
| software checksum | 101.514 Mbit/s | 12.510 s |
| assume-valid diagnostic | 102.534 Mbit/s | 10.026 s |

The software checksum is real Core 1 work: bypassing it removed about 20% of
the measured network-task residence. It recovered only 1.020 Mbit/s while
Core 0 radio/protocol residence remained about 14.3 seconds of the 16-second
workload, so checksum is not the present RX ceiling. The diagnostic capability
remains explicit and is not the production default.

The S31 DMA service was next made structurally synchronous. Its implementation
now completes descriptor observation, affine detach/recycle and list
publication before returning a ready future; no descriptor transaction can
cross an executor suspension. Completed descriptor/unit tokens are also
explicitly non-`Send`; only the detached packet-buffer owner is allowed to
cross a core boundary. The two HIL controls after that correctness change were
`1787862182893-002aedac` at 103.657 Mbit/s and
`1787862518019-002af3b7` at 102.730 Mbit/s. Both retained BA16, zero station
retries and zero hardware buffer-full/FIFO-overflow events. The result is a
type-topology/correctness improvement, not an independently established
throughput increase.

The useful untested part of the subsequent external page-pool review was not
dynamic descriptor replacement, which the live-refill A/B above had already
rejected. It was the proposed single-producer/single-consumer ownership
topology. The standalone station DMA-to-protocol queue still used the general
Embassy channel even though both endpoints are unique Core 0 owners. That
channel was replaced only at this internal boundary by an affine bounded SPSC
ring with separate 64-byte-aligned producer and consumer cursors. The station
plus AP routed queue remains unchanged. Sender and receiver cannot be cloned,
only one pair may be active in an owner epoch, cursor publication is
Release/Acquire, and dropping the queue releases every still-published affine
frame.

| Internal DMA-to-protocol handoff | Run | RX | DMA service cycles / MPDU | Protocol-transition cycles / MPDU | Protocol dispatch cycles / MPDU | Frame-body cycles / data MPDU | Core 0 runner-driver cycles / MPDU |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| general channel control | `1787862182893-002aedac` | 103.657 Mbit/s | 8,218 | 3,699 | 8,515 | 2,552 | 22,680 |
| general channel control, same ELF repeat | `1787862518019-002af3b7` | 102.730 Mbit/s | 8,227 | 3,693 | 8,512 | 2,552 | 22,713 |
| bounded SPSC | `1787863009142-002b89d8` | 103.961 Mbit/s | 8,575 | 2,821 | 8,307 | 2,569 | 21,692 |
| bounded SPSC, same CRC repeat | `1787863193062-002b8e42` | 103.645 Mbit/s | 8,567 | 2,824 | 8,302 | 2,568 | 21,682 |
| bounded SPSC plus non-`Send` DMA token | `1787863496458-002b9654` | 104.166 Mbit/s | 8,551 | 2,826 | 8,303 | 2,564 | 21,679 |

Both SPSC runs used runtime CRC32 `063fcd23`. They retained MCS7/40 MHz,
BA16/A-MPDU provenance for every benchmark MPDU, zero retries and zero
hardware buffer-full/FIFO-overflow events. The measured protocol transition
fell by about 24%, and total Core 0 runner-driver work fell by about 4.5%.
This counter is the sum of poll-to-first-dequeue,
preceding-frame-to-next-dequeue and dequeue-to-frame-body intervals; the last
interval includes BA reorder ingress work, so it must not be labelled as pure
queue implementation cost. The per-data-frame body changed by less than 0.7%,
while the two same-CRC repeats agree within four transition cycles and eleven
runner cycles per MPDU. This is therefore accepted as an architectural
hot-path reduction, not inferred from the small raw-throughput movement alone.
Adding the zero-sized non-`Send` marker changed the linked runtime CRC to
`1ad92553` through Rust type/code layout, but its normalized counters remained
inside the SPSC repeat range. That third run is the exact current source and
confirms that the correctness marker adds no measured hot-path work.

The same measurement also bounds the claim made by the external review.
Separating descriptor identity from packet-buffer identity is a sound generic
RX architecture, and any such design requires a synchronous rearm-or-drop
starvation policy. It is not the S31 performance solution measured here:
immediate copied rearm did not increase throughput, live replacement added
about 1,684 DMA cycles per MPDU and lost 7.38 Mbit/s, while retained controls
showed no catastrophic hardware-ring exhaustion. This does not erase the
backpressure coupling: a delayed Core 1 can still consume up to the bounded
32-credit allowance. It establishes only that the coupling is not a
significant performance cost in the measured ceiling workload. The selected
S31 design therefore continues to separate cores and deliberately retains at
most 32 exact descriptor/buffer leases.

Several details in that review describe invariants which the selected path
already has. When no upper credit exists, an ordinary overload unit is dropped
and its same descriptor/buffer is rearmed; another core is never awaited for a
replacement. Reorder storage is already selective-copy: an in-order MPDU keeps
the zero-copy path, while only a frame whose lifetime crosses a BA sequence gap
is copied to independent reorder backing. Partial overload is handled per
unit, not as an all-or-nothing refill batch. Descriptor address publication
was also validated with a permuted binding and three saturated live-refill
runs, including ring wrap; it is functional but measured slower.

The review's warning that a copied diagnostic might still retain the
descriptor does not apply to run `1787824731710-002987bb`: that path copied the
terminal Ethernet frame and released the original DMA owner immediately
before Core 1 consumption. The 0.224 Mbit/s throughput difference alone is not
causal evidence because both paths reached the contemporary air ceiling. The
system result is nevertheless decisive for the current design: immediate
release produced no compensating system gain, added 22.3% Core 0 work, and the
retained control had no hardware exhaustion. The diagnostic matrix therefore
rejects descriptor replacement as the current speed fix while retaining the
synchronous-token and SPSC ownership improvements.

## 2026-08-27: fixed-per-service versus per-MPDU decomposition

The SPSC result did not establish that the remaining protocol-transition
counter was pure queue cost, nor that low-level descriptor parsing should be
optimized first. A task-poll-only histogram therefore grouped every DMA
service by the number of admitted MPDUs, including zero-work runner entries.
It also measured the exact local SPSC push/pop operations and reported the
counter-update self cost separately. Production ownership and scheduling
policy were unchanged.

Two runs of runtime CRC32 `9cfe45d9` passed with AP-to-station MCS7/40 MHz at
135 Mbit/s (long GI), reverse-link MCS7/40 MHz at 150 Mbit/s (short GI),
BA16/A-MPDU provenance for every benchmark MPDU, zero retries and zero
hardware buffer-full/FIFO-overflow events:

| Run | RX | Runner entries | MPDUs | Mean MPDUs/entry | Fitted fixed cycles/service | Fitted cycles/MPDU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `1787864522524-002bdcf6` | 105.689 Mbit/s | 48,100 | 144,340 | 3.001 | 13,203 | 4,676 |
| `1787864780388-002be062` | 106.730 Mbit/s | 48,415 | 145,774 | 3.011 | 13,390 | 4,601 |

The weighted least-squares fit over actual `0..32` service bins is stable
between the same-CRC runs. Its phase coefficients are:

| Phase | Fixed cycles/service, run 1 / run 2 | Incremental cycles/MPDU, run 1 / run 2 |
| --- | ---: | ---: |
| setup | 9,046 / 9,098 | 464 / 423 |
| frontier | 1,202 / 1,249 | 733 / 723 |
| admission | 27 / 27 | 61 / 61 |
| descriptor take | 911 / 941 | 1,207 / 1,196 |
| external handoff pool | 247 / 248 | 1,123 / 1,126 |
| internal publication | 981 / 1,024 | 1,037 / 1,025 |
| tail | 790 / 804 | 50 / 47 |

Thus the former approximately 2.9k setup cycles/MPDU were mostly an
amortized approximately 9.1k fixed setup cost per runner entry, not intrinsic
per-packet DMA work. At the measured batch of three, the fitted complete
service cost is about 9.1k cycles/MPDU including empty entries. Holding the
coefficients constant would predict about 8.0k at four MPDUs/entry and 6.3k
at eight. Those are projections, not optimization evidence, but they make a
same-ELF batching/coalescing A/B the next decisive experiment.

The queue data also bounds the direct-dispatch hypothesis. Both runs had one
successful SPSC push and pop per MPDU, no full push, and one terminal empty pop
per active service. Exact push cycles plus *all* pop cycles were 459.0 and
459.3 cycles/MPDU respectively. This is a conservative upper bound on the
mechanical SPSC operations which direct dispatch could remove; it is far below
the clean 2.82k protocol-transition counter. The remainder includes protocol
turn organization and BA reorder ingress. Moreover, the 1.12k external-pool
cost is the retained buffer owner which later crosses Core 1, not the local
SPSC, and cannot disappear merely by calling the protocol processor directly.

A fully direct async callback from the synchronous DMA transaction would also
violate the selected ownership topology whenever network backpressure can
suspend. A valid fast path must finish the descriptor transaction first and
either collect a finite detached batch or prove a synchronous/nonblocking
ordinary protocol path, retaining the SPSC only for backlog. The current
measurements show no backlog in this workload, so such a fast path remains a
valid later experiment. It is secondary to batching because its measured
mechanical queue upper bound is about 0.46k cycles/MPDU versus a projected
2.7--2.8k opportunity from increasing the mean service batch from three to
eight.

## 2026-08-27: minimally observed occupancy and offered-load sweep

The deep phase image cannot establish production Core 0 occupancy by itself.
It executes timer reads and atomic updates at many ownership boundaries and
changed both the linked image and the measured workload. In its two same-CRC
runs the radio future occupied 92.780% and 93.472% of the traffic interval.
The phase observers self-accounted for about 3.3 percentage points. Independent
32-bit `mcycle` deltas, corrected for one wrap, measured 318.092 and 318.078 MHz
during those radio-poll intervals, consistent with the configured 320 MHz core
clock and with the wall-time accounting.

To remove that confound, runtime image classes now distinguish:

- `diagnostic-task-residence`, which wraps only the top-level futures and has
  no driver observation or per-phase cycle counters;
- `diagnostic-task-poll`, which retains general driver observation; and
- `core0-rx-cycle-telemetry`, which explicitly opts into the intrusive phase
  profiler and service histogram.

This separation also fixed a runner-policy error: the minimal residence image
had previously been rejected for not emitting typed radio evidence even though
its purpose was deliberately to omit driver observers.

The minimal image first passed a saturated run at 106.443 Mbit/s. The radio
future occupied 14.947759 of 16.050295 seconds, or 93.131%; the unobserved
remainder was only 6.869%. This remainder is an *upper bound* on Core 0 idle,
not an exact idle measurement, because other Core 0 execution outside the
wrapped future is omitted while interrupts occurring inside a poll are
included. Core 1 network plus UDP residence was 91.055%. The AP reported
MCS7/HT40 at 135 Mbit/s with long GI for the downlink and 150 Mbit/s with short
GI for the reverse link, with zero retries and failures.

A four-point same-ELF sweep then used runtime CRC32 `a4dd7899`:

| Offered UDP | Delivered UDP | Datagrams | Core 0 radio residence | Core 0 unobserved upper bound | Core 1 network + UDP |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 30 Mbit/s | 29.994 Mbit/s | 40,753 | 49.235% | 50.765% | 29.864% |
| 60 Mbit/s | 60.003 Mbit/s | 81,517 | 75.249% | 24.751% | 53.850% |
| 90 Mbit/s | 89.958 Mbit/s | 122,221 | 91.076% | 8.924% | 75.161% |
| 120 Mbit/s | 105.678 Mbit/s | 144,092 | 92.235% | 7.765% | 85.051% |

This directly rejects the premise that Core 0 is about 50% occupied at the
current ceiling: about 50% applies only at a 30 Mbit/s offered load. It also
shows that Core 0 residence is not linear in delivered frames. Radio time per
datagram fell from about 193.3 microseconds at 30 Mbit/s to 102.8 microseconds
at saturation as fixed service work was amortized over larger batches. The
small increase from 91.076% at 90 Mbit/s to 92.235% at 105.678 Mbit/s means a
linear extrapolation to a CPU ceiling is invalid without a controlled change
to the air/service ceiling.

A four-point linear fit for Core 1 gives approximately 8.80 percentage points
of intercept, 0.7307 points/Mbit/s and R-squared 0.998, extrapolating to 100%
near 124.8 Mbit/s. This is a short local model, not proof of a 124.8 Mbit/s
limit. It is sufficient to show that moving the IP/UDP stack onto the already
busy Core 0 would reduce, not improve, available CPU headroom.

## 2026-08-27: delivery-frontier localization at the current ceiling

The independent-laptop monitor could not be used: its Wi-Fi PHY was
soft-blocked and non-interactive `sudo` could not change that policy. The
follow-up therefore used OpenWrt station counters plus the target's typed
delivery frontiers. The OpenWrt monitor interface captured only 62 frames and
is not quantitative evidence for this run.

Run `1787867005847-002c87b2` passed at 107.796 Mbit/s with:

- 163,037 host-generated datagrams;
- 163,000 OpenWrt Wi-Fi-egress datagrams;
- 147,176 AP station transmissions, zero retries and zero failures;
- 147,159 target post-reorder frames and 147,159 network enqueues;
- 147,011 UDP-consumer deliveries;
- 147,385 target hardware-service admissions and zero `BUFFER_FULL` or
  `FIFO_OVERFLOW` events;
- 6,342 OpenWrt AQM drops.

The AP's actual station-transmit count and the target's post-reorder count
differ by only 17 frames. Conversely, the difference between host generation
and AP station transmission is 15,861 frames, essentially the complete
host-to-target deficit before end-of-window effects. Of that deficit, 6,342
frames are explicitly accounted for by OpenWrt AQM drops; the remainder was
not transmitted during the measurement window and can be queued at another
AP boundary or remain after the interval. These counters do not distinguish
those two cases.

The supported conclusion is therefore narrow but strong: in this run, frames
which the AP reported as transmitted reached the target post-reorder frontier,
and there is no evidence of an on-air retry/loss or target DMA/MAC drop causing
the 120 to approximately 108 Mbit/s reduction. The immediate measured ceiling
is the AP's pre-air scheduling/queueing plus its 135 Mbit/s long-GI downlink
service rate. This is not a general claim that radio conditions can never
matter; it is a correlation for the current AP, channel and interval.

## 2026-08-27: ownership audit and corrected credit publication

The selected production topology remains:

- Core 0 owns DMA, MAC, CCMP and BA processing;
- Core 1 owns IP, UDP and socket processing;
- the physical RX ring has 64 descriptors;
- at most 32 exact descriptor/buffer leases may be retained above DMA;
- the negotiated BA receive window is 16;
- descriptor/completion tokens are non-`Send`; only detached packet-buffer
  ownership can cross an executor/core boundary.

The audit found one real ordering defect in
`ExternalRxHandoffSlot::release`. It previously published `SLOT_FREE` before
dropping the old `ExternalRxBuffer`. The buffer's drop callback is what marks
the physical DMA allocation released, so another Core 0 admission could claim
the newly free handoff record during a short cross-core window while the old
physical credit was still detached. That transiently violated the intended
32-credit upper bound.

Release now holds `SLOT_RELEASING`, drops the old buffer and completes its DMA
release callback, and only then publishes `SLOT_FREE` with release ordering.
The focused regression test asserts that the callback still observes
`SLOT_RELEASING`. This is a correctness repair; because the observed ceiling
runs reported neither pool exhaustion nor hardware buffer-full, there is no
evidence that this race caused their throughput ceiling.

Post-fix run `1787867428060-002c92df`, runtime CRC32 `57a070c2`, passed at
105.506 Mbit/s. The minimally wrapped radio future occupied 14.867200 of
16.044006 seconds, or 92.665%, leaving a 7.335% unobserved upper bound on Core
0 idle. Core 1 network plus UDP residence was 88.764%. The AP again reported
135 Mbit/s MCS7/HT40 long-GI downlink, 150 Mbit/s short-GI reverse link, zero
retries and zero failures. The repair therefore does not introduce a visible
throughput or occupancy regression; this one run does not estimate how often
the repaired race occurred before the fix.

The audit also found stale resource comments which still described the former
copy-before-reload design. They now document the actual bounded retained-buffer
model. The common ordinary RX path does not await Core 1 capacity before
returning from its Core 0 protocol turn; copied reorder/reassembly and overload
paths remain separate bounded cases.

The current architecture is directionally correct but not fully proven for a
higher PHY ceiling. Dynamic replacement/page-pool is not the selected next
step: the measured implementation added about 1,684 cycles/MPDU and reduced
throughput, while copied immediate release added 22.3% Core 0 work without a
system gain. The next causal architecture experiment is a same-ELF bounded
coalescing A/B, retaining the current ownership model and measuring throughput,
radio residence, service-batch histogram, retained-credit watermark, release
latency and drops. A direct in-owner batch may then remove the measured local
SPSC mechanical upper bound of about 459 cycles/MPDU, but only after the DMA
transaction has ended and without allowing a descriptor token to cross an
`await`.

Before interpreting a throughput gain from either experiment, the AP downlink
must be moved above the current long-GI ceiling while the target records the
per-frame HT-SIG MCS, width and GI distribution. Otherwise a CPU optimization
can increase headroom without changing the already air-limited application
rate, and a latest-rate snapshot can hide mixed PHY vectors.

## 2026-08-28: safety-boundary audit of the retained SPSC path

The repository safety audit found that the first SPSC implementation had put
its `UnsafeCell`/`MaybeUninit` ring directly in the otherwise safe ESP32-S31
Embassy adapter. Its cursor algorithm had focused tests, but this still
violated the intended architecture: the adapter package is compiled with
`forbid(unsafe_code)`, while raw affine storage primitives belong in an
explicitly audited ownership foundation.

The generic non-blocking ring now lives in `open-esp-radio-dma` as
`AffineSpscQueue<T, DEPTH>`. The unsafe surface is limited to slot
initialization/move/drop and the `Sync` proof. The public producer and consumer
endpoints enforce one active pair per epoch, use cache-line-separated cursors,
return the complete value when full and contain no executor or Wi-Fi policy.
The ESP32-S31 adapter is again safe code and only maps the generic errors and
adds optional timing observation. Twelve foundation tests and 242 adapter
tests pass; `tools/audit-driver-safety.sh` reports 24 safe packages and 12
audited-unsafe packages.

The audit also rejected unsafe `link_section` attributes on two diagnostic
counter statics in the safe adapter. Those statics now use ordinary placement.
Consequently, deep phase-profile numbers from before and after this change are
not a placement-controlled A/B and must not be compared directly. The minimal
task-residence image does not enable those counters, so its occupancy evidence
is unaffected by this diagnostic placement change.

The architecture audit was updated to encode the newly separated feature
contract: `task-residence-telemetry` must not enable intrusive integration
telemetry, while `core0-rx-cycle-telemetry` must enable it. Both the driver
safety and driver architecture audits now pass. The complete
`tools/audit-source-only.sh` proceeds through those gates and later stops at
Blobray publication because the local
`generated/findings/review-scopes.json` input is absent; this is an evidence
workspace prerequisite, not a compiled RX failure.

Two post-move runs used the exact same runtime CRC32 `284109c7`:

| Run | RX | Datagrams | Core 0 radio | Idle upper bound | Core 1 | Radio us/datagram | AP downlink |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `1787868417900-002d5d29` | 108.756 Mbit/s | 148,285 | 94.856% | 5.144% | 86.237% | 102.71 | 135 Mbit/s MCS7/HT40 LGI |
| `1787868579907-002d5f62` | 106.146 Mbit/s | 144,600 | 92.761% | 7.239% | 84.121% | 102.91 | 135 Mbit/s MCS7/HT40 LGI |

Both runs reported zero AP retries and failures. Their application throughput
and total residence varied with the number of frames delivered during the
interval, but Core 0 radio time per delivered datagram differed by only about
0.2%. This is evidence that the safe-foundation move did not add a per-packet
regression. It is not evidence for a throughput improvement: the AP's pre-air
AQM drops differed substantially between the two intervals and both remained
under the same long-GI downlink vector.

The combined occupancy evidence is now unambiguous at the current ceiling:
Core 0 is not approximately 50% used. Across minimally instrumented saturated
runs it occupies about 92.2--94.9% of the interval. The architecture work is
therefore justified as CPU-headroom work. It must not, however, be described as
the proved cause of the current 105--109 Mbit/s application ceiling, because
the delivery-frontier run localizes that interval's missing traffic before AP
station transmission and the AP remains at 135 Mbit/s long GI.

## 2026-08-28: guard-interval control was mutating the AP

The first strict GI scenarios did not merely validate a received HT vector.
They used `iw ... set bitrates ... lgi-2.4/sgi-2.4` on the mt7915 AP. Upstream
mt76 implements that request through the firmware `RATE_PARAM_FIXED_GI`
parameter. A same-application-ELF air-observed A/B established the effect:

| AP policy | Run | RX | Target HT40 MCS7 | Independent BlockAck result |
| --- | --- | ---: | --- | --- |
| automatic | `1787871599954-002e2aab` | 102.641 Mbit/s | 20,959 LGI / 119,087 SGI | 8,793 full BA, about 15.92 MPDU/BA |
| fixed long GI | `1787871676121-002e2fdb` | 69.540 Mbit/s | 95,044 LGI / 0 SGI | 18,894 full BA, about 5.03 MPDU/BA |
| fixed short GI | `1787871747721-002e375c` | 75.674 Mbit/s | 0 LGI / 103,442 SGI | 20,573 full BA, about 5.02 MPDU/BA |

All three received only HT40 MCS7 benchmark frames and reported zero hardware
buffer-full/FIFO-overflow. Both fixed-GI choices collapsed aggregation from
about sixteen to about five MPDUs per BA exchange. The loss is therefore not
the direct 400 ns GI airtime difference and not evidence about target code
placement. It is a fixture mutation which changes mt7915 firmware behavior.

The HIL schema now separates `[link].guard_interval`, which only validates
target observations, from
`[fixture_mutation].openwrt_fixed_guard_interval`, which explicitly opts into
the AP mutation. Ceiling scenarios require automatic GI. The target reports
complete per-frame MCS0..7 LGI40, SGI40 and HT20 histograms; a latest-rate
snapshot can no longer satisfy a strict link claim.

## 2026-08-28: deep-observer and backpressure audit

Several diagnostics had been returning plausible zeroes while not observing
the intended boundary:

- preserved bulk admission was omitted from the software-backpressure count;
- the Core0 cycle feature had no explicit HIL image and did not attach the RX
  pipeline observer;
- the combined `ORXS` line exceeded the UART record limit and was discarded;
- the host parser still required removed legacy `ORXHT*` text lines, then
  silently converted that parse failure to `None` even though authoritative
  typed PHY evidence was present.

The service observation now reports `stage_capacity_blocked`, boot-wide pool
and queue credit floors, and a separate `ORXSC` credit record. The runtime has
an explicit `diagnostic-core0-rx-cycles` image, and the host overlays typed PHY
vectors onto optional text diagnostics instead of discarding the complete RX
pipeline. Focused regressions cover the new fields. These are measurement
repairs; old `back=0` text alone must not be used as proof that no software
credit pressure existed.

One additional profiling defect was found during the architecture audit.
`stage_copy` was not a copy phase. It was exactly the derived sum
`stage_take + stage_pool`, printed next to both constituents. Adding all three
double-counted about 381 million cycles in a typical run and encouraged a
false payload-copy interpretation. It is now named `stage_total` and has a
regression asserting that it is derived, not exclusive.

## 2026-08-28: same-ELF fast/slow localization

The corrected deep image used runtime CRC32 `34120aa6`. It produced both a
fast and a severe slow state without changing the target image:

| Run | RX | Target MPDUs | Core0 radio residence | Pool / queue floor | `abort_fcs_pass` / `other_unicast` |
| --- | ---: | ---: | ---: | ---: | ---: |
| `1787873803415-002ef8a7` | 51.374 Mbit/s | 71,115 | 7.733 / 16.299 s = 47.45% | 20 / 20 | 5,229 / 4,420 |
| `1787874033541-002f0093` | 105.687 Mbit/s | 144,180 | 15.073 / 16.063 s = 93.84% | 15 / 16 | 619 / 269 |
| `1787874157812-002f041c` | 102.649 Mbit/s | 140,018 | 14.711 / about 16.06 s | 16 / 18 | 675 / 345 |
| `1787874217488-002f0f4f` | 104.052 Mbit/s | 141,935 | 14.895 / about 16.06 s | 16 / 18 | 545 / 205 |
| `1787874280355-002f1064` | 104.871 Mbit/s | 143,039 | 14.959 / about 16.06 s | 15 / 17 | 443 / 112 |

Every row reported zero software drops, zero bulk-capacity-blocked services
and zero hardware buffer-full/FIFO-overflow. In the slow run Core0 was
underfed, not saturated, and at least twenty credits remained in both bounded
domains. That run therefore proves that CPU occupancy and retained-ring credit
pressure are not a universal explanation of the instability.

The slow run also delivered a contiguous BA stream with no reorder missing or
gap expiry, while its UDP sequence evidence contained 119,080 unrecovered
observations. It had no independent air capture, so the missing MSDUs cannot
yet be assigned to AP pre-air scheduling or to an S31 PHY abort before DMA.
The counter correlation is nevertheless specific: across the fast rows,
`abort_fcs_pass - other_unicast` stayed near 330--350, whereas the slow row had
4,261 additional `other_unicast` events and a residual of 809. Most of the
abort increase therefore tracks frames classified as other unicast. This is
evidence of a different receive environment, not proof that those frames
caused the complete throughput loss.

Three fast runs had independent passive air evidence. In each, independent
unique BlockAck MPDUs and target benchmark MPDUs differed by only 2--3 frames,
the observer reported zero kernel drops, and almost every BA acknowledged a
full sixteen-MPDU window. Thus the ordinary fast state has no unexplained loss
between air and the target protocol boundary.

The OpenWrt fixture now records channel busy/active counters over the measured
session and station TX duration in addition to the existing optional idle
sample. Run `1787874502453-002f1436` passed at 106.853 Mbit/s with 145,796
independent MPDUs versus 145,793 target MPDUs, workload utilization 174/255
(12,229/18,004 ms busy/active) and 13.671 s station TX duration. The channel
busy counter includes the AP's own traffic, so it cannot by itself quantify
foreign airtime; the two values must be compared in a future captured slow
run. The diagnostic also now rejects a pre-workload idle utilization above
64/255.

## 2026-08-28: corrected current Core0 cost model

Run `1787874502453-002f1436` delivered 145,996 DMA units. The exclusive DMA
service phases were:

| Exclusive phase | Total cycles | Cycles/MPDU |
| --- | ---: | ---: |
| setup | 383,913,737 | 2,630 |
| frontier | 117,760,709 | 807 |
| admission | 11,798,113 | 81 |
| descriptor take | 207,319,389 | 1,420 |
| external handoff pool | 173,664,431 | 1,190 |
| local SPSC publication | 164,993,413 | 1,130 |
| tail | 115,501,869 | 791 |
| complete DMA service | 1,174,951,661 | 8,048 |

The former `stage_copy=380,983,820` was exactly descriptor-take plus handoff
pool and is deliberately absent from the exclusive sum. No ordinary payload
copy occurs there.

The complete protocol-frame owner used 1,715,286,041 cycles, or about 11,749
cycles/MPDU. The final protected-data body was about 2,474 cycles/data MPDU;
the rest includes preflight, BA/reorder ingress, dispatch and publication of
the retained in-place Ethernet view. Raw SPSC push plus all pop operations
were about 337 cycles/MPDU, so deleting only the local queue cannot remove the
multi-thousand-cycle protocol cost. The complete radio runner consumed about
26,968 cycles/MPDU and occupied 15.180 of 16.065 seconds, 94.49%. Core1 network
plus UDP occupied about 80.8% of its interval. Moving IP/UDP onto Core0 would
therefore combine two busy owners and is rejected by the measured residence.

The adjacent single-core GMAC experiment is not contradictory. Its raw
descriptor-turnover loop reaches 870--888 Mbit/s, but one full CPU data pass
reduces that to 217--239 Mbit/s and the complete TCP TX path reaches about
150 Mbit/s only with PSRAM code, SRAM data, SRAM DMA/ISR and optimized software
checksum. Wi-Fi additionally performs 802.11 parsing, CCMP/replay/duplicate
work, BA processing and cross-core packet publication. The Ethernet result
proves that S31 DMA hardware is capable; it does not prove that the Wi-Fi
protocol owner should be 50% idle at a 105--107 Mbit/s UDP rate.

The current architecture is therefore directionally correct: fixed ring64,
bounded retention32, BA16, same-Core0 DMA/MAC/protocol ownership, selective
copy only for reorder/reassembly, and Core1 IP/sockets. Dynamic replacement
and unconditional copy remain rejected by measured CPU cost. The affine SPSC
endpoints were additionally made `!Sync` while remaining movable, so safe code
cannot share one endpoint between concurrent producers or consumers and
invalidate the cursor proof.

The next architecture experiment is not another ring rewrite. At this point
the batch histogram had not yet been collected, so same-ELF service coalescing
and direct ordinary dispatch both remained candidates. The later exact-current
controls below supersede their ordering: they show that queue removal alone is
bounded by the measured approximately 338-cycle SPSC cost, while the ordinary
async protocol wrapper has a much larger measured opportunity. In parallel,
the intermittent underfed state must be captured with the new AP airtime
counters and independent observer before assigning it to radio or PHY.

## 2026-08-28: exact-current-source occupancy controls

Two final controls used the current source after the `stage_total` correction
and affine-endpoint safety repair.

Deep run `1787875062238-002f7b72`, runtime CRC32 `60260ce0`, passed at
105.966 Mbit/s. It observed 144,518 independently BlockAcked MPDUs versus
144,515 target benchmark MPDUs, with 9,058 full and four partial BA responses.
Every target benchmark frame was HT40 MCS7: 20,978 long-GI and 123,537
short-GI. Pool/queue credit floors were 15/16 and every software-drop,
capacity-block, hardware BUFFER_FULL and FIFO_OVERFLOW count was zero. Core0
radio residence was 15.108692/16.057922 seconds, or 94.09%; Core1 network plus
UDP residence was 80.28%.

That run also exposed a diagnostic-domain defect: Core0 phase aggregates are
u32 cycle counters, and a 320 MHz counter wraps after 13.42 seconds. Its
individual DMA/protocol/runner totals remained below the wrap point, but the
complete raw `radio_poll_cycles` total did not. No occupancy conclusion here
uses that wrapped field. The cycle scenario is now limited by catalog
validation to twelve seconds. Safe-length control
`1787876019861-002fd86c`, using the same runtime CRC32 `60260ce0`, passed at
105.727 Mbit/s. Core0 radio residence was 11.444060/12.065746 seconds, or
94.85%, and its raw cycle total was an unwrapped 3,648,373,594. Core1 network
plus UDP residence was 80.31%. Independent capture observed 108,350 MPDUs
versus 108,346 target benchmark MPDUs, with 6,770 full and four partial BA
responses. Every target benchmark frame was again HT40 MCS7; credit floors
were 19/19 and all software/hardware exhaustion counters were zero.

The less intrusive residence-only run `1787875437985-002f8299`, runtime CRC32
`f3356530`, passed at 105.009 Mbit/s. Its Core0 radio future occupied
14.665044/16.059815 seconds, or 91.32%, while Core1 network plus UDP occupied
84.94%. This image does not publish per-frame driver evidence; its managed AP
snapshot still reported HT40 MCS7 and its independent PCAP is retained. The
strict complete-vector claim comes from the deep run, not from the AP's latest
rate snapshot. The runner now rejects explicit per-frame MCS/GI requirements
for images which cannot publish driver observation instead of silently
accepting them, and performance reports preserve requested independent-air
evidence.

The safe-length deep decomposition gives a mean non-empty DMA frontier of 4.73
MPDUs. DMA service cost was 8,113 cycles/MPDU; complete protocol-poll cost was
17,787 cycles/MPDU, of which frame dispatch occupied 11,818 cycles/MPDU. The
complete runner cost was 27,300 cycles/MPDU. Setup alone averaged 12,588 cycles
per non-empty service, or 2,660 cycles/MPDU at the observed batch size. This is
measured evidence for a bounded coalescing experiment, not yet evidence that a
particular larger batch improves total CPU cost. Raw SPSC operations were only
338 cycles/MPDU. Directly measured deep-telemetry counter maintenance was about
781 cycles/MPDU, so production cost must not be inferred by subtracting or
extrapolating one intrusive run; the independent residence-only run supplies
the occupancy control.

## 2026-08-28: architecture decision after the occupancy proof

The measured residence is active `poll` residence, not a PMU measurement of
retired instructions. It includes interrupt preemption and memory stalls, but
excludes time for which the radio future returned `Pending`. It therefore does
not prove that Core0 retires useful instructions for 94.85% of the wall-clock
interval. It does prove the narrower fact needed for executor architecture:
the radio owner gives at most 5.15% of that fast interval back to the executor.
The independent residence-only image bounds that returned time at 8.68% while
still delivering 105.009 Mbit/s. A plan which assumes an approximately 50%-idle
Core0 is consequently inconsistent with both controls.

The safe cycle-domain control is stronger than residence alone. At 320 MHz,
the unwrapped 3,648,373,594-cycle radio poll total equals 11.401 seconds, or
94.49% of the 12.065746-second interval. The combined runner itself accounted
for 2,962,130,708 cycles (9.257 seconds, 76.72%). Within it, measured DMA
service accounted for 2.751 seconds (22.80%) and protocol poll for 6.031
seconds (49.99%), including 4.007 seconds (33.21%) in frame dispatch. These
regions are elapsed core-cycle regions and include stalls rather than a retired
instruction count, but they cannot be executor-idle time. The DMA and protocol
regions also approximately close against the complete driver region, which
accounted for 75.75% of the interval.

The next production change should target measured synchronous-path overhead,
not deliberately delay initial descriptor processing to manufacture larger
batches. Recovered vendor control flow immediately drains the currently
available RX-success prefix and appends released blocks back to the receive
list; no hardware RX interrupt threshold or coalescing timer has been recovered
for this target. The setup result (2,660 cycles/MPDU at a mean batch of 4.73)
makes bounded coalescing a useful diagnostic A/B, but not yet a justified
production policy.

The ordinary shared split-core path currently calls
`wait_staged_ready().await` even though its implementation selects an
immediately-ready future whenever the shared in-place publisher is installed.
In the safe run this nominal wait region still accounted for about 906
cycles/frame. Complete protocol poll cost exceeded frame dispatch by about
5,969 cycles/frame, whereas the raw local SPSC cost was only 338 cycles/frame.
The first architecture experiment should therefore be a synchronous ordinary
frame path for the already-ready shared publisher, with the existing async
path retained only for A-MSDU, reorder/reassembly and bounded-copy cases. Cold
error/configuration branches should be outlined from that ordinary path.

The ELF already places the lower measured descriptor, pool, CCMP and protected
data helpers in `.rwtext.open_radio_rx_hot` in internal SRAM. The large async
producer/protocol/dispatcher state machines remain in cached external text.
Further SRAM placement is justified only as a same-ELF A/B for small measured
helpers after the synchronous-path split; moving whole async state machines or
choosing a magic alignment is not a stability design. Espressif's own Wi-Fi
configurations also reserve IRAM for selected hot Wi-Fi/RX functions, but that
is supporting precedent rather than evidence that cache placement caused the
intermittent 51 Mbit/s state.

The slow state remains a separate open failure. It had only 47.45% Core0 radio
residence, credit floors of 20/20 and no software or hardware exhaustion. Thus
it is neither explained by the fast-state Core0 ceiling nor by ring retention.
Until it is reproduced with simultaneous independent-air MPDU evidence and AP
session airtime/rate counters, the boundary is only known to be before the
target DMA/protocol accounting; assigning it to AP scheduling, S31 PHY aborts,
cache layout or RF would exceed the evidence.

One subsequent independent-air control, run `1787876860825-00300dff` with
runtime CRC32 `60260ce0`, passed at 103.867 Mbit/s. It observed 106,449 unique
BlockAck MPDUs versus 106,446 target benchmark MPDUs, zero capture drops,
6,668 full and four partial BA responses, credit floors 15/16 and no
BUFFER_FULL/FIFO_OVERFLOW. The laptop observer was a standalone HT40 monitor;
the route to the DUT remained on Ethernet, and laptop WLAN was software-blocked
again after the capture.

The existing same-firmware slow/fast pair also narrows the open failure more
than throughput alone. Slow run `1787873803415-002ef8a7` and fast run
`1787874280355-002f1064` have the same application SHA256 and runtime ELF
SHA256. Per accepted frame, the slow/fast pipeline times were 24.51/23.80 us
for DMA service, 13.57/13.48 us for dispatch, 5.59/5.56 us for publication and
1.15/1.15 us for the nominal ready wait. The already-admitted software path
therefore did not become twice as slow. The divergent evidence is before DMA:
the slow run recorded 6,547 RX aborts, 5,229 FCS-pass aborts, 4,420
other-unicast events, 315 NRX restart errors and 692 NRX service errors; the
fast run recorded 449, 443, 112, 46 and 2 respectively. BUFFER_FULL and
FIFO_OVERFLOW only describe ring exhaustion and remained zero in both; the
radio/receive-error counters were not zero. Their public semantics are not
sufficiently complete to assign causality, but their correlation identifies
the exact capture boundary for the next recurrence.

## 2026-08-28: same-ELF ordinary-admission A/B

The ordinary retained-frame path used to call `wait_staged_ready().await`
even when the shared publisher already owned the exact staging slot which it
would publish. This was not a capacity wait: the shared implementation always
returned `Ready` immediately. The sink contract now exposes a stable typed
admission capability. Only a sink whose staging slot is itself the complete
output credit may select synchronous admission; copied and independently
queued sinks retain the capacity wait. A diagnostic selector can only force a
shared sink back onto the former slower path and cannot grant the synchronous
capability to another sink.

Run `1787883338432-0031aa4a` exercised both policies from one flash and one
diagnostic ELF, runtime CRC32 `7ef8c9b7`. The target logged the selected policy
at startup, and the two catalog scenarios had identical image, workload, link
and evidence contracts.

| Policy | RX | Ready waits | Wait cycles/frame | Protocol frame cycles/MPDU | Protocol poll cycles/MPDU | Runner cycles/MPDU |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| synchronous shared | 105.505 Mbit/s | 0 | 0 | 10,931.76 | 17,024.96 | 26,561.98 |
| deferred-ready control | 104.998 Mbit/s | 107,602 | 918.28 | 11,699.77 | 17,769.52 | 26,971.10 |

The synchronous capability removed every nominal wait and reduced normalized
protocol-frame cost by 768 cycles/MPDU (6.6%), complete protocol-poll cost by
745 cycles/MPDU (4.2%), and complete runner cost by 409 cycles/MPDU (1.5%).
The complete DMA service varied in the opposite direction, 7,911 versus 7,700
cycles/MPDU, together with a different service-batch distribution. Therefore
the 0.507 Mbit/s throughput difference at the HT40 air ceiling is not used as
the causal evidence; the eliminated wait count and nested cycle reductions
are the evidence.

Both controls retained BA16, zero reorder occupancy/missing/gap expiry, full
A-MPDU provenance, zero station retries/failed transmissions, and zero
hardware BUFFER_FULL/FIFO_OVERFLOW. Independent observation differed from the
target by three MPDUs in each policy and saw almost exclusively full sixteen-
MPDU BlockAck responses. Hardware abort/restart counters were also comparable;
this was not a recurrence of the earlier underfed state.

This change is a valid production simplification, not a Core0-ceiling fix. The
exact Core0 radio-poll counters occupied 94.17% of the synchronous interval and
93.90% of the deferred interval; Core1 network plus UDP task residence was
85.29% and 85.57%, respectively. More packets were processed by the faster
path in the same interval, so total occupancy did not fall. A claim that Core0
is now approximately half idle remains false.

For the synchronous run, complete protocol-poll time closes exactly as:

| Nested protocol-poll region | Cycles/MPDU |
| --- | ---: |
| poll start to first dequeue | 588.56 |
| prior frame end to subsequent dequeue | 1,054.93 |
| dequeue to frame-dispatch entry | 3,800.08 |
| complete frame dispatch | 10,931.76 |
| last frame end to poll exit | 649.17 |
| complete protocol poll | 17,024.96 |

Raw SPSC push plus pop remains only about 334 cycles/MPDU. Consequently,
deleting the queue cannot by itself explain or remove the approximately 6,093
cycles/MPDU outside frame dispatch. The next architecture step must first
separate command/deadline and async-state-machine cost inside the measured
dequeue-to-entry and inter-frame regions. A synchronous ordinary dispatch with
the async path retained only for reorder/reassembly/A-MSDU is a justified
same-ELF candidate; calling the entire 6,093-cycle remainder queue overhead is
not justified by these measurements.

The observer-free production feature set was then built independently. Its
application SHA256 is `802cec3a...`, runtime CRC32 is `dcdbc1d0`, and the
application occupies 2,898,896 of the 4,194,304-byte partition. Placement,
stack-frame, autonomous-source-graph and serialized-log-writer audits passed;
the diagnostic deferred-admission symbol is absent from the runtime ELF.
Run `1787884057659-0032014e` passed at `105.861 Mbit/s` from an actual
`120.001 Mbit/s` host offer. The pre-workload channel utilization was 54/255,
the AP reported MCS7/HT40, and station retries/failed were zero. This run has
no driver or independent-air observer and is therefore only a shipping-feature
throughput non-regression, not additional BA/PHY correctness evidence.

## 2026-08-28: retired instructions and same-ELF cache-counter control

The cycle profile was extended with coarse, paired `mcycle` and `minstret`
samples around the complete Core0 radio poll, runner transaction and protocol
poll. This addresses a limitation of the earlier occupancy wording: poll
residence is not synonymous with useful instruction execution. Per-frame CSR
reads were deliberately avoided at these three boundaries. A separate phase
profile decomposes only synchronous BA reorder ingress and explicitly accounts
for its own counter updates.

The first completed profile, run `1787886107321-00328a6b`, passed at
`105.401 Mbit/s`. The 12.058104-second interval contained 3,626,470,787 radio
poll cycles and 818,237,191 retired instructions. At a measured 320 MHz this
is about 94.0% Core0 radio-poll residence and 4.43 cycles per retired
instruction. The runner used 2,953,519,999 cycles / 751,126,242 instructions
and the nested protocol owner used 1,976,447,959 / 478,106,897. This is direct
evidence that the normal saturated state is not a 50%-residence state. It is
not evidence that the remaining cycles are all cache stalls.

Hardware L1 cache counters were then exposed through the safe S31 CACHE PAC.
The reported bus names are deliberately kept as `IBUS0/1` and `DBUS0/1`; they
are not relabelled as CPU identities. `HIT`, `MISS`, `ACS_CONFLICT` and
`NEXT_LEVEL` are also kept as separate events. In particular, the existence
of a distinct hardware miss counter makes it invalid to call every access
conflict a set-associative conflict miss.

Enabling those counters changes peripheral state, so a runtime selector and a
same-image scenario pair were added. Run `1787886776974-0032c322` executed
both policies from runtime CRC32 `46bcab46`:

| L1 counters | RX | UDP / admitted frames | Core0 radio cycles / instructions | Runner cycles / instructions | Protocol cycles / instructions |
| --- | ---: | ---: | ---: | ---: | ---: |
| off | 106.831 Mbit/s | 109,422 / 109,586 | 3,662,331,938 / 839,750,279 | 2,952,611,333 / 769,160,621 | 1,942,476,078 / 489,150,534 |
| on | 106.652 Mbit/s | 109,279 / 109,441 | 3,653,765,159 / 839,161,452 | 2,941,561,267 / 768,366,358 | 1,932,302,845 / 488,432,203 |

The `0.179 Mbit/s` difference is 0.17%. Normalized cycle and instruction
totals also agree within about one percent. Enabling the hardware event
counters is therefore non-intrusive at the resolution of this workload. The
preceding cache-enabled run `1787886473817-00329495` reached only
`93.230 Mbit/s`, but this same-ELF control proves that the trace bit did not
cause that state. The slow-ish run admitted fewer frames, reduced Core0
residence to about 86%, and diverged before the measured DMA path, like the
earlier 51 Mbit/s underfed state.

For the cache-off control, the radio-poll region occupied 94.89% of the
320 MHz interval and retired about 69.6 million instructions/second. Its CPI
was 4.36; runner CPI was 3.84 and protocol CPI was 3.97. Subtracting nested
regions gives a more useful architectural split:

| Exclusive region | Cycles | Instructions | CPI | Interval share |
| --- | ---: | ---: | ---: | ---: |
| protocol poll | 1,942,476,078 | 489,150,534 | 3.97 | 50.33% |
| runner outside protocol | 1,010,135,255 | 280,010,087 | 3.61 | 26.17% |
| radio poll outside runner | 709,720,605 | 70,589,658 | 10.05 | 18.39% |

The last row is the most instruction-sparse measured region, but it still
contains several possible causes: async scheduler/state-machine execution,
interrupt preemption, multi-cycle instructions and memory/cache stalls. CPI
alone cannot distinguish them. Espressif's official `ccomp_timer` component
also states that on RISC-V it falls back to the CPU cycle counter and cannot
compensate cache misses; IRAM placement is its proposed control. Thus there is
no supported CPU PMU shortcut which can convert these cycles directly into
cache-stall cycles on S31. The CACHE peripheral events provide hierarchy
traffic, not stall duration.

With counters enabled, the raw L1 events were:

| Bus | Hit | Miss | Access conflict | Next-level read | Next-level write |
| --- | ---: | ---: | ---: | ---: | ---: |
| IBUS0 | 596,944,814 | 1,636,948,621 | 0 | 11,252,488 | n/a |
| IBUS1 | 1,223,230,656 | 642,697,273 | 0 | 3,993,142 | n/a |
| DBUS0 | 301,522,583 | 81,261,870 | 3,216,740 | 469,710 | 362,883 |
| DBUS1 | 139,294,677 | 49,325,926 | 3,274,286 | 319,267 | 227,503 |

The corresponding `MISS/(HIT+MISS)` ratios are 73.28%, 34.44%, 21.23% and
26.15%, while next-level reads are only 0.62--0.69% of the instruction-bus
miss events and 0.58--0.65% of the data-bus miss events. The miss counters
therefore cannot be interpreted as one external refill or one CPU stall each.
Per admitted frame, the observed next-level reads are approximately 102.8,
36.5, 4.29 and 2.92 respectively. These numbers establish a usable current
baseline; without a same-ELF fast/slow pair they do not establish that cache
traffic is the throughput blocker.

The reorder phase split further corrects an earlier attribution error. Of
109,586 calls in the cache-off run, 109,438 took the immediate BA path, 148
had no reorder key and none took the slow buffered path. The measured
synchronous ingress cost was 312,065,160 cycles, but about 180.7 million of
those cycles were diagnostic observer callbacks (`ingress`, `release`,
`occupied`, and `prepared`) and another 43.0 million cycles were spent updating
the phase telemetry itself. After excluding those diagnostic-only regions, the
remaining measured key lookup, bank lookup, immediate ingest, deadline and
tail are about 1,200 cycles per frame. Calling the full approximately
2,850-cycle value intrinsic BA processing would therefore be wrong. Production
does not enable these observers.

Both same-ELF controls retained BA16, had zero buffered/slow reorder ingress,
zero software capacity blocks, zero hardware `BUFFER_FULL` and
`FIFO_OVERFLOW`, and pool/queue credit floors of 20/22 and 17/18. The current
ring64/retained32 architecture is consequently not showing a credit or reorder
failure in the normal ceiling state. This does not erase its bounded
backpressure contract; it only demonstrates adequate credits in this measured
workload.

Two failures observed while establishing the control remain separate. Run
`1787885914266-003261c8` failed during connected TX protection publication
before RX traffic, with `PhysicalPublicationUnverified` for CTS-to-self HT
nonmember protection, despite a fully armed 64-entry ring. Run
`1787886031457-00328523` failed because the optional independent laptop monitor
could not enable its intentionally soft-blocked WLAN. The RX scenarios now use
the OpenWrt monitor while the laptop WLAN remains blocked and the host route is
validated over `enp0s20f0u2u4c2`. Neither failure is evidence about RX cache or
ring behavior; the TX protection failure is an additional current-main
stability issue to reproduce independently.

The diagnostic fat-LTO build also exposed a pre-existing stack-audit mismatch:
`cold_start::start_esp32s31_wifi` materializes as 20,400 bytes after the recent
PAC-owned PHY transition refactors, above its former reviewed 18,432-byte
limit. A clean detached build of exact commit `603d94c4` produced the same
20,400-byte frame, proving that the new telemetry did not cause it. The
function-specific reviewed limit is raised to 21,504 bytes; the global 48 KiB
limit and 4 KiB move limit remain unchanged, and runtime stack painting remains
the dynamic bound.

### Architecture decision at this checkpoint

The current ownership topology remains the best measured design: Core0 owns
DMA/MAC/CCMP/BA, Core1 owns IP/UDP/sockets, the fixed ring has 64 entries with
retention bounded to 32, and only detached packet ownership crosses cores.
Copied immediate release and live descriptor replacement were already measured
as more expensive; moving the Core1 stack onto a Core0 owner with 94--95%
radio-poll residence is not justified. The synchronous ordinary admission and
affine SPSC changes are valid because they reduced nested cycle counters in
same-ELF controls, not because of a throughput guess.

The architecture is not finished. Normal ceiling operation leaves little
executor-return headroom, and the exclusive radio-outside-runner region has a
measured CPI near 10. The next CPU experiment must pair `mcycle/minstret` at
the existing DMA-service and protocol-frame boundaries, not add another broad
rewrite. That will distinguish instruction-heavy work from instruction-sparse
residence before selecting a small runtime-selectable IRAM control. A
same-ELF external/internal implementation A/B is required before cache
placement becomes a production decision.

The intermittent 93/51 Mbit/s state is a different localization problem. It
has lower, not higher, Core0 residence and fewer DMA admissions. The next
recurrence must preserve the exact ELF and collect AP station airtime/rate,
target PHY abort/restart/frontier counters and, where available, independent
air MPDU/BA counts in the same interval. Only that correlation can distinguish
AP scheduling/RF/PHY admission. Repeating cache on/off, changing the ring, or
calling zero buffer errors proof of a healthy radio path would not answer it.

An observer-free production build after this instrumentation has application
SHA256 `8825d4b5...`, runtime CRC32 `c42f03e8`, and still occupies 2,898,896 of
the 4,194,304-byte partition. Placement, stack-frame, autonomous-source-graph
and serialized-log-writer audits pass. This proves that the diagnostic modules
and cache-counter access do not leak into the shipping feature graph; it is a
build/audit result, not a new on-air throughput measurement.

## 2026-08-28: instruction split inside the Core0 owner

The next profile paired `mcycle/minstret` at the existing synchronous DMA
transaction and ordinary protocol-frame boundaries. It also split instructions
before runner entry and after runner completion within one top-level radio
poll. No ownership, queue, ring, BA or scheduling policy changed.

Final-source run `1787887802948-0032fe68`, runtime CRC32 `4b18a1e1`, passed at
`107.152 Mbit/s`. It admitted 109,927 frames, delivered 109,762 UDP datagrams,
received only HT40/MCS7 benchmark vectors, retained BA16 with zero buffered,
missing or stale reorder events, and reported zero software capacity blocks,
`BUFFER_FULL`, `FIFO_OVERFLOW`, AP retries and AP failures. Pool/queue credit
floors were 22/24. The mean DMA batch was 4.76 MPDUs per service.

At 320 MHz, 3,671,037,068 radio-poll cycles over 12.062768 seconds are 95.10%
Core0 residence. The exclusive decomposition is:

| Region | Cycles | Retired instructions | CPI | Interval share |
| --- | ---: | ---: | ---: | ---: |
| DMA transaction | 952,146,420 | 264,379,394 | 3.60 | 24.67% |
| protocol frame body | 1,106,450,312 | 319,348,815 | 3.46 | 28.66% |
| protocol poll outside frame bodies | 702,954,439 | 171,930,370 | 4.09 | 18.21% |
| runner outside DMA and protocol polls | 147,168,300 | 18,789,825 | 7.83 | 3.81% |
| radio poll outside runner | 762,317,597 | 73,380,818 | 10.39 | 19.75% |

The percentages sum to radio residence rather than 100% of the interval. The
DMA transaction averaged 8,662 cycles and 2,405 instructions per admitted
frame. The protocol body averaged 10,065 cycles and 2,905 instructions per
frame. They are substantial real work, but neither has the worst CPI.

The outer radio region is now almost completely closed by the pre/post split:

| Scheduler boundary | Cycles | Instructions | CPI | Per runner call |
| --- | ---: | ---: | ---: | ---: |
| radio poll to runner entry | 486,609,670 | 54,133,572 | 8.99 | 21,091 cycles |
| runner completion to poll exit | 267,941,874 | 18,173,317 | 14.74 | 11,613 cycles |
| remaining polls/sampling difference | 7,766,053 | 1,073,929 | 7.23 | n/a |

Existing cycle-only subphases independently account for the first row:
scheduler reentry was 183.5 million cycles, stop probing 30.2 million,
housekeeping 140.2 million, TX checks 78.0 million and final RX checks
41.5 million. The second row is primarily counter publication followed by the
mandatory `yield_now` poll and async unwind to the executor. The earlier
no-yield A/B remains decisive: deleting the yield caused premature empty
re-entry, reduced batching and made total radio residence worse. Therefore the
new numbers do not justify removing fairness or spinning on DMA.

The approximately 703-million-cycle protocol wrapper is not all production
protocol work. This image's reorder observer callbacks consumed about 188.8
million phase cycles and reorder counter updates another 37.8 million; general
dequeue/entry/frame/data/publication telemetry adds more measured work. The
ordinary path nevertheless took 109,784 of 109,927 keyed reorder calls and no
call used slow buffering. BA buffering, gap recovery and ring backpressure are
not responsible for the large wrapper total in this interval.

This changes the optimization order. Dynamic descriptor replacement,
unconditional copies, moving the IP stack to Core0 and deleting the local SPSC
remain rejected by measured costs. A broad SRAM move is also premature. The
largest instruction-sparse, non-payload cost is the scheduler/service boundary,
crossed 23,073 times for 109,927 MPDUs. The next architecture A/B should reduce
that boundary frequency or its generic state-machine work while preserving the
useful coalescing produced by the yield.

A valid candidate is a bounded RX continuation state, not a busy loop: after
one finite DMA/protocol turn and one cooperative yield, an unchanged
stop/control/TX generation may re-enter the RX owner through a compact fast
path when a durable RX signal is already present; any changed generation or
empty frontier returns to the complete scheduler. Its same-ELF acceptance must
show lower poll-to-runner and runner-to-exit cycles *and* instructions, no drop
in mean DMA batch, unchanged RX-before-TX/stop/control ordering, zero credit or
hardware exhaustion, and no throughput regression. If a runtime-selectable
small IRAM copy of that fast path lowers CPI after the semantic win is proven,
cache placement becomes causal evidence rather than layout speculation.

## 2026-08-28: bounded scheduler continuation rejected

The candidate above was implemented only as a runtime selector in the existing
`diagnostic-core0-rx-cycles` ELF and measured in suite
`1787888709835-0033b872` (runtime CRC32 `08d1d344`). Both scenarios used the
same image, split radio/network placement, software checksums, synchronous RX
admission, enabled L1 counters and the same HT40/MCS7 workload. The selector
was the only behavioral input difference.

The continuation variant passed at 105.359 Mbit/s and the complete-scheduler
control passed at 104.849 Mbit/s. Both are at the known air ceiling, so that
0.510 Mbit/s difference is not treated as proof of a performance gain. Both
retained BA16, had no buffered/missing/stale reorder traffic, no pool or queue
capacity blocks, no hardware `BUFFER_FULL`/`FIFO_OVERFLOW`, and no AP retries
or failures.

The reduced branch was exercised 14,473 times. It did remove generic
scheduler work: poll-to-runner cost fell from 21,916 cycles / 2,358 retired
instructions to 20,294 cycles / 2,188 instructions per runner call, and TX
checks fell from 2,152 to 475 cycles per service. However, it re-entered RX
before the next physical frontier had coalesced as widely. Mean DMA batch fell
from 6.072 to 5.904 MPDUs per service, and service calls increased from 17,711
to 18,329 for comparable traffic.

After normalization, total radio work was effectively identical: 33,704.2
cycles per admitted frame in the control and 33,702.2 in the continuation.
Runner work slightly increased from 27,963.8 to 28,133.3 cycles per frame;
DMA work increased from 7,289.5 to 7,345.2 cycles per frame. Measured Core0
radio-poll residence was 93.93% and 94.38% respectively. Thus the local branch
saving was exchanged for smaller batches and more runner entries; it did not
reduce total Core0 work.

The experiment fails its predeclared acceptance condition (no decrease in mean
DMA batch) and is not a production architecture change. The mandatory yield
is serving a useful coalescing function, not merely scheduler fairness. The
next analysis must separate production Core0 residence from the substantial
intrusive observer cost, then optimize per-frame or fixed-per-batch work using
that lower-overhead baseline. Making RX re-entry earlier is now a measured
non-solution.

## 2026-08-28: observer-free Core0 load and air-delivery closure

The production-like residence sweep was repeated on the exact current source
with only the top-level task timer enabled. Runs at 30, 60, 90 and saturated
140 Mbit/s offered load delivered 30.004, 60.003, 90.004 and 106.806 Mbit/s.
The corresponding radio-task residence was 50.070%, 75.423%, 91.675% and
93.205%. Thus Core0 is not approximately 50% occupied at the RX ceiling; it is
above 90%. The non-linearity is real: normalized radio work falls from about
62.9 thousand cycles per frame at 30 Mbit/s to 32.9 thousand at saturation as
poll and batch costs amortize. A single fixed cycles/frame extrapolation from
light load is invalid.

Independent air capture also closes the apparent receive-loss question. In
run `1787890236502-0033fa69` (runtime CRC32 `03ab814c`, 106.217 Mbit/s), the
laptop monitor decoded 9,080 full and five partial BlockAck responses covering
144,875 unique acknowledged MPDUs. The target recorded approximately 144,873
post-reorder frames and 144,856 UDP datagrams. Later coarse runs repeat the
same relationship. The target therefore loses essentially none of the MPDUs
which the station successfully acknowledges. The host-to-target datagram gap
is upstream of the radio: OpenWrt AQM/queueing and packets not transmitted by
the end of the measurement interval. Zero target DMA error counters are
consistent with this evidence, but are not the basis of the conclusion.

The air observer reports almost exclusively full BA16 responses. BA16 is the
current negotiated production window. Raising it mechanically to BA32 is not
a valid next experiment: ring64 with retained capacity 32 deliberately keeps
at least 32 physical descriptor credits, while BA32 would allow one aggregate
to consume that complete reserve. Earlier BA32 evidence exposed executor
latency as `BUFFER_FULL`. The current BA16 setting is therefore an ownership
invariant, not an accidental rate-control limitation.

## 2026-08-28: coarse production-path cycle profile

A new `diagnostic-core0-rx-coarse` image was added to remove driver observers,
per-frame phase counters and L1 event collection from the profile. It samples
only the radio future poll, one runner call, one DMA batch and one protocol
batch, pairing `mcycle` with `minstret`. The shipping feature graph remains
unchanged.

Run `1787891588059-00344fe3`, runtime CRC32 `9f949382`, delivered 106.340
Mbit/s. The independent observer saw 6,808 full and four partial BlockAcks
covering 108,982 unique MPDUs; the target delivered 108,961 UDP datagrams. The
radio task occupied 11,123,451 of 12,066,184 microseconds (92.185%). Coarse
counter totals were:

| Region | Cycles | Retired instructions | Per DMA MPDU |
| --- | ---: | ---: | ---: |
| complete radio poll | 3,530,356,010 | 721,577,140 | 32,350 cycles |
| DMA batches | 1,069,718,654 | 259,418,123 | 9,802 cycles |
| protocol batches | 1,027,875,024 | 281,026,393 | 9,419 cycles |
| poll entry to runner | 807,464,001 | 105,900,150 | 7,399 cycles |
| runner completion to poll exit | 390,905,346 | 37,755,912 | 3,582 cycles |

There were 52,763 DMA calls for 109,127 completed units: 2.068 MPDUs per DMA
call. The AP simultaneously supplied near-full 16-MPDU aggregates. This
establishes a scheduling granularity mismatch, but does not by itself prove
that fewer calls reduce total work. At the measured 32.35 thousand cycles per
MPDU, 115 Mbit/s would consume approximately 99% of one 320 MHz Core0 if
batching and per-frame costs stayed unchanged. This is an extrapolation, not a
measured 115 Mbit/s operating point.

## 2026-08-28: same-turn frontier chaining rejected

The granularity hypothesis was tested rather than promoted to an architecture
change. A runtime-only selector in one coarse diagnostic ELF re-probed DMA
after each productive protocol frontier while retaining the existing total
32-frame limit and the same outer cooperative yield. Suite
`1787892105095-00345f32`, runtime CRC32 `a1d01186`, compared this with the
single-frontier control. Both paths delivered virtually every independently
BlockAcked MPDU: 108,908 of 108,930 in the chained run and 110,662 of 110,688
in the control. Throughput was 106.380 and 108.048 Mbit/s respectively; the AP
put different amounts of traffic on air, so that difference is not attributed
to the target implementation.

Normalized Core0 evidence is decisive:

| Metric | Single frontier | Chained | Change |
| --- | ---: | ---: | ---: |
| DMA calls / MPDUs | 53,446 / 110,843 | 129,218 / 109,084 | +145.7% calls per MPDU |
| MPDUs per DMA call | 2.074 | 0.844 | -59.3% |
| complete radio cycles/MPDU | 32,535.96 | 32,527.85 | -0.025% |
| DMA cycles/MPDU | 8,954.80 | 10,882.32 | +21.52% |
| protocol cycles/MPDU | 9,759.63 | 10,351.57 | +6.07% |
| exclusive runner cycles/MPDU | 1,970.13 | 2,161.50 | +9.71% |
| outer entry+exit cycles/MPDU | 11,703.05 | 8,512.74 | -27.26% |

Chaining did reduce the outer scheduler boundary, but the terminal probes were
usually empty and transferred more cost into DMA and protocol execution. The
total changed by only -0.025%, far below a meaningful architectural gain. The
experimental selector and scenario were removed. Along with the earlier
post-yield continuation result, this proves that both earlier and immediate
re-entry exchange scheduler cost for worse DMA batching; neither is a current
solution.

The current architecture remains justified: Core0 owns DMA/MAC/CCMP/BA,
Core1 owns IP/UDP/sockets, ring64 and retained32 bound cross-core backpressure,
and the affine SPSC removes a measured queue abstraction cost. The next useful
question is why a full BA16 air aggregate becomes only about two completed
units at each frozen DMA frontier. That requires identifying the exact MAC/DMA
interrupt or completion edge which wakes the owner. Software spin/re-probe,
larger BA, SRAM placement and another ownership rewrite are not justified by
the present evidence.

## 2026-08-28: missing STA RX source moderation

The next audit found a concrete lifecycle asymmetry. The interrupt runtime is
constructed with an RX-delivery mask/unmask capability and the bottom half
calls `unmask_rx_after_drain()`, but only the AP epoch enabled source
moderation. The connected STA epoch left it disabled. Existing intrusive runs
had already shown approximately one `RX_SUCCESS` publication per received
MPDU; the zero hardware error counters did not measure this interrupt load.

The behavior was tested twice with a runtime-only selector in one coarse ELF,
runtime CRC32 `51bacda3`. The second suite, `1787893012066-0034b6f7`, had
nearly identical air supply: 110,607 unique BlockAcked MPDUs in the control and
110,700 with moderation, with 110,575 and 110,677 target UDP datagrams. The
first suite was `1787892772189-00349f6f`; it produced the same normalized CPU
result.

| Metric | Unmoderated | Moderated | Change |
| --- | ---: | ---: | ---: |
| real RX IRQ posts/MPDU | 1.0000 | 0.0168 | -98.32% |
| retired instructions/MPDU | 6,605.89 | 6,282.30 | -4.90% |
| radio cycles/MPDU | 32,453.89 | 32,528.46 | +0.23% |
| MPDUs/DMA call | 2.073 | 1.987 | -4.15% |
| delivered throughput | 107.913 Mbit/s | 108.070 Mbit/s | air-limited tie |

The first suite measured -98.35% IRQ posts, -4.78% instructions and +0.48%
cycles per MPDU. Thus the direction is reproducible: source moderation removes
almost all hard-ISR publications and about 320 retired instructions per MPDU,
but does not lower the current radio cycles/MPDU or raise throughput. It fixes
a real STA lifecycle/CPU-instruction defect; it is not the RX ceiling fix.

The moderated owner still made 55,791 DMA calls for 110,844 MPDUs while only
1,863 hardware RX publications occurred. Therefore the approximately
two-MPDU service frontier is not driven by an interrupt storm. Almost all
subsequent runner calls are the durable software `ProbePending` handoff after
the initial source-masked interrupt. This is the new localization: the ring
owner polls hardware completion across cooperative yields while a PPDU is
still arriving. Immediate same-turn polling was already measured as too early;
source masking alone does not change the physical completion cadence.

STA source moderation is promoted to the normal connected-epoch lifecycle,
matching the existing AP rule: enable before route activation, unwind it on an
activation failure, and disable it only after successful route quiescence. The
same-image selector is removed. The remaining ceiling work must address the
cost of DMA/protocol processing or use a qualified later hardware/coalescing
edge; it must not claim that removing interrupts has already freed Core0
cycles.

## 2026-08-28: reconnect exposed retained-producer SPSC lifecycle defect

The first reconnect qualification after the SPSC conversion failed before its
second connected epoch. Run `1787893614483-0034d064`, runtime CRC32
`8a5b64f4`, panicked in `AffineSpscQueue::split()` with one endpoint still
active. UART evidence showed that the first connected epoch had stopped and
quiesced normally. The failure was therefore not an interrupt-moderation
failure and not a radio-performance result.

The owner graph audit identified the exact mismatch. Connected RX deliberately
retains its physical DMA producer inside `Esp32s31StoppedRx` across station
reassociation. The old protocol consumer is drained and dropped. The
supervisor nevertheless called `STAGED_RX_QUEUE.split()` again, which requests
a new producer and consumer while the retained producer is still the unique
active publisher. The earlier endpoint count could reject this but could not
express the valid state "producer active, consumer absent."

The queue now tracks producer and consumer capabilities with distinct bits.
`split()` still succeeds only from the fully unowned state. A retained sender
may reacquire exactly one consumer only when the queue is empty and the active
state is exactly producer-only. The typed receiver capability is exposed
through the standalone RX publisher; reconnect never resets cursors and never
manufactures a second producer. The paired STA+AP cutover explicitly drops the
standalone consumer before replacing the drained standalone publisher with the
paired publisher.

Host tests cover duplicate split rejection, cursor/FIFO behavior, one consumer
resume beside a persistent producer, duplicate-resume rejection, and complete
queue reuse after both endpoints end. The adapter suite passed 242 tests.

The first complete hardware confirmation was run `1787894212719-0034e500`,
runtime CRC32 `cba9331b`: all 10 cold boots and all 30 reconnect cycles passed.
After rebasing over the next seven PAC/TX mainline commits, the final tree was
qualified again rather than reusing evidence from the earlier ELF. Run
`1787895070553-003564f1`, runtime CRC32 `1d5c313b`, again passed all 10 cold
boots and all 30 reconnect cycles. CPU0 and CPU1 free stack remained 25,216 and
12,012 bytes respectively at both the initial-connected and every
reconnect-complete boundary.

The final rebased observer-free performance gate is run
`1787895345267-00359a58`, runtime CRC32 `0b041fe7`. It passed at 108.499 Mbit/s
with placement, stack, source-graph and serialized-log audits all passing. The
pre-rebase control `1787894496776-0034f3fd` had passed at 108.291 Mbit/s. This
establishes that neither the lifecycle correction nor its mainline rebase
introduced a production-layout throughput regression. The small difference is
not claimed as a throughput improvement: both values remain inside the already
observed air-supply variation around the HT40 LGI ceiling.

## 2026-08-28: Core1 packet-rate loss and bounded Embassy polling

The full-size saturated ceiling concealed a separate packet-rate defect. A new
40 Mbit/s control reduced UDP payload from 1,472 to 512 bytes, increasing the
packet rate to approximately 9.77 kpps without approaching the HT40 byte-rate
ceiling. On the upstream Embassy `Runner::run()` path, both the coarse image
and the observer-free production image lost sequence ranges after successful
AP transmission:

| Network runner | Image/run | Host datagrams | Target datagrams | Forward missing | RX |
| --- | --- | ---: | ---: | ---: | ---: |
| upstream unbounded | production `1787896075456-0035b54e` | 117,111 | 114,280 | 2,830 | 39.008 Mbit/s |
| upstream unbounded | coarse `1787895841554-0035a6da` | 117,189 | 115,621 | 1,564 | 39.468 Mbit/s |
| cooperative, 64 ingress | production `1787896821370-0035cf9c` | 117,183 | 113,868 | 3,313 | 38.869 Mbit/s |
| cooperative, 32 ingress | production `1787897053343-0035d68b` | 117,165 | 117,157 | 0 | 39.995 Mbit/s |

The host tail in the accepted run was eight datagrams beyond the highest
target sequence; there was no gap inside the target interval. The 64-ingress
variant is not accepted: its maximum observed sequence gap was exactly 64,
the complete HIL UDP socket metadata depth. A network quantum equal to the
whole application reserve still permits the runner to fill that reserve
before the consumer receives an executor turn. The accepted production
policy processes at most 32 ingress packets per poll against the HIL's
64-packet socket reserve and self-wakes when work remains.

The delivery profile localized the defect rather than inferring it from
throughput. Before the fix, run `1787896169402-0035bb73` observed 117,185 host
data units, 117,185 post-BA/reorder units and 117,185 successful driver-to-stack
enqueues, but only 117,127 UDP consumer observations. The delivery ledger
reported 58 skipped sequences and zero driver network queue-full events.
DMA admitted 117,393 units with no malformed/overload discard, BA16 reorder
released all 117,201 benchmark MPDUs with no missing/stale/expiry event, and
hardware `BUFFER_FULL`/`FIFO_OVERFLOW` remained zero. The loss boundary was
therefore inside the Core1 IP/socket scheduling interval, after the Wi-Fi
driver enqueue.

Task timing independently matches that boundary. In the unbounded coarse run,
one Core1 network poll lasted up to 37.653 ms and eleven polls exceeded 5 ms.
After bounding ingress to 32, run `1787898031640-00362069` had a 2.247 ms
maximum network poll and no poll exceeded 5 ms. It received 117,188 datagrams
with zero sequence gap at 40.002 Mbit/s. Total Core1 network residence rose
from 7.273 to 8.648 seconds because the fixed path actually processed the
packets previously discarded; this is not evidence that the fix made network
processing cheaper.

The final delivery run `1787897708884-0036191e`, runtime CRC32 `80ca0baa`,
closed every measured frontier exactly:

```text
host data        117189
post reorder     117189
stack enqueue    117189
UDP consumer     117189
ledger skipped        0
queue full            0
BA missing/stale      0 / 0
HW full/overflow      0 / 0
```

This is a Core1 executor-fairness correction, not a radio-ring or Core0
optimization. The split-core ownership remains correct: Core0 is still the
saturated DMA/MAC/CCMP/BA owner, while Core1 owns IP and sockets. What was
incorrect was allowing the Core1 stack future to drain a continuously
replenished cross-core device queue in one executor poll and thereby starve
its sibling socket task.

Upstream Embassy main `645068a3` still exports only the unbounded runner; the
single-packet xarxa primitives needed to implement this policy are private.
The repository therefore pins minimal fork commit `f7f09eb6`, one commit
rebased over that upstream main. It adds the 4/4 directionally fair polling
loop, 32-ingress and 32-egress bounds, self-wake semantics, TX-credit
distinction and focused tests. Published `embassy-time`, `embassy-futures` and
`embassy-sync` crates remain in use. This is the previously missing functional
reason for the fork.

The fairness change did not trade away the existing ceilings:

- observer-free saturated RX `1787897214560-0035ee1c`: 108.928 Mbit/s;
- observer-free TX `1787897343048-003612db`: 106.671, 106.941 and
  107.582 Mbit/s host floors;
- observer-free 40/40 full duplex `1787897476706-00361585`: five passes at
  79.996--80.035 Mbit/s combined.

This result supersedes the broad earlier conclusion that restoring a fork
cannot be the answer. The old 64-ingress fork remains rejected by measurement;
the accepted 32-ingress policy solves a different, explicitly localized
packet-rate failure while preserving full-size RX, TX and bidirectional
throughput. It does not remove the independently proven greater-than-90%
Core0 residence at the saturated RX ceiling. Further Core0 work must continue
from the DMA/protocol cycle decomposition and must not relabel this Core1 fix
as a solution to the remaining Core0 headroom problem.

Pre-rebase validation used the cleaned lockfiles rather than reusing an
intermediate ELF. Observer-free performance run
`1787898803779-003668b6`, runtime CRC32 `faf16f19`, passed at 109.310 Mbit/s.
The immediately preceding attempt `1787898644374-0036655f` used the same
runtime CRC but never published connected-station readiness and therefore
contained no traffic measurement. The unchanged second attempt passed; the
first attempt remains recorded as an association/lifecycle event rather than
being relabeled as throughput noise.

Correctness run `1787898870718-00366a08`, runtime CRC32 `d8e305a7`, then
passed all 10 cold boots and all 30 station reconnect cycles. CPU0 and CPU1
free stack remained 25,216 and 12,012 bytes at every connected boundary.
Together these final-image controls establish both the restored packet-rate
fairness and preservation of the existing full-size RX and lifecycle behavior.

The scheduler commit was then rebased over five newer mainline commits. One of
them, `de24dca5`, changes the production RX-DMA register API to typed field
access; the other four refactor Bluetooth PAC access. Because the first change
touches the actual Wi-Fi DMA path, the earlier ELF was not accepted as final
evidence. Post-rebase observer-free run `1787899595905-0036cc2c`, runtime
CRC32 `3501bff3`, passed at 109.353 Mbit/s. Post-rebase correctness run
`1787899749385-0036d8e3`, runtime CRC32 `e35d0a75`, again passed all 10 cold
boots and all 30 reconnect cycles with the same 25,216/12,012-byte CPU0/CPU1
free-stack floors. The mainline RX-DMA field refactor therefore introduced no
measured throughput or station-lifecycle regression in the integrated tree.

A final mainline commit, `ed5cf11f`, moved MAC interrupt-enable access to
typed PAC fields. The fully integrated observer-free ELF therefore changed
again and was measured rather than assumed equivalent. Run
`1787900107117-0037256a`, runtime CRC32 `ab220437`, passed at 109.208 Mbit/s
after a successful station connection. This checks the shipping MAC interrupt
path and preserves the RX ceiling; the exhaustive 10-boot/30-reconnect result
immediately before it remains the lifecycle stress control.
