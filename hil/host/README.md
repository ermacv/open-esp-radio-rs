# HIL host

`runner/` owns the typed CLI, scenario catalog, build/flash orchestration and
UART evidence. `linux-net/` contains only privileged fixture operations.

Public commands:

```console
cargo hil doctor
cargo hil doctor timebase
cargo hil plan udp-rx-ht40-ceiling
cargo hil scenario list
cargo hil scenario validate [id]
cargo hil image build|flash <image-class>
cargo hil image verify-rebuild <image-class>
cargo hil image verify-rebuild <image-class> --trim-paths
cargo hil image replay <run-id> <image-class>
cargo hil device status
cargo hil report rebuild
cargo hil report verify [run-id]
cargo hil archive export <archive-id> --run <run-id>
cargo hil archive verify|import <archive.tar.gz>
cargo hil archive publish <archive.tar.gz> --repo <owner/repository>
cargo hil archive fetch <archive-id> --repo <owner/repository>
cargo hil run <scenario-id>
cargo hil run <scenario-id> --firmware-from <run-id>
cargo hil run-all [--tag qualification]
```

The `network-comparison` tag selects the same five station workloads for each
network implementation: bidirectional UDP at 65 + 65 Mbit/s, RX-only and
TX-only at 130 Mbit/s, bidirectional UDP at 130 + 130 Mbit/s, and idle ping.
Each scenario runs once and uses the task-residence image. UDP windows last
12 seconds; idle ping sends 120 requests at 100 ms intervals. This is a quick
comparison, not a repeatability or endurance qualification. Build, association,
reset and cleanup time is additional. Run one implementation at a time:

```console
cargo hil run-all --tag network-comparison --network patched-xarxa
```

Select `upstream-xarxa`, `upstream-smoltcp` or `owned-xarxa` for the other
compositions. The throughput criteria still apply under overload; a completed
measurement can fail its speed gate. Task residence is not full CPU utilization.

The separate `ap-network-comparison` tag uses ESP as an HT40 AP with two
physical stations (laptop and OpenWrt). Each scenario has one boot, one AP
cycle and one 12-second UDP window:

| Workload | Offered traffic, relative to ESP |
| --- | --- |
| Balanced TX | 65 Mbit/s to each station |
| Balanced RX | 65 Mbit/s from each station |
| Balanced bidirectional | 32.5 Mbit/s in each direction per station |
| TX with sparse peer | 130 Mbit/s to laptop; two datagrams every 100 ms to OpenWrt |

```console
cargo hil run-all --tag ap-network-comparison --network patched-xarxa
```

The AP comparison checks per-peer progress, with an additional sparse-peer
delivery and interarrival gate. Its low throughput floors do not qualify
performance or fairness. Task residence and throughput alone do not establish
A-MPDU aggregation quality; that requires separate aggregation evidence.
Multi-client UDP saves each peer's raw host delivery and available target
transport counters in `cycle-*/delivery-progress.json` before terminal-evidence
and delivery/rate gates. A later gate failure does not discard these measurements.
Single-cycle measurements do not replace the catalog's repeated AP lifecycle
qualification scenarios.

Durable evidence packages and private remote storage are described in
[HIL archives](../../docs/hil-archives.md). Archive commands do not access the DUT.

`plan [scenario] [--tag ...]` resolves requirements from the catalog offline;
it does not read `hil/local.toml`, inspect tools or contact hardware. `doctor`
accepts the same selection and reports all independent environment checks as
JSON, returning nonzero if any fail. With no selection it checks the whole
catalog. It checks build/flash tools, scenario preconditions, required fixture
services and current cooperative resource availability; it neither flashes nor
resets the target. Availability is an observation, not a reservation for a
later run. Optional monitor tools are checked only when selected evidence uses
them. AP workloads currently include an initial STA connection, so their
requirements include the station network.

`cargo hil run memory-copy-benchmark` builds the dedicated memory diagnostic
image and measures CPU, blocking GDMA and async GDMA copies from SRAM and
PSRAM into SRAM. It requires the board and serial connection, without an AP
or network helper. `cargo hil run memory-copy-batch-benchmark` uses the same
image to compare CPU frame loops with single-chain GDMA batches of 1, 2, 8 and
32 frames. Scenarios select frame sizes, batch sizes, iterations and repeated
boots. An omitted `batch_sizes` field means `[1]`; every size/batch combination
must fit the 49,152-byte payload limit per iteration. Case order is source,
frame size, batch size, then copy mode.

