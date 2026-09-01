# HIL architecture audit and refactor plan

Status: current-state audit and implementation record, 2026-09-01. This document describes the HIL
implementation after exact firmware replay, build/lab provenance, route
validation and controlled-OpenWrt epoch isolation were implemented. It is a
refactor plan, not a qualification claim.

## Decision

The HIL does not need a replacement architecture. Its important ownership
boundaries are sound:

- production radio behaviour remains under `driver/`;
- HIL firmware applies public production constructors instead of carrying a
  shadow radio implementation;
- the host owns scenarios, criteria, fixture mutation and traffic generation;
- target readiness and completion use a typed, versioned protocol;
- every invocation creates an immutable, integrity-indexed evidence bundle;
- exact archived firmware replay is separate from source reconstruction and
  from reproducible rebuilds;
- clean-current qualification is stricter than diagnostic replay;
- the fixture lock, recorded host route and controlled radio epoch make the
  physical path part of the experiment contract.

The problem is implementation scale. Several files now combine independent
state machines, parsers, validators and renderers. That makes changes hard to
review and encourages an optional diagnostic to leak into a required test
contract. The correct response is an evolutionary, behaviour-neutral split
along existing ownership boundaries, followed by targeted evidence hardening.

## Measured inventory

The audit used the repository catalog at commit `e14f0ad7` and obtained:

- 55,970 lines of Rust under `hil/`;
- 33,888 lines in the host runner;
- 12,369 lines in the ESP32-S31 runtime;
- 4,564 lines in target-only telemetry;
- 3,948 lines in the wire protocol;
- 202 versioned scenario files, all reset-isolated. They were schema 3 at the
  audited commit and are now schema 4 after the direction-neutral independent
  air-monitor cutover;
- 124 scenarios tagged `diagnostic` and 63 whose ID starts with
  `diagnostic-`;
- 202 host-runner unit tests, 39 protocol tests and five target-runtime host
  tests;
- 12 image classes selected by the current catalog.

`cargo hil scenario validate` accepts all 202 scenarios. The quantity is not
itself a defect: many files intentionally retain one historical A/B recipe.
It does mean the catalog is now a material public surface and cannot be
treated as an incidental list of TOML files.

The largest implementation units are:

| File | Lines | Responsibilities currently combined |
| --- | ---: | --- |
| `host/runner/src/traffic/bidirectional.rs` | 4,462 | CLI options, host traffic, typed and text evidence models, log parsing, qualification, Markdown rendering, tests |
| `targets/esp32s31/runtime/src/product_hil.rs` | 3,318 | Wi-Fi observations, role configuration, task composition, station/AP evidence, IEEE 802.15.4 probes |
| `host/runner/src/evidence/traffic_capture.rs` | 2,573 | serial ownership, decoder health, command protocol, session recovery, readiness probes, target reset, evidence validation |
| `host/runner/src/qualification/scenario.rs` | 2,571 | scenario model, semantic validation, catalog loading and catalog-wide tests |
| `host/runner/src/qualification/access_point.rs` | 2,558 | AP lifecycle, client fixtures, UDP/TCP/ICMP, multi-client fairness, validation and JSON reports |
| `protocol/src/message.rs` | 2,358 | every protocol domain plus central command/event enums and compatibility tests |
| `targets/esp32s31/runtime/src/console.rs` | 2,316 | emergency logging, async logging, protocol transport, startup artifacts, command admission and session state |
| `host/runner/src/reporting/run.rs` | 2,091 | run schemas, bundle lifecycle, source/firmware import, integrity inventory, JUnit/HTML rendering and tests |

The line counts are only a signal. A split is justified where it creates a
single owner or schema boundary, not merely to make files shorter.

## What is already reliable

### Execution and evidence

