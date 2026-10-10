# Project capability map and qualification

Qualification owns the engineering map and the assessment of a selected supported
path. The map connects hardware knowledge, implementation owners, checks,
observations and remaining work. A program selects a precise scope for strict
evaluation; the wider project inventory has no aggregate readiness verdict.
The checked-in TOML catalogs and programs own these declarations, and
`evaluator/` reads them independently of the evidence producers. Implementation,
host coverage and async states are reviewed declarations; vendor and HIL
states are derived from independent evidence. Qualification remains the sole
readiness authority for the selected scope.

The root-workspace [`evidence-shard/`](evidence-shard/README.md) library owns
the shared vendor and HIL evidence shard format, validation, currency and
reader/writer. The evaluator records HIL shards through it, and verification
producers use the same format for their scenario evidence.

Saved HIL evidence uses the current observer descriptor prepared by `cargo hil`
or `cargo hil observer`. Evaluation reads it once and never builds or
executes a runner. An unavailable descriptor is diagnosed separately from an
incompatible historical observer; the observations remain in the report.
See [observer preparation and cancellation](../hil/host/README.md) for the
build configuration, receipt selection and explicit offline option.

## Everyday status and next work

Read declarations without loading vendor results, HIL runs or Git state:

```console
cargo qualification status \
  --catalog qualification/catalog/esp32s31/bluetooth-products.toml \
  --capability secure-peripheral-gatt \
  --json-report target/qualification/secure-gatt-map.json

cargo qualification next \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --capability runtime-phy-calibration
```

Read existing evidence for a selected program, without running any checks:

```console
cargo qualification status \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --capability runtime-phy-calibration \
  --json-report target/qualification/wifi-calibration-map.json

cargo qualification next \
  --manifest qualification/targets/esp32s31/bluetooth-secure-gatt.toml \
  --capability secure-peripheral-gatt
```

Both commands accept multiple `--catalog` inputs, or one `--manifest`.
`--capability` selects that capability and its transitive dependency context.
An unknown or unselected ID is an error. This closure is not a change-impact
analysis or an instruction to rerun its checks. Without a focus, catalog mode
includes all source facts, inventory facets and references as well as capability
declarations; program mode includes the selected program and linked source facts.
Neither command fails just because work remains.
Invalid declarations and corrupt evidence still fail validation.
`status` prints a compact state view and counts of original HIL observations,
including excluded history. `status --details` expands scopes, limits, owner
links and individual applicability decisions. JSON always contains the full
selected map. Program mode validates the saved evidence archive and can take
longer than the declarations-only view; it does not regenerate evidence.

The schema-1 JSON map separates source declarations, knowledge links, test
selectors, original HIL decisions and explained work candidates. Static mode
marks evidence `null` / `not-evaluated`; it never labels unread results missing.
Knowledge is `not-linked` or `linked-not-assessed`: the map does not infer how
well hardware is understood from a file's existence or a vendor MATCH.
Host selectors are navigation, not recorded test passes. HIL observations retain
exclusion reasons and completion boundaries from the existing evaluator.