`memory-benchmark.json` schema 2 preserves each requested case and its typed target
result, including failed observations. Each case has a 15-second host response
deadline. The runner checks completeness, data/guard results and counter-scope
consistency without imposing a throughput or speedup floor. Elapsed and
foreground cycles/instructions describe their measurement windows, not CPU
utilization or energy consumption. Measurements distinguish bytes per frame,
frames per iteration and total payload bytes per iteration; comparisons must
use the same geometry and source memory.

`device status` attaches to the flashed runtime without reset, provisioning,
initialization or result acknowledgement. The report includes the boot ID,
capabilities, operation state, retained session identity, stack snapshot when
available, and cumulative target link counters. A null stack means the target
cannot safely snapshot it in its current state. Existing link errors are
reported as observations. Every invocation gets its own directory under
`target/hil/esp32s31/device-status/`, containing `status.json` and UART evidence,
including on failure. Firmware must implement read-only boot discovery; there
is no reset fallback. Serial-driver line behavior remains platform-dependent;
the runner issues no reset-line sequence when attaching.

Scenarios are versioned TOML files in domain folders under `hil/scenarios`; they contain workload,
isolation and acceptance criteria, never serial paths or secrets. Machine-local
device, STA/AP and OpenWrt values live only in mode-0600 `hil/local.toml`.
`LabConfig` is immutable. Each workload receives its own execution context:
borrowed laboratory inputs and the selected scenario's initialization settings.
Experiment policies are never written back to the shared laboratory object.
The CLI grammar lives in `runner/src/cli.rs`. Scenario dispatch passes typed
workload configurations directly; workloads neither rebuild CLI arguments nor
parse private command-line dialects. Serial ownership comes from the execution
context, independently of traffic configuration. Workload limits and acceptance
policy remain enforced when constructing a running workload.

By default, `run` builds and flashes the scenario's exact image before
executing it. An explicit `--firmware-from` selects exact artifact replay
instead, never a rebuild. `run-all` groups scenarios by image class, so changing
UDP/TCP direction or rates does not rebuild or reflash firmware. It continues
after scenario, image-build and image-flash failures, records the remaining
scenarios as blocked when necessary, writes the complete suite, and returns a
non-zero status unless every selected scenario passed. Independent scenarios
reset the target. Ordinary scenario files must use `reset` isolation.

AP scenarios select a controlled Linux or OpenWrt client. The Linux fixture
leases WLAN as a managed WPA2 client without a gateway and restores
NetworkManager on every return path. Lifecycle, ICMP,
UDP and TCP are independent workloads over the same declared image class.
Correctness scenarios record terminal AP observations in
`access-point-report.json`; performance scenarios reject driver observations
and retain only transport, external-fixture and stack evidence. AP IP policy
belongs to HIL, not to the radio driver request.

Each invocation creates an immutable directory under
`target/hil/esp32s31/runs/<run-id>/`. The runner never deletes or reuses an old
run. Its canonical records are:

```text
manifest.json       invocation, repository, host, lab and firmware provenance
lab-provenance.json secret-free pre-run topology, host and fixture observation
plan.json           selected and filtered catalog entries
events.jsonl        append-only execution timeline
suite.json          typed suite/scenario/repetition outcomes
junit.xml           CI view derived from suite.json
report.html         human view derived from suite.json
integrity.json      deterministic size/SHA-256 inventory of the whole bundle
firmware/<image>/
├── build-provenance.json
│                   build recipe, source materials, tools and output subjects
├── application.bin exact application bytes used by the flash operation
├── runtime.elf     exact symbolized runtime used to produce the image
├── runtime.bin     exact packed stage-two runtime
├── bootstrap.elf   exact bootstrap used to encode the application
└── effective-Cargo.lock
                    embedded dependency resolution observed before restore
scenarios/<id>/
├── scenario.json
├── result.json
└── repetition-NNN/
    ├── result.json
    ├── host-route.json
    ├── cleanup.json
    ├── measurements.json
    └── workload evidence
```

