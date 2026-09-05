# HIL host

`runner/` owns the typed CLI, scenario catalog, build/flash orchestration and
UART evidence. `linux-net/` contains only privileged fixture operations.

Public commands:

```console
cargo hil doctor
cargo hil scenario list
cargo hil scenario validate [id]
cargo hil image build|flash <image-class>
cargo hil image verify-rebuild <image-class>
cargo hil image verify-rebuild <image-class> --trim-paths
cargo hil image replay <run-id> <image-class>
cargo hil device status
cargo hil report rebuild
cargo hil report verify [run-id]
cargo hil run <scenario-id>
cargo hil run <scenario-id> --firmware-from <run-id>
cargo hil run-all [--tag qualification]
```

Scenarios are versioned TOML files in domain folders under `hil/scenarios`; they contain workload,
isolation and acceptance criteria, never serial paths or secrets. Machine-local
device, STA/AP and OpenWrt values live only in mode-0600 `hil/local.toml`.

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
    └── workload evidence
```

`lab-provenance.json` is collected while the fixture lock is held and before a
firmware build or flash. It deliberately omits both network credentials and
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
verdict. ICMP latency/loss uses these typed measurements; rendering does not
parse Markdown reports to decide scenario outcomes.
A session unwound by a runner error is marked interrupted in the manifest.
Reconnect stores UART/protocol pairs per boot. The command emits one completion
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

`cargo hil report verify [run-id]` performs a read-only offline integrity
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