Next work distinguishes research, implementation, host coverage, experiments,
measurement methods, inspection of excluded observations, incomplete attempts
and current failures. Declarations-only mode asks to inspect evidence before deciding to
repeat an experiment. Existing gaps may carry a reviewed work classification;
unclassified HIL gaps require review rather than interpretation of their names.
Actions are deterministic candidates with reasons, not a priority ranking or an
automatic execution plan. Choose the goal first, then inspect its owners and
checks. Current captured-input research is described in the
[Blobray task map](../tools/blobray/README.md#choose-a-task).

### Linking knowledge and focused checks

Optional `[capabilities.development]` metadata adds references to existing
owners and classifies existing gaps. For example:

```toml
[capabilities.development]
knowledge = ["crates/hardware/esp32s31/phy/src/tracking/rfpll/README.md"]
host-tests = [
  { manifest = "crates/hardware/esp32s31/phy/Cargo.toml", filter = "tracking::rfpll::tests", source = "crates/hardware/esp32s31/phy/src/tracking/rfpll/tests.rs" },
]
gap-work = [
  { gap = "opposite-thermal-correction-and-radiated-rf-quality-missing", kind = "measurement-method", reason = "Radiated RF quality requires a separately identified observer and procedure." },
]
```

Knowledge and test paths must name regular repository files without symlinks.
A test manifest must declare a Cargo package; its filter is a reviewed Cargo
test selector, not a source-text discovery mechanism. Static validation does
not compile tests or prove a filter currently matches a test. `gap-work` must
name exactly one existing gap and retain a reason. Accepted declared kinds are
`research`, `implement`, `host-test`, `experiment`, `measurement-method`,
`inspect-vendor` and `review-gap`. Removing a gap requires removing its annotation.
These links never alter readiness requirements. Source contracts continue to
own implementation paths and limits; vendor roots, evidence rows and HIL
requirements retain their existing owners.

Focused links cover Wi-Fi DMA, calibration/RFPLL and peripheral BLE
maintenance/security. Other declarations remain visible with missing links;
missing navigation is not an absent implementation or an unexplored chip.

The focused product programs are Wi-Fi STA and BLE peripheral/ACL, followed by
secure peripheral GATT. Their definitions share canonical capability declarations:

| Program | Required product boundary |
| --- | --- |
| [Wi-Fi AP availability](targets/esp32s31/wifi-ap-availability.toml) | Controlled AP-loss recovery with fresh IP exchange, and bounded initial no-candidate exhaustion |
| [Wi-Fi STA](targets/esp32s31/wifi-sta.toml) | Station association/WPA2, datapath, recovery and PHY/power lifecycle |
| [BLE peripheral/ACL](targets/esp32s31/bluetooth-peripheral-acl.toml) | One LE 1M connection, bidirectional ACL, recovery and terminal powered release |
| [Secure peripheral GATT](targets/esp32s31/bluetooth-secure-gatt.toml) | The complete peripheral/ACL boundary plus encrypted traffic, Secure Connections, protected ATT access and bonded reconnect |

The [full Bluetooth LE program](targets/esp32s31/bluetooth-le.toml) retains its
wider role and feature requirements. [IEEE 802.15.4](targets/esp32s31/ieee802154.toml)
is an independent program. Product selection does not change source coverage or
make any hardware evidence current.


## Capability catalogs and program resolution

Capability declarations live below `catalog/`. A qualification program names
one or more catalog files with `catalogs` and selects stable IDs with
`catalog-capabilities`; both are required, and a program declares no
capability itself. Resolution adds the selected declarations and their
catalog-owned dependency closure to the program before the evaluator
runs.

### Imports and required sets

Catalogs declare shared inputs with `imports = ["qualification/catalog/…"]`.
Paths are relative to the repository root. Imports resolve transitively; a shared
catalog is loaded once even when also explicitly selected. Missing inputs,
import cycles and repeated imports within one catalog are errors. Every imported
source retains its own identity and hash in the rendered/evaluated provenance.
Importing a catalog does not select all of its capabilities for a product.

A program normally supplies an exact `required-capabilities` list. Alternatively,
`required-capabilities-from = "catalog-closure"` explicitly derives the full set
from `catalog-capabilities` and transitive dependencies. That mode cannot mix
explicit required IDs. Missing policy is not permission
to derive a set: the existing exact-set checks still apply. Added dependencies
become mandatory automatically in closure mode and retain all their evidence
requirements. The evaluator report lists the resolved membership and provenance.

Every catalog declaration keeps the existing reviewed/evidence axes and may
carry the existing source contracts. Its required `catalog-scope` identifies
the chip, role, PHY, security set, composition, native/lower/composed level,
activation boundary and limitations. This metadata is validated and rendered,
but it is not a readiness axis.

### Catalog owners

The ESP32-S31 [Wi-Fi/PHY catalog](catalog/esp32s31/wifi-phy.toml) owns all 14
Wi-Fi STA qualification declarations and the wider Wi-Fi/shared-PHY source
inventory. Direct roots and their dependency closure resolve to the same exact
14 required IDs. Source-facet
inventory status and native/lower/composed level are separate from evaluator
axes; unselected inventory entries have no readiness value.

The [Bluetooth catalog](catalog/esp32s31/bluetooth.toml) owns all 68 existing
Bluetooth LE qualification declarations and the wider LE, Classic and Host-only
source inventory. It imports the Wi-Fi/PHY and
[coexistence](catalog/esp32s31/coex.toml) catalogs to reuse the exact
Bluetooth initial handoff, idle PHY maintenance, idle physical release and
same-storage restart facts, alongside the diagnostic `coex-timer-validation-bridge`.
The periodic Bluetooth maintenance fact owns configured active-ACL windows and
DTM hard-expiry behavior; qualified timing/RF bounds and encrypted recovery
remain separate limits. Implemented subsets do not promote the incomplete common-PHY, powered
teardown or coexistence lifetimes.

The coexistence catalog and the
[whole-radio catalog](catalog/esp32s31/whole-radio.toml) are source-inventory
owners, not qualification programs. They add no capability declaration or
readiness authority. Whole-radio views load the Wi-Fi/PHY and coexistence
catalogs explicitly because they project facts owned there; a missing fact
owner is an error rather than an omitted row.

The [IEEE 802.15.4 catalog](catalog/esp32s31/ieee802154.toml) owns the existing
six-capability radio/MAC program and its wider PHY, CCA, dataplane, filtering,
timing, security, power, coexistence and Host-only source inventory. Its
`ieee802154-registered-timing-entry` and
`ieee802154-mac-operation-subset` facts expose implemented lower operations
without promoting their incomplete RF-ready and public-dataplane parents.

### Check and render catalogs

Static catalog validation checks every declaration, dependency graph, source
contract, disposition mapping, HIL scenario reference and inventory path
without loading a vendor evidence index or HIL runs:

```console
cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --out target/qualification/catalog/wifi-phy-static

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --out target/qualification/catalog/wifi-sta

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/bluetooth.toml

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml \
  --out target/qualification/catalog/bluetooth-le

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/ieee802154.toml

cargo qualification catalog render \
  --manifest qualification/targets/esp32s31/ieee802154.toml \
  --out target/qualification/catalog/ieee802154

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --out target/qualification/catalog/coex-static

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/whole-radio.toml \
  --out target/qualification/catalog/whole-radio-static

cargo qualification catalog render \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/bluetooth.toml \
  --catalog qualification/catalog/esp32s31/whole-radio.toml \
  --catalog qualification/catalog/esp32s31/ieee802154.toml \
  --out target/qualification/catalog/esp32s31-radio-static
```

Static render writes `project-status.md`, `domain-inventory.md` and
`capability-catalog.md`; each entry names the code that owns it (its
`// CAPABILITY:` anchors). Manifest render additionally
writes `program-status.md` and `program-inventory.md` after evaluation. The program view records repository
commit/dirty state and configured evidence provenance; a catalog or manifest
hash is source identity, never firmware identity. All outputs are ignored
views, not another readiness decision or tracked snapshot.

`cargo xtask check docs` invokes the static catalog owner for all catalogs and
program selections, and renders the complete catalog set in both input orders
to check deterministic presentation. It uses `catalog check --catalog`,
`catalog check --manifest`, and `catalog render --catalog`; it does not use
manifest rendering, `validate` or `evaluate`, and it does not read
vendor evidence or HIL runs. Its ignored static views live below
`target/docs/catalogs/` and carry no readiness verdict.

The Bluetooth LE program includes legacy and extended roles, connected PHY and
control procedures, security/privacy, periodic advertising and PAwR, Direction
Finding, ISO in both connected and broadcast roles, LE Audio and the named LE
Host integrations. It also requires capacity admission, power lifecycle and
coexistence. Each new scope retains explicit incomplete axes until its own
production composition and evidence exist. The chip's
[feature inventory](../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md#qualification-scope-mapping)
maps these requirements to current source boundaries.

### Validate and evaluate a program

Validate each program from the repository root:

```console
cargo qualification validate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/ieee802154.toml
```

The two commands have deliberately different contracts:

- `validate` rejects malformed manifests, unsafe references, invalid
  dependency graphs, mismatched verification inputs and corrupt HIL bundles;
  an incomplete target is still a valid development state;
- `evaluate` emits the same derived verdict and optionally a complete JSON
  report through `--json-report PATH`.

`catalog check --catalog` and `catalog render --catalog` are static operations
over the complete explicitly loaded catalog set. The `--manifest` check form
also resolves selected IDs and dependencies and enforces the program's exact
`required-capabilities` set or explicitly selected `catalog-closure` policy, still
without reading vendor evidence or HIL runs.
Manifest render then evaluates that same statically validated program and
writes the separate readiness view.

## Focused Bluetooth products

[The product catalog](catalog/esp32s31/bluetooth-products.toml) defines the exact
single-peripheral criteria. It imports the existing cold-start, common-PHY,
HCI and portable protocol prerequisites. Broad multi-role capabilities are
separate; the product criteria explicitly retain single-role IRQ, timer, list,
capacity, cancellation and powered-cleanup obligations. Passing an ACL exchange
or logical HCI Reset does not establish the complete product lifecycle.

The peripheral program selects the existing recovery, local Disconnect, local
Reset, RF-loss and soak scenarios, plus physical retirement, same-storage
powered restart, quiescent PHY maintenance and automatic maintenance with live
ACL traffic. Qualified execution/restoration bounds and complete terminal-fault
coverage remain gaps; active DTM is non-preemptible and has a separate
hard-deadline fail-stop requirement. See the canonical
[periodic maintenance contract](catalog/esp32s31/wifi-phy.toml).
The secure program additionally requires key/counter/MIC handling, Numeric
Comparison-only Secure Connections pairing, authenticated ATT, RAM-bond
restoration and coordinated Host/Controller shutdown. Its automated secure
GATT scenario checks pairing rejection, protected traffic, RAM-bonded reconnect
after physical Controller cold restart, withheld Reset-reader retention and
terminal cold close after injected bond-load failure. The separate HCI-reader
failure scenario checks retained ownership, not successful cold close. Neither
establishes human presence, bond retention across SoC reset or all terminal
fault dispositions. The timing diagnostic uses boundary-only IRQ watermark
sampling; it does not replace the required every-snapshot memory-stress scenario.
The canonical catalog selects the required repetitions and retains
the missing lifecycle evidence separately. Existing plaintext ACL scenarios do
not supply encrypted or GATT evidence.

Check the selected programs without scanning the evidence archive:

```console
cargo qualification catalog check --manifest qualification/targets/esp32s31/bluetooth-peripheral-acl.toml
cargo qualification catalog check --manifest qualification/targets/esp32s31/bluetooth-secure-gatt.toml
```

Use `evaluate --manifest PATH --json-report PATH` for a current evidence-backed
assessment; its verdict says which selected capabilities are ready.
The catalog-check summary counts all loaded catalog declarations, including
unselected ones; the evaluator report describes only the resolved product set.

## Execution selection

`cargo qualification plan --manifest <program> [--capability <id>]` emits a
read-only JSON selection from the same HIL decisions as `status` and `evaluate`.
Each obligation explains `satisfied`, `run`, `review` (inspect excluded
observations), `investigate` (a current failure) or `unsupported`, and binds the
digest of the scenario's current normalized document. It performs no build,
test or hardware action; run the scenarios it names with `cargo hil run`. A focused
capability plan includes only its own obligations; prerequisite capabilities
remain context, not additional execution requests.

## Declared and derived axes

Every capability has five independent axes:

- `implementation` is the reviewed declaration `complete` or `incomplete`;
- `host` is the reviewed declaration `covered` or `incomplete`. The evaluator
  checks consistency with declared gaps; it does not find or run Rust tests.
  Workspace testing remains a separate repository check;
- `vendor` is derived from the vendor evidence index of the program's chip,
  derived data in `target/verification/<chip>/evidence` that
  `cargo verification evidence --chip <chip>` computes for the checkout. It
  qualifies only when every declared root has an evidence reference with a
  MATCH entry in that index and no source-only anchor remains. One stale
  shard (a recorded source changed or gone, or another format schema) voids
  the whole index;
- `hil` is derived from sealed schema-2 HIL run bundles and the tracked HIL
  evidence shards recorded from them. Every required scenario
  must pass with enough repetitions in an independently sealed attempt or run
  bound to the current source inputs;
- `async` is the reviewed declaration `bounded`, `incomplete`, or explicitly
  `not-applicable` with a reason. Consistency with gaps is checked; the
  evaluator does not infer executor behavior from source names.

`proof-ready` means all five axes are terminal. `ready` additionally requires
every dependency to be ready. `required-capabilities` must exactly equal the
manifest capability set, preventing a mismatch between required roots and declared capabilities.
Changing the declared program still requires review; validation cannot prove
that a removed capability was unnecessary.

Known `gaps` declare reviewed blockers rather than override outcomes. The
evaluator also derives a gap whenever required machine evidence is absent.

## Source contracts

A capability may attach `[[capabilities.source-contracts]]` reference entries
to describe its hardware-facing implementation paths. The ESP32-S31 Wi-Fi
catalog keeps SRAM/PSRAM DMA, descriptor chaining, scatter/gather and cache
handoff boundaries under `rx-tx-dma`. Cold calibration/cache, Bluetooth DTM and
PHY lifetime, coexistence, and IEEE 802.15.4 lower operation contracts likewise
make implemented subsets visible without promoting their broader parents.
The [inventory agreement contract](../docs/verification-and-qualification.md#agreement-with-feature-inventories)
requires matching implementation descriptions for the same scope.

Each entry has a unique `id`, an explicit `scope`, `limits`, repository-relative
`source-paths`, and a `composition`:

- `production`: the production owner composes the operation in the stated scope;
- `diagnostic`: an executable diagnostic composition exists;
- `unimplemented`: the stated composition has no implementation. This says
  nothing about whether the hardware could support it.

These declarations describe reviewed source coverage. Unknown hardware
reachability and ownership limits stay explicit in `limits`; source references
do not establish hardware support or measured performance. The evaluator checks
unique IDs, required descriptions and unique regular source files within the
repository, and preserves the entries in each JSON capability's
`source_contracts` array. It does not infer implementation from file contents.
Entries are optional reference metadata and do not change the five readiness
axes, evidence requirements or capability dependency graph.

## Code anchors

The catalog states what is supported; a code anchor states where. A
`// CAPABILITY: <id>[, <id>...]` line comment directly above a Rust item,
after its doc comments and attributes, names the inventory items, source
facts or catalog capabilities that item owns (the line grammar has one
recogniser, `oer_tidy::anchors`, which `cargo tidy check` uses too):

```rust
/// Finite station attempts and reconnect generations.
// CAPABILITY: station-lifecycle-owners
pub struct StationSupervisor { /* ... */ }
```

`cargo qualification catalog anchors --catalog PATH ...` scans every Rust file
of the repository and checks the anchors against the selected catalogs; `cargo
xtask check capabilities` passes all of them, because an anchor may name an
entry of any catalog. The rules follow the declared source status:

| Declared state | Anchors required |
| --- | --- |
| `implemented`, `partial`, `fail-closed` | At least one in a package of a production layer |
| `diagnostic` | At least one, in any package |
| `host-only` | Optional: the entry belongs to an upper protocol stack, not the radio |
| `absent` | None |
| Capability with `implementation = "complete"` | A production anchor on the capability, or `source-fact-refs` whose facts are all implemented and anchored |
| Capability with `implementation = "incomplete"` | Optional |

Every anchor must name an existing entry. An inventory item that projects a
source fact is anchored through that fact. An item that lists its `packages`
must be anchored only in those packages, so the listed owners cannot drift
from the code. With `--changed FILE`, the command
also lists each entry anchored in an edited file (`CAPABILITY-CHANGED`), so
the author can confirm that its declared status and limits still hold; that
list never fails the check.

Anchors are reference metadata like `source-paths`: they locate the owner and
keep the declared state from outliving the code, but they are not evidence and
do not change a readiness axis.

## Evidence ownership

```text
reviewed capability declarations ─┐
vendor evidence shards ────────────┤
sealed HIL run bundles ────────────┼─> qualification evaluator ─> JSON/verdict
tracked HIL evidence shards ───────┘
```

Blobray and the HIL runner never decide product readiness. Blobray owns vendor
comparison truth; the HIL runner owns hardware execution truth; qualification
maps both into the declared capability graph.

Capability dependencies describe readiness, not instructions to re-execute
every prerequisite scenario.

### HIL requirements and scenario roles

A HIL requirement names a scenario of the program's HIL catalog, and the
programs decide each scenario's [role](../hil/scenarios/README.md#roles):
validating a program fails while a scenario declared `qualification` is
referenced by no program of `qualification/targets`. A requirement on an
`investigation` scenario, such as one on a diagnostic image, is never
satisfied: its decision is marked `investigation`, it derives the HIL gap
`requirement-names-investigation-scenario`, and its next work is to re-home
it to a qualification scenario on a product image.

### Named checks

HIL requirements may select named `checks` in addition to the scenario and
minimum repetitions. The scenario owns thresholds; requirements reference
names, not duplicate numeric limits. Static validation rejects unsupported or
duplicate checks. The evaluator independently validates each recorded numeric
value, unit, threshold and original verdict, then evaluates the value against
the requested current criterion. A new criterion can be assessed from sufficient
existing measurements without rewriting their original assessment. Missing observations
remain missing even when the scenario says PASS. All checks in an obligation
must be supplied by one eligible run; individual successes from different runs
cannot be combined to satisfy it. JSON `hil_checks` exposes individual evidence
references or `null`; those diagnostic rows do not replace the conjunction.
The console exposes the same detail as `HIL-CHECK` rows. Absence means no
eligible proof, not necessarily that the check has never been executed.

### HIL decisions and source currency

JSON `hil_decisions` explains each complete obligation: its applicability policy,
completion boundary, status, selected evidence, and every observed scenario's
exclusion reasons and unmet requirements. Console `HIL-OBLIGATION` rows summarize
these decisions. Only current evidence counts: an observation whose sources
changed since is listed with its exclusions, and the obligation stays
`missing`. Completed scenarios remain candidates when an unrelated
scenario fails in the same sealed suite. A failed current scenario or repetition
cannot be hidden by selecting a later PASS: the status is `unresolved-failure`
while that observation stays current. `broken`, `blocked`, `skipped` and `interrupted` alone are
neither PASS nor a product failure, but they do not erase a different repetition's
explicit failure.

The supported completion boundary remains one complete scenario repetition set.
Named checks are not independently sealed lifecycle phases: a successful early
check in a failed lifecycle cannot qualify that lifecycle or become independent
evidence merely by selecting its name. Reassessment against a weaker numeric
criterion likewise does not turn a failed lifecycle into PASS. A completed
observation below the requested criterion is an unresolved failure; an absent
measurement is missing evidence, not an invented failure.

Named checks cover station UDP RX rates, configured maximum RX
silence, a validated maintenance transaction, and absence of station lifecycle
change through that transaction's traffic session. Semantic checks are recorded
only when attempted. They do not establish RF quality, a qualified PHY execution
bound, or relative performance non-regression. Default applicability is
`current-source-composition`: verified current source binding and no unbound
replay. Currency is judged on the run's closure, the files that can change its
observation: every file each of its image builds read (the
`source-inputs.json` the build recorded), the files of the scenarios it ran,
the workspace manifest, lockfile and toolchain, and the HIL runner's packages,
of the HIL protocol only the framework, `base`, the modules of the messages the
run exchanged (its manifest's `messages_used`) and their dependencies as
`hil/protocol/messages.lock` lists them,
except the code that only operates the stand (the arbiter's leases and queue,
flash transactions, and the post-mortem, recovery and profile reports of
failed repetitions) and their tests and prose. A
run whose images recorded no complete inputs falls back to the checkout-wide
closure of every firmware and runner package. A build from a clean commit binds while no
closure file differs between that commit and the checkout, tracked or
untracked, so commits that touch only other crates, image classes or
scenarios leave it current. A `source-snapshot` build can establish this direct binding:
the evaluator independently verifies snapshot identities, every archived file,
and the current tracked and nonignored untracked file set of the closure, bytes and executable modes. It also
checks the current lockfile and local override pins. A matching snapshot binds
regardless of dirty state or commit identity. Nothing admits an observation
whose closure differs from the checkout; original outcomes and exclusions
remain visible, and a commit change or another PASS does not establish that a
failure was resolved.

### Run bundles and tracked HIL evidence

A run bundle stays in ignored output, in the run store every checkout of the
user shares (see [find and compare runs](../hil/host/runs.md#find-and-compare-runs)),
and qualifies only for a checkout whose sources it binds. The evaluator reads
the store only through the checkout's `target/hil/runs` link, which
any `cargo hil` command creates.
The runner's observer proof names the build its executable was compiled from,
which most runs share. A run's manifest names that build by
digest, and the build is stored once in `observers/<build_sha256>.json` next to
the store's `runs` directory, as the exact bytes the digest is computed over.
A shard does the same with `observers/` next to it in the evidence directory.
The evaluator fails closed when a named build is missing or its bytes do not
hash to its name. A stored record that embeds its build instead is an error.
A build holds the observer's resolved Cargo graph, each of Cargo's nodes (a
package with its features) once with its edges: under a megabyte. Builds of
schema 2 recorded the graph as a tree with a node per path, a few hundred
megabytes; they are not read, so their observations are not this observer's
(`observer-identity-not-established`). The store keeps one build per observer
its runs used, so an observation keeps only the reference: the evaluator
parses each build once per evaluation, for every observation naming it
together, and drops it before the next, while the current observer's side
(its projection per workload and the digests of that workload's inputs) is
computed once.
Digests of sealed files are remembered in the user's cache per file identity
and status-change time (`OER_QUALIFICATION_HASH_CACHE=0` disables it). `cargo qualification hil-evidence (--manifest PATH |
--hil-target TARGET)` records
the latest qualifying observation of every scenario as a tracked shard in the
program's `[hil] evidence` directory (`hil/evidence/<chip>/`). A shard holds
the observation's outcome, repetitions, measurements and failures, the run's
completion seal, the observation subject (observer proof, firmware identity,
repository provenance) and the executed scenario document, and the digests of
the sources the firmware and observer were built from. The runner records, per
image, the repository files its build read in the bundle's
`firmware/<image>/source-inputs.json` (schema 2): the sources Cargo's dep-info
lists for the runtime and bootstrap binaries, the repository inputs their build
scripts declare (linker scripts), each compiled package's manifest and build
script, both firmware workspaces' manifests (release profile and `[patch]`)
and lock files, the Cargo configuration and toolchain files above them, the
workspace manifest the compiled packages inherit from, the stack policy and
partition table, and the sources of the image builder, packer and memory
auditor. A shard binds those files, its own observer's manifest directories
and the observer's lock, toolchain and input registry, so a change in another
radio's driver leaves it current. A record of another schema is an error.
The evaluator independently takes the path-package closure of the image's
runtime (with the features its build provenance records) and bootstrap for
the image's target from the repository model (`oer-repo`); when the recorded
list lacks the manifest of any package found there, the shard falls back to
the broad binding. An observation whose
firmware was built with inherited `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` is
never recorded: no source binding covers the builder's environment, and
`hil-evidence` names the skipped run. Firmware builds drop inherited
`CARGO_PROFILE_*`, `CARGO_BUILD_*` and `CARGO_TARGET_*` variables except the
job count and target directory. A replayed
image, or one built without this repository's image builder, has no recorded
inputs and binds the path packages of
`hil/targets/esp32s31` and `platform/esp32s31` and every qualifying observer
instead. The evaluator reads shards next to run bundles. A shard whose
recorded sources all match the checkout supports its scenario whatever else the
repository changed, and those digests stand for the observer's identity; a
changed source makes it stale until the scenario runs again and is recorded,
and a bound source that no longer exists is an error: record the scenario again
or delete the shard.
Running scenarios never writes tracked files: recording is an explicit step,
`cargo qualification hil-evidence --hil-target <chip> --pending` (this
checkout's pending clean runs, which leave the pending list once observed) or
`--run ID`; the evaluator reads the runs itself with the checkout's current
observer descriptor, and the stand never runs it. `cargo hil evidence
pending` lists what is not recorded yet. `--hil-target` selects the
programs naming that HIL target, which must agree on their run and evidence
directories. Commit the shards. `INPUT` reports `hil-shards` and `hil-current-shards`.

### Bundle validation and attempt seals

The HIL runner writes bundles below `target/hil/runs/<run-id>/`, and
qualification reads them through the run bundle's one typed reader
(`oer-hil-run-bundle`): its independence is the seals it verifies and the
files it hashes again at admission, not a second reader. It checks
`integrity.json`, every indexed file hash,
manifest/suite identity, current source applicability,
scenario outcome and repetition count. Markdown reports are not proof inputs.
A generated run directory without a manifest is incomplete mutable execution
state, ignored as evidence and counted as `hil-incomplete` in console
output and `evidence_inputs.hil.incomplete` in schema-4 JSON reports;
`hil-directories` counts every entry while `hil-bundles` counts only entries
that have published a manifest. A published run that fails validation (a
malformed manifest, a broken integrity or attempt seal, material that does not
match its seal) contributes no evidence: it is printed as `HIL-INVALID` with
its reason, listed in `evidence_inputs.hil.invalid` of JSON reports, and every
other run is still evaluated. A run whose manifest names another `RUN_SCHEMA`
is not read: such runs are counted per schema as one `HIL-UNSUPPORTED` line
each (`evidence_inputs.hil.unsupported` in JSON reports, their total in
`hil-invalid-unsupported`), and `cargo hil runs prune` removes those of an
older schema. Whole-invocation evidence requires a valid
integrity seal and a completed run; a running invocation alone supplies no
evidence.

When `attempts/` is present, the evaluator consumes its independently published
scenario seals instead of the aggregate suite. It validates their complete
material inventory, image identity, result and repetition-set boundary without
depending on completion of the enclosing campaign. Failed attempts are indexed
too. Temporary publications are not evidence, and a corrupt seal excludes its
run as `HIL-INVALID`; the aggregate suite is not used to replace a missing or
invalid attempt.
`hil-sealed-attempts` / `hil.sealed_attempts` counts these records. Each observation
in `hil_decisions` includes its completion seal's path and digest. The same
attempt is indexed once, not again when the enclosing run completes. Fixture
recovery and applicability to another build are separate from this completion.

### Evidence reports and absent directories

Use a JSON report for CI and downstream presentation:

```console
cargo qualification evaluate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --json-report target/qualification/wifi-sta.json
```

The console `INPUT` row and JSON `evidence_inputs` object expose how many
verification rows and HIL directories were observed, how many are incomplete
or current, together with repository dirty state and source applicability. `hil-qualifying` / `hil.qualifying` counts
eligible runs containing at least one passed scenario, not qualified products;
per-obligation decisions still enforce checks, repetitions and failures.
`hil-current-source-producer` / `hil.current_source_producer` counts direct
source bindings independently of dirty state.

An evidence directory that does not exist (the vendor evidence index, the
HIL evidence directory, or the checkout's `target/hil/runs` link before its
first run) holds no evidence: the evaluator prints
`EVIDENCE-DIR absent kind=… path=… shards=0` (`bundles=0` for runs), and
every obligation it would serve stays `missing`. A vendor evidence index
stale for the checkout holds none either and prints
`EVIDENCE-DIR stale kind=vendor-evidence path=… shards=0`. A path that an
existing tracked record names and that does not exist remains an error.

See the canonical
[verification and qualification contract](../docs/verification-and-qualification.md)
for evidence strength and the release workflow.

### Readiness drops

```console
cargo qualification readiness --out target/qualification/readiness \
  --base target/qualification/previous --drops target/qualification/drops.md
```

evaluates every program below `qualification/targets` into one JSON report
each (`<chip>-<program>.json`) in `--out`, and compares each with its report
in `--base`, the reports of an earlier evaluation. A capability's readiness
drops when one of its axes was terminal and is no longer, or when it was
proof-ready or ready and is no longer; a capability or program only one side
has is a reviewed catalog change, not a drop. Each drop prints as
`READINESS-DROP`, and with `--drops` they are written as a Markdown table
(the file is removed when nothing dropped). Nightly runs it after its
verification checks against the last good reports, the
`qualification-readiness` artifact of a `main` run: while a readiness issue
is open, those of the run its marker names, the last before the drop;
otherwise the previous night's. A drop opens a `kind:bug`,
`area:qualification`, `priority:P1` issue; each later night updates its
table, comments on a further drop and closes it once readiness is back.
Closing it by hand accepts the drop, and the next night's base is the
previous night's reports again. When the issue's base reports expired, its
marker becomes `expired-base-run`: the issue stays open, uncompared, and
the previous night's reports are the base. Only a complete evaluation is
published or reported; a base that fails to download fails the step
rather than being replaced.

### Host observer identity

A firmware snapshot does not identify the process that observed its behavior.
Run manifests include `runner.observer`: the SHA-256 of the running executable
and, by digest, the build record embedded in it (host package and local
dependency source hashes, compiler identity and build environment). On Linux the
executable digest is read through `/proc/self/exe`, so replacing the executable
while a run is active does not change its recorded subject.

The evaluator checks the current inputs selected by
[`observer-inputs.json`](../hil/schema/observer-inputs.json). Shared execution
and transport mechanisms are the common execution, link, stand and evidence
packages; each radio family's workloads and fixtures are a separate package, so
a change in one family never invalidates another family's observations. The
recorded build projects the selected direct dependency groups and their
transitive dependencies, including shared feature unification: edges come from
`cargo tree`, package features and unit profiles from Cargo's
`compiler-artifact` messages, and emitted build-script flags are bound with
output directories normalized. `cargo hil` builds the runner,
keeps a receipt under `target/hil/runners/` and launches a copy identified by
executable hash; a direct Cargo build has no receipt and cannot establish
current observer compatibility. The registry's `build.profile` selects the
required host profile (`debug` maps to Cargo's `dev`). An observation whose
observer differs from the current one in sources, dependencies, features,
compiler, profile or Cargo settings is excluded. The observer is assessed
only for an observation nothing else excludes, or of a run `hil-evidence
--run` or `--pending` names (a stale snapshot may still be recorded): an
observation already excluded, for example by its commit, is listed without
an observer exclusion, so evaluation time follows the current runs, not the
store.

## Functional AP availability

The [focused program](targets/esp32s31/wifi-ap-availability.toml) selects two
independent functional contracts. `station-ap-loss-recovery` requires the same
boot to observe generation-zero connection, beacon-loss disconnection,
generation-one connection, three fresh ICMP replies and a control response.
`station-initial-ap-absence` stops the AP before the first station start and
requires service admission, generation-zero no-candidate exhaustion after three
attempts without a connection, and a control response. A successful start admits
the station service; it does not report association. Each requires three complete
repetitions on the configured HT40 WPA2 fixture.

The existing `station-ap-absence` scenario instead removes an already connected
AP and observes generation-one exhaustion. It does not establish initial-start
behavior. The broader `interrupt-recovery`, `async-deadlines` and
`timeout-error-recovery` capabilities retain their own vendor contracts and
hardware gaps. Functional scope does not prove all fault, cancellation, timing,
throughput or radio-retirement paths.

```console
cargo qualification status --manifest qualification/targets/esp32s31/wifi-ap-availability.toml --details
cargo qualification next --manifest qualification/targets/esp32s31/wifi-ap-availability.toml
```