`lab-provenance.json` is collected while the fixture lock is held and before a
firmware build or flash. Its explicit `system` scope collects OS facts and sysfs
interface identity without calling `ip`, `iw` or SSH; network fields are not
observed and the fixture is `not-used`. Network workloads use `network` scope.
Offline verification requires a matching archived plan and scenario snapshots
before accepting system-only provenance. It deliberately omits both network credentials and
transport endpoints. For a managed OpenWrt fixture it records the actual
release/kernel/boot identity, driver and firmware, country, TX power, channel,
frequency, width, associated-station count and concurrent VIFs. Host interface
and route-table state are recorded at the same boundary. The target-specific
route cannot exist reliably at that point, so every station traffic repetition
later writes `host-route.json` after address assignment and fails unless the
socket source and required Ethernet/WLAN medium match the kernel route.

Repetition records index every evidence attachment with its relative path,
media type, byte length and SHA-256 digest. Schema 2 repetition records also
carry typed measurements: a stable name, integer value, unit and, where the
scenario has a gate, its comparator, threshold and independently computed
verdict. Every workload family receives one repetition-owned recorder through
its execution context. Captures project decoded protocol values into numeric
observations and save a `measurements.json` beside `protocol.jsonl`, including
on error or unwinding. Repetition results include the same observations.
Names distinguish boot/capture paths, sessions, requests and individual flows;
`ReplayResult` cannot duplicate or replace the first traffic observation.
Transport rates use the target's reported elapsed time; zero elapsed time
produces no rate. Link counters are explicitly named as lifetime observations.
The projection includes transport, link/stack, timer, scan, monitor, AP peer
and ED polling measurements; other typed facts remain in `protocol.jsonl`.

Workload-owned ICMP loss/latency, station UDP RX/TX and TCP rate measurements
also expose their existing acceptance floors. The UDP RX target gate retains
its integer kbit/s resolution; the host offer gate retains bit/s resolution.
These are the resolved predicates, not new criteria. Target observations have
no implicit verdict, and numerical observations alone do not qualify a radio
feature. Broken or interrupted repetitions can retain failed measurements;
a passed repetition cannot. Rendering never parses Markdown to decide an
outcome. Host ICMP and TCP measurements are recorded before UART finalization,
so a later link failure does not discard completed host observations.
The HTML run report groups measurements by repetition in expandable sections;
the history page filters measurement trends by scenario or metric name.

The context owns the shared capture lifecycle: cancellation check, output-scope
validation, reset, observation collection and finalization. Bounded control
and probe operations use `with_capture`; concurrent traffic owners can keep an
explicit capture handle. Both paths retain primary and teardown errors, and
ordinary unwinding saves partial observations. Concrete fixtures still own
restoration of their AP/client/monitor state.

SIGINT and SIGTERM cancel protocol waits, paced traffic and supervised host
commands. The active repetition is saved as `interrupted`; execution does not
start another repetition or fabricate results for unexecuted scenarios. The
manifest and integrity index retain the partial run, and stdout reports its
location. An interrupted run does not require a completed suite.

Fixture owners restore partially configured resources on errors and cancellation.
`cleanup.json` records restoration attempts, elapsed time and failures separately
from the primary workload failure. Restoration runs within a bounded cleanup
scope (30 seconds, shared by nested operations). A cleanup failure alone makes
the repetition `broken`; it cannot turn a failed workload into a pass.
OpenWrt client preparation installs host recovery before restarting wireless;
the remote restart also restores wireless on ordinary shell exit or signals.
TX-monitor ownership starts before spawning SSH or waiting for readiness. A
private remote directory identifies that capture's resources, so a rejected
pre-existing monitor is left intact. Remote traps remove the owned interface;
host cleanup retries removal and deletes the capture directory, with failures
recorded in `cleanup.json`. Loss of SSH connectivity can prevent restoration;
the runner reports that failure rather than claiming the fixture was restored.
OpenWrt client cleanup checks the remaining NAT/forwarding rules and managed
interface. An already absent resource is safe to retry; an inspection or
deletion failure is recorded. Before signalling a stored PID, cleanup checks
that its command is `wpa_supplicant` with the owned interface and configuration;
a different command is left intact and reported as a recovery failure.
AP management, packet capture and fixture snapshot failures carry a typed
fixture error and produce `broken/infrastructure`, including errors returned
by the secondary-client probe thread. Actual peer packet loss remains a
scenario failure. A radio configuration mismatch
reports the required channel/width and the observed channel line without
including network credentials. These failures do not change scenario criteria.

