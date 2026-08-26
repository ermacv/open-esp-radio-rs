# ESP32-S31 STA RX degradation investigation checkpoint

Date: 2026-08-26 through 2026-08-27. Repository head: `e5fbc5e0` (`main`, equal to
`origin/main`).

This is an investigation checkpoint, not a qualification claim. Most runs
below used dirty sources or diagnostic images. The purpose of this record is
to preserve what was already tested, separate facts from hypotheses, and stop
repeating throughput-only A/B runs that no longer distinguish causes.

## Executive status

The historical STA RX expectation is valid: the 2026-08-13 qualification
record measured `108.752..116.172 Mbit/s`. The immediate `7..10 Mbit/s` loss
investigated here now has a causal fixture-level explanation. A byte-identical
archived application that had produced `102.378..104.699 Mbit/s` measured only
`94.559..96.798 Mbit/s` on OpenWrt channel 6, then measured
`101.772..103.119 Mbit/s` after changing only the AP primary channel to 11.
The application, runtime ELF, host route, HT40 width, MCS7 vector, offer, and
checksum policy were unchanged.

Channel 6 had `31.5%` idle CCA busy time versus `23.6%` on channel 11. An
independent capture identified a `FRITZ!Box 7530 FN` on channel 6 at
approximately `-28..-37 dBm`, close to the laboratory AP's signal. The
`7.9` percentage-point idle-busy difference matches both the direction and
magnitude of the recovered throughput. The current degradation is therefore
an uncontrolled radio-cell precondition, not a DUT code regression.

The investigation also found two real but separate engineering concerns:
exact ELF placement can affect the network-core cache ceiling, and the dirty
ownership diagnostic exposes finite RX-ring capacity under overload. Neither
explains the identical-binary channel A/B. No evidence supports a broken
A-MPDU/BlockAck agreement, a laptop WLAN route, the upstream Embassy switch,
checksum policy, or the OpenWrt Ethernet path as the cause of the current
`94..97 Mbit/s` ceiling.

## Current working state

The source tree is not a clean current-main performance baseline. It contains
28 modified files and one untracked scenario, about 1,000 insertions and 105
deletions before this record is counted. The changes belong to several
different experiments and must be split
before publication:

- RX ring ownership: bulk frames are preserved at a completed ring head when
  staging is full instead of being discarded and recycled. Focused adapter
  tests passed (`244` tests in the affected crate during this investigation).
- Driver/HIL observation: expanded RX PHY/decode/hang counters, fuller ring
  frontier accounting, task-poll timing, and AP/independent-air evidence.
- Runtime startup: one `wait_config_up` waiter starts all long-lived network
  services, avoiding replacement churn in Embassy's single `state_waker`.
- Temporary experiment scaffolding: same-boot RX probes controlled by
  `OPEN_RADIO_RX_SAME_BOOT_PROBES`, one-repetition scenarios, and relaxed
  acceptance criteria.

The local performance scenario is deliberately weakened for diagnosis:

- repetitions changed from `3` to `1`;
- the RX floor changed from `100` to `90 Mbit/s`;
- the RX-delivery diagnostic changed from exact `80 Mbit/s` delivery to a
  lossy `120 Mbit/s` overload probe and no longer requires beacon integrity.

Those scenario changes are not acceptable as a final qualification gate.

The Cargo manifests and locks are restored to upstream Embassy and have no
local diff. They select Embassy commit `e7a576f2` and crates.io support crates.
The source-only audit before the final rebase rebuilt the dirty root performance
application with SHA-256 `697a3dcc...` from those restored manifests, but did
not flash or run it. The DUT was most recently flashed from the separate clean
`e5fbc5e0` worktree with current-main application `961dc049...`. That clean
control is valid, but neither it nor the successful dirty build makes the dirty
root worktree a publishable performance baseline.

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

## Conclusions by confidence

### Proven

- Historical and archived current-cell RX can exceed `100 Mbit/s`.
- The laptop sends over the Ethernet route with the correct source address.
- Native Embassy/xarxa `Runner::run()` can be much faster than the old custom
  cooperative wrapper; restoring the fork is not the answer.
- Hardware cache counter enable itself is not the speed increase.
- Performance does not accidentally enable diagnostic features.
- BA16 negotiation and reorder behavior are intact in the observed run.
- The exact same application/ELF is `6.2..8.6 Mbit/s` faster when only the
  OpenWrt HT40 primary channel changes from 6 to 11.
- Channel 6 currently has about `7.9` percentage points more idle CCA busy time
  and a same-channel FritzBox at near-laboratory signal strength.
- The current `94..97 Mbit/s` result is an invalid radio-cell ceiling sample,
  not evidence of a DUT RX regression.
- The old ring policy silently dropped completed bulk RX frames under staging
  pressure; the preservation policy exposes a real 64-descriptor hardware
  capacity limit instead.
- Neither upstream-vs-fork, the one-waiter gate, nor unused network features
  alone explains the complete loss.

### Separate, strong but not yet causal for code robustness

- Network-core instruction layout/cache behavior determines a substantial
  part of the processing ceiling.
- Dirty diagnostics can saturate staging and the hardware ring when Core
  1/network consumption cannot keep pace with BA16 bursts.
- Diagnostic instrumentation can worsen AP/radio behavior and must not be used
  as a direct throughput baseline.

### Still open

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
- Another old-fork versus upstream dependency run.
- Another cache trace bit on/off run.
- Another broad feature-disable build without an identical ELF/source control.
- Another multiple-waiter versus one-waiter run as an explanation for 10
  Mbit/s.
- Another magic padding/alignment sweep presented as a final fix.
- Another diagnostic-throughput comparison interpreted as production speed.

## Next decisive experiment

Do not run another SRAM, cache-padding, checksum, ring-size, or Embassy A/B on
channel 6 under the present occupancy. The clean current-main baseline is now
back above `100 Mbit/s` under the RF gate. If the remaining approximately
`1.6 Mbit/s` mean difference is worth pursuing, the next code comparison is:

1. exact archived native-`run()` application `11a3e465...`;
2. clean current-main application built from the canonical source root;
3. alternating runs with laptop WLAN disabled, the same Ethernet route,
   MCS7/HT40 validation, and pre-workload utilization at or below `64/255`;
4. three repetitions per image, preserving application/ELF hashes and AP
   utilization in every bundle.

Alternate the two already archived applications rather than rebuilding them,
and record the 12-second busy/active delta for every repetition. Only a stable
residual difference under that controlled RF boundary justifies returning to
JTAG cache counters and a selective IRAM/DRAM control. SRAM remains a
robustness tool for a proven hot working set, not a response to an invalid
radio-cell benchmark.