The runner flashes the archived `application.bin`, retains the corresponding
ELF/runtime/bootstrap subjects, records source materials, seals every regular
file in `integrity.json`, and can replay a prior image without invoking Cargo.
It also distinguishes exact replay, source reconstruction, rebuild
reproducibility and physical experiment reproduction. These distinctions must
survive the refactor.

`SerialCapture` fails closed on protocol corruption and retains typed
readiness, terminal evidence, stack watermarks and decoder health. Text is
diagnostic. Current throughput acceptance uses typed transport/radio evidence;
failed parsing of detailed text telemetry is reported as a diagnostic warning
instead of fabricating zero evidence.

### Physical fixture

Station traffic validates the selected host route and socket source after the
target address exists. AP traffic through the controlled OpenWrt client starts
from a fresh wireless epoch, removes scoped VIF/firewall state and validates
channel and width before the DUT is reset.

This reset was required by a reproduced failure: the exact same historical
application fell from roughly 115 Mbit/s to 18--49 Mbit/s after repeated
managed-interface lifecycles and returned to roughly 116 Mbit/s after an
OpenWrt wireless restart. BA32 completion, retry, DMA and radio error counters
remained clean. The reset is therefore an experiment-isolation requirement,
not a throughput workaround.

One unproven failure family remains recorded for future diagnosis. Stale
mac80211/mt76 RX-BA, TXQ or airtime accounting could produce a similar
lifecycle-dependent state. OpenWrt AQL itself is not a current causal result:
the failing workload was ESP-to-OpenWrt, while AQL gates frames transmitted by
OpenWrt, and the mt7915 RX BA session is separately installed in firmware.
Do not change AQL policy without a same-state measurement of pending airtime,
TXQ state and over-the-air PPDU-to-BlockAck timing.

### Optional evidence

Optional observers remain optional. In particular, a performance image must
not be required to emit a separately generated internal driver report. The AP
report uses `Option` for driver evidence and records absence as absence rather
than a synthetic zero-valued observation. This rule must be applied to new
reports.

## Findings

### 1. Host domain modules are too broad

The host runner has the strongest test coverage, so it should be split first.
The intended boundaries are already visible in its types:

```text
scenario model -> semantic validation -> catalog
fixture owner -> traffic session -> typed assessment -> report projection
run model -> bundle transaction -> integrity -> presentation
```

Today these stages frequently share one file and private implementation state.
That raises the review cost of changing a criterion or report without changing
traffic semantics.

### 2. Diagnostic text is still too structurally important

Readiness and terminal results are typed, but detailed task-poll, phase and
histogram evidence is emitted as bounded text and parsed by the host. A busy
diagnostic image can report dropped text records even when enough lines happen
to survive for an analysis. This is acceptable for exploratory output, but it
is not a durable representation for comparisons that the project wants to
repeat months later.

Frequently consumed interval summaries should become one bounded typed event
or terminal evidence block per interval. High-frequency samples must not be
streamed through the reliable control queue and must never backpressure the
radio. Raw text can remain as a human diagnostic attachment.

### 3. Downlink air timing is only partially observable

The independent laptop observer now records a direction-neutral target-egress
evidence block. It always summarizes consecutive peer BlockAck timestamps and
only publishes paired intervals when it decoded enough target data records to
associate every BlockAck with a target transmission. This fail-closed
availability bit prevents a sparse decoder result from becoming false timing
evidence.

Exact replay `1788269193985-00130434`, using the archived current correctness
image from `1788268379279-0012dc66`, passed and produced:

| Cycle | Target data records | Peer BlockAck | BA-to-BA p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 0 | 1 | 1,228 | 4,475 us | 26,618 us | 28,010 us |
| 1 | 0 | 1,247 | 4,463 us | 23,723 us | 28,328 us |

The independent Intel observer therefore captures peer BlockAck cadence but,
in this laboratory mode, does not decode the ESP target's HT40 A-MPDU records
reliably enough to answer the full AP-TX question:

```text
end of ESP PPDU -> BlockAck response -> next ESP PPDU
```