The laptop helper contract is schema 6. Its `client` action returns status 10
only when a prepared client exhausts the association wait; command failures
and malformed supplicant status are infrastructure errors. `doctor` and the
selected run preflight reject older helpers before flashing or resetting the DUT.
Provision it with `sudo hil/host/linux-net/install.sh`
from the repository root before using laptop client scenarios.

`oer-process` owns local child process groups, drains captured stdout/stderr
concurrently and stops descendants on cancellation, deadlines or owner drop.
Routine commands have a 120-second deadline; image commands allow 30 minutes;
packet captures use their configured duration plus shutdown allowance. Remote
process lifetimes additionally depend on the OpenWrt scripts' timeouts and traps.

HIL cell leases and serial-device leases live in the user's host cache, outside
individual checkouts. Serial leases are shared with `cargo xtask build firmware <example> --flash`
and use USB identity when available, otherwise the canonical device path.
A run additionally leases every required local wiphy and the managed OpenWrt
host boot. Local client/monitor interfaces sharing a radio conflict even across
cell IDs. The remote boot identity makes different SSH aliases and radio
interfaces on one OpenWrt host conflict; the whole host is reserved because
client setup also changes firewall state. A remote reboot invalidates that
fixture epoch. These are cooperative locks between runners on this host and
user account, not distributed reservations across separate laboratory hosts.
External unmanaged APs have no discovered physical identity and rely on a
consistent cell ID. Build/flash-only commands and device inspection acquire no
AP or laptop-radio resources.

A session unwound by a runner error is marked interrupted in the manifest.
Each UART capture owns its output directory before opening or resetting the
serial device. `uart.bin` is the exact received stream, written as bytes arrive;
`uart.log` is its lossy UTF-8 view for diagnostics. `protocol.jsonl` contains
received target events, decoder counters, a `capture-end` record with the first
typed link failure, and the final target-health query when available. Host
commands and host error messages are never inserted into the received stream.

Serial open, reset, read, write and worker failures wake protocol waits. Decode
errors, receive overflow, sequence gaps and an unexpected new boot invalidate
the capture. A later boot cannot erase an earlier failure: each intentional
reset starts a new capture. Optional event waits distinguish a healthy timeout
from a broken link. A traffic result has one collection deadline; target
session and Wi-Fi operation failures terminate their corresponding waits.
Before acknowledging a completed traffic session, the host requests its
retained result again and requires identical evidence and completion digest.
These protocol exchanges run after the measured traffic interval.

Host I/O and typed link failures produce `broken/infrastructure`; scenario
assertions produce `failed/scenario`. Operation context preserves the typed
cause. Capture finalization saves partial evidence before returning a link
failure; when both the scenario and finalization fail, the report retains both
messages and classifies the primary cause. Ordinary early returns also save
the decoded transcript through the capture's destructor. Abrupt process
termination can leave only the incrementally written raw bytes; it does not
run Rust destructors or seal a completed run.

Reconnect stores captures per boot. The command emits one completion
JSON object on stdout; diagnostics, progress and inherited child-process output
belong on stderr.

After a completed run, the runner deterministically rebuilds
`target/hil/esp32s31/history.json` and `history.html`. These are disposable
views, not authoritative state: `cargo hil report rebuild` recreates them from
the immutable manifests and suites without hardware access. The history view
shows run/cell/DUT provenance plus per-scenario pass rate, mixed-outcome
flakiness and the current consecutive non-passed count. Measurement series are
kept separate by scenario, name, unit and threshold contract, and expose
minimum/latest/maximum values plus failed-verdict counts. A malformed or
inconsistent run bundle makes rebuilding fail closed.

Publication of a new run directory and history snapshots share a short-lived
index lock. History writers hold it through snapshot and publication, so a
slower writer cannot overwrite a newer snapshot. Firmware builds and hardware
workloads run outside that lock. A malformed unrelated bundle still makes
`report rebuild` fail explicitly. It cannot revoke a completed run: the run's
completion JSON retains its outcome and artifact paths, sets `history_report`
and `history_html` to null, and reports `history_failure`. Retrying the derived
view does not change the sealed bundle.

`cargo hil report verify [run-id]
cargo hil archive export <archive-id> --run <run-id>
cargo hil archive verify|import <archive.tar.gz>
cargo hil archive publish <archive.tar.gz> --repo <owner/repository>
cargo hil archive fetch <archive-id> --repo <owner/repository>` performs a read-only offline integrity
check. With no run ID it checks every bundle. It validates manifest/suite
structure, canonical relative paths, regular-file boundaries, attachment byte
lengths and SHA-256 digests, plus the archived application image for every
recorded firmware class. Completed and interrupted runs are sealed by
`integrity.json`; verification also requires an exact match for every regular
file in the bundle, including plan, event stream, scenario records and derived
JUnit/HTML views. Unindexed additions, missing files, symlinks, path traversal
and changed content fail closed. The hashes detect accidental corruption and
internally inconsistent bundles; because they live beside the evidence, they
are not a signature against a malicious rewrite of the whole bundle.

Runs retain all firmware subjects through a SHA-256 content-addressed
store under `target/hil/<target>/objects/`. The files inside a run are ordinary
hard links when the filesystem supports them, or independent copies
otherwise. This keeps a copied run self-contained without allocating another
large runtime ELF for every repeated scenario. Build provenance follows the
subjects/materials/recipe separation described in
[build and report reproducibility](../../docs/hil-reproducibility.md). A tracked dirty delta is stored
as a binary Git patch; untracked content is identified but never copied
implicitly, and makes source reconstruction incomplete.

`cargo hil image replay <run-id> <image-class>` first performs the same offline
bundle verification and then flashes the archived `application.bin` without
running Cargo or changing its bytes. It is the supported primitive for exact
same-artifact comparison. It does not by itself rerun a scenario or claim that the lab
environment matches the original run.

`cargo hil run <scenario-id> --firmware-from <run-id>` performs the same
verification before acquiring the physical fixture, requires the archived
image class to match the selected scenario, and then executes the ordinary
scenario lifecycle without invoking Cargo. The resulting run imports all
available firmware subjects, effective lock and tracked source patches into
its own CAS-backed bundle; it remains verifiable after the source run is
removed. Its manifest records the source run and source integrity digest, and
the independent qualification reader deliberately excludes replayed firmware
from current-clean evidence. `run-all` does not accept `--firmware-from`.

The qualification evaluator consumes these same sealed bundles through an
independent reader. `qualification/targets/<chip>/*.toml` maps capabilities to
scenario IDs and minimum passing repetitions; only a bundle produced from the
exact current commit with a clean worktree can satisfy the HIL axis. The
derived history views and Markdown narratives are never proof inputs.

`boot-smoke` intentionally precedes the radio protocol and proves only runtime
relocation plus one Embassy timer wake. It uses its single fixed PASS record;
all radio, lifecycle and traffic evidence uses the typed HIL protocol.

## Source ownership

`runner/src` follows execution and evidence boundaries:

- `scenario` owns catalog values, discovery and semantic acceptance rules;
  `image/class` owns image identities and feature recipes.
- `image` owns build/rebuild and placement/stack auditing; the reusable ELF
  analyzer remains `tools/memory-report`.
- `lab` owns local configuration, topology/provenance and the exclusive fixture
  guard; `fixture` implements controlled host and peer capabilities.
- `session` owns one UART capture and its protocol/readiness/validation state.
- `workload` groups system, IEEE 802.15.4, IEEE 802.11 role and network traffic
  operations. They report scenario outcomes, not product readiness.
- `evidence` owns sealed run models, archive/integrity/verification and build
  provenance. `reporting` renders HTML/JUnit and rebuildable history views.

The recursive [catalog contract](../scenarios/README.md) is checked independently
by the runner and qualification evaluator. Shared synthetic input documents
exercise both readers; qualification never imports execution or validation
implementation from the runner. Tests are adjacent files within each owner.