Consequently the observed 4--27 ms cadence distribution proves that exchanges
are not evenly spaced, but it does not localize whether a gap precedes the
BlockAck, follows it inside the target, or belongs to the traffic source or
peer. AQL, TXQ accounting, BA response and target completion remain possible
investigation branches, not established causes.

### 4. Report schemas are local and uneven

The sealed run/repetition model is typed and versioned, but workload
attachments use local literal schema numbers and a mixture of JSON and
Markdown-only projections. That is valid today because attachments are opaque
to the bundle verifier. It becomes fragile when tools begin comparing their
fields.

Each machine-read attachment should have a named schema constant and an
explicit unavailable representation for optional evidence. Common throughput,
delivery, CPU residence and PHY facts should additionally be projected into
the existing typed repetition `Measurement` list; Markdown remains a view.

### 5. Scenario growth needs lifecycle policy, not templates

The 202 standalone TOML files are verbose but transparent and reconstructable.
Introducing inheritance or generated fragments would make an individual
historical recipe harder to audit. The catalog should retain standalone files.

What is missing is lifecycle metadata:

- `qualification` scenarios are stable gates;
- `diagnostic` scenarios are repeatable experiments;
- superseded diagnostics should be marked by a typed catalog field before
  removal, with the replacement ID recorded;
- image-class and feature compatibility should remain centrally validated.

No scenario ID or criterion should change during the behaviour-neutral module
split.

### 6. Startup calibration is not yet a replay input

The current S31 PHY intentionally rejects a supplied calibration cache and
performs complete calibration because the driver does not own a complete typed
hardware replay. The returned startup artifact is therefore an output, not an
input, and differing bytes did not explain the reproduced throughput failure.

The bundle should eventually retain the supplied artifact identity,
disposition and returned artifact identity explicitly. It must not pretend the
artifact enables deterministic PHY replay until production can safely apply
it.

### 7. Target and protocol splits are valuable but higher risk

Moving Rust types between modules does not change postcard encoding, but the
central `Command` and `Event` enum order is wire ABI. Target console and
product composition also own boot/lifecycle ordering that host-only unit tests
cannot fully exercise. These files should be split only after the host runner
boundaries are stable and with framing round-trip tests unchanged.

## Target structure

The intended host layout is:

```text
qualification/scenario/
  model.rs          serialized scenario vocabulary only
  validation.rs     semantic/cross-field rules
  catalog.rs        filesystem loading, uniqueness and selection

qualification/access_point/
  mod.rs            one AP lifecycle transaction
  clients.rs        scoped local/OpenWrt client ownership
  udp.rs            single-client UDP
  multi_client.rs   flow accounting and fairness checks
  tcp.rs
  icmp.rs
  report.rs         attachment schemas and projections

traffic/bidirectional/
  mod.rs            orchestration
  model.rs          evidence and assessment values
  parse.rs          diagnostic text parser only
  qualify.rs        typed acceptance rules
  report.rs         Markdown/measurement projection

evidence/traffic_capture/
  mod.rs            serial-capture owner facade
  protocol.rs       request/response and recovery state machine
  readiness.rs      UDP/TCP/network readiness probes
  validation.rs     typed terminal evidence invariants

reporting/run/
  model.rs          serialized run/bundle schemas
  session.rs        unpublished-to-sealed transaction
  archive.rs        firmware/source/CAS import
  integrity.rs      canonical file inventory
  render.rs         JUnit and HTML views
```

The public paths used by the rest of the runner should remain re-exported by
the facade modules. This keeps the first change mechanically reviewable and
prevents a broad call-site rewrite.

The later target layout should separate Wi-Fi observation, role composition
and IEEE 802.15.4 probes in `product_hil`, and separate emergency logging,
runtime log transport, command admission and session protocol in `console`.
Protocol domain types may then move into `message/network.rs`,
`message/wifi.rs`, `message/ieee802154.rs` and `message/startup.rs`, while the
central wire enums and discriminant tests remain obvious.

## Implementation order and gates

1. Record this audit and keep the fixture-epoch fix as the clean performance
   baseline.
2. Split scenario model/validation/catalog and their tests without changing a
   serialized type, default, catalog ID or validation error contract.
3. Split run model, integrity and rendering. Verify existing bundles before
   and after the change; generated canonical JSON, JUnit and HTML must remain
   byte-identical for the same fixture input.
4. Split AP and bidirectional workload modules. Existing focused tests and a
   clean exact-image AP TX replay are the behavioural gate.
5. Split `SerialCapture` by protocol/readiness/validation while retaining one
   serial owner and one decoder-health accumulator.
6. Add typed interval summaries and direction-neutral AP-TX air timing as
   separate evidence improvements, each with a focused schema/test and HIL A/B.
   This is complete through the honest BA-cadence observer described above;
   direct target-data pairing remains unavailable with the current adapter.
7. Split target composition and protocol domains only after host refactors are
   complete. Run protocol framing tests, target checks, placement/stack audits
   and representative STA/AP/STA+AP HIL.
8. Review the diagnostic catalog for explicit supersession metadata. Do not
   add scenario inheritance and do not delete historical recipes as part of a
   code-movement commit.

Every step must pass, as applicable:

```console
cargo hil scenario validate
cargo test -p open-esp-radio-hil-runner
cargo test -p open-esp-radio-hil-protocol
cargo check --manifest-path hil/targets/esp32s31/runtime/Cargo.toml \
  --target riscv32imafc-unknown-none-elf --features boot-smoke
cargo fmt --all -- --check
tools/audit-source-only.sh
```

Hardware-facing evidence changes additionally require exact-image replay or a
same-ELF A/B. Pure module movement does not justify reflashing hardware, but it
must not be combined with a scenario, fixture or acceptance-policy change.

### Target split progress and behavioural gate

The first target-only split moves the reset-isolated IEEE 802.15.4 probe and
evidence mapping domain out of `product_hil.rs` into
`product_hil/ieee802154.rs`. A mechanical comparison proved that all 588 moved
lines stayed identical apart from the two `pub(super)` entrypoint names. Both
probe feature profiles, the ordinary Wi-Fi profile, all wire tests and the
full source-only audit pass.

Because moving source changes embedded panic locations and can perturb an ELF
even without changing the executed Wi-Fi logic, the split was also tested on
hardware rather than accepted from source equivalence alone:

- AP performance run `1788269891637-00133af8` passed all six cycles at
  120.49--121.50 Mbit/s;
- STA performance run `1788270143730-00133e9b` passed all three repetitions at
  119.23--119.49 Mbit/s;
- STA+AP correctness run `1788270588780-001344d0` failed before workload on the
  AP network RX readiness boundary.

The STA+AP failure is not attributed to this split. Exact replay
`1788270814590-001348c1` of pre-split correctness image
`1788268379279-0012dc66` failed at the same boundary in the same current lab
state. Both images transmitted beacons, associated and authorized the client,
negotiated BlockAck, and admitted protected AP frames through MAC/reorder, but
reported zero completed AP network RX units. This localizes the next defect to
the STA+AP AP-to-network delivery boundary; it is neither an association
failure nor evidence of an air/AQL cause. The task-residence variant is not a
valid substitute gate: its run history is currently 0/3.

## Non-goals

- Do not make bit-identical rebuild verification part of every HIL run. Exact
  artifact replay is the current performance A/B mechanism.
- Do not put ELF or firmware binaries in Git.
- Do not move production behaviour into HIL probes.
- Do not require optional observers or generated reports for an otherwise
  valid test.
- Do not tune AQL, BA, rate control or fixture firmware from an unexplained
  counter pattern. First acquire direction-correct timing evidence.
- Do not rewrite the runner around a generic test framework; its typed radio,
  route and ownership contracts are project-specific and valuable.
