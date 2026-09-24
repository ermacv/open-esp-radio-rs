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

Saved HIL evidence uses the current observer descriptor prepared by `cargo hil`
or `cargo xtask hil-observer`. Evaluation reads it once and never builds or
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
Neither command changes a gate outcome or fails just because work remains.
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
measurement methods, applicability review, incomplete attempts and unresolved
failures. Declarations-only mode asks to inspect evidence before deciding to
repeat an experiment. Existing gaps may carry a reviewed work classification;
unclassified HIL gaps require review rather than interpretation of their names.
Actions are deterministic candidates with reasons, not a priority ranking or an
automatic execution plan. Choose the goal first, then inspect its owners and
checks. Current captured-input research is described in the
[Blobray task map](../tools/blobray/README.md#choose-a-task). The selected
verification projects also retain legacy `project status` and `project research
next` contracts; these commands are unavailable in current `cargo blobray` and
are not replaced by the declarations-only qualification map.

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

The initial focused links cover Wi-Fi DMA, calibration/RFPLL and peripheral BLE
maintenance/security. Other declarations remain visible with missing links;
missing navigation is not an absent implementation or an unexplored chip.

The focused product programs are Wi-Fi STA and BLE peripheral/ACL, followed by
secure peripheral GATT. Their definitions share canonical capability declarations:

| Program | Required product boundary |
| --- | --- |
| [Wi-Fi maintenance continuity](targets/esp32s31/wifi-maintenance-continuity.toml) | One HT20 station epoch across combined calibration and fresh post-maintenance ICMP exchange; independent of RF-quality and throughput-ceiling claims |
| [Wi-Fi AP availability](targets/esp32s31/wifi-ap-availability.toml) | Controlled AP-loss recovery with fresh IP exchange, and bounded initial no-candidate exhaustion |
| [Wi-Fi STA](targets/esp32s31/wifi-sta.toml) | Station association/WPA2, datapath, recovery and PHY/power lifecycle |
| [BLE peripheral/ACL](targets/esp32s31/bluetooth-peripheral-acl.toml) | One LE 1M connection, bidirectional ACL, recovery and terminal powered release |
| [Secure peripheral GATT](targets/esp32s31/bluetooth-secure-gatt.toml) | The complete peripheral/ACL boundary plus encrypted traffic, Secure Connections, protected ATT access and bonded reconnect |

The [full Bluetooth LE program](targets/esp32s31/bluetooth-le.toml) retains its
wider role and feature requirements. [IEEE 802.15.4](targets/esp32s31/ieee802154.toml)
is an independent program. Product selection does not change source coverage or
make any hardware evidence current.


## Capability catalogs and program resolution

Canonical capability declarations may live below `catalog/`. A qualification
program names one or more catalog files with `catalogs` and selects stable IDs
with `catalog-capabilities`. Resolution adds the selected declaration and its
catalog-owned dependency closure to the program before the schema-4 evaluator
runs. Inline capabilities remain supported while domains migrate; a capability
ID cannot be declared both inline and in a loaded catalog.

Catalogs declare shared inputs with `imports = ["qualification/catalog/…"]`.
Paths are relative to the repository root. Imports resolve transitively; a shared
catalog is loaded once even when also explicitly selected. Missing inputs,
import cycles and repeated imports within one catalog are errors. Every imported
source retains its own identity and hash in the rendered/evaluated provenance.
Importing a catalog does not select all of its capabilities for a product.

A program normally supplies an exact `required-capabilities` list. Alternatively,
`required-capabilities-from = "catalog-closure"` explicitly derives the full set
from `catalog-capabilities` and transitive dependencies. That mode cannot mix
explicit required IDs or inline capabilities. Missing policy is not permission
to derive a set: the existing exact-set checks still apply. Added dependencies
become mandatory automatically in closure mode and retain all their evidence
requirements. The evaluator report lists the resolved membership and provenance.

Every catalog declaration keeps the existing reviewed/evidence axes and may
carry the existing source contracts. Its required `catalog-scope` identifies
the chip, role, PHY, security set, composition, native/lower/composed level,
activation boundary and limitations. This metadata is validated and rendered,
but it is not a readiness axis.

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

Static render writes `project-status.md`, `domain-inventory.md`,
`capability-catalog.md`, and `migration-map.md`. Manifest render additionally
writes `program-status.md` and `program-inventory.md` after evaluation. The program view records repository
commit/dirty state and configured evidence provenance; a catalog or manifest
hash is source identity, never firmware identity. All outputs are ignored
views, not another readiness decision or tracked snapshot.

`cargo xtask check docs` invokes the static catalog owner for all catalogs and
program selections, and renders the complete catalog set in both input orders
to check deterministic presentation. It uses `catalog check --catalog`,
`catalog check --manifest`, and `catalog render --catalog`; it does not use
manifest rendering, `validate`, `evaluate`, or `gate`, and it does not read
vendor evidence or HIL runs. Its ignored static views live below
`target/docs/static/catalogs/` for the ordinary command (`target/docs/gate/catalogs/`
for `--full`) and carry no readiness verdict.

The Bluetooth LE program includes legacy and extended roles, connected PHY and
control procedures, security/privacy, periodic advertising and PAwR, Direction
Finding, ISO in both connected and broadcast roles, LE Audio and the named LE
Host integrations. It also requires capacity admission, power lifecycle and
coexistence. Each new scope retains explicit incomplete axes until its own
production composition and evidence exist. The chip's
[feature inventory](../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md#qualification-scope-mapping)
maps these requirements to current source boundaries.

Validate each program from the repository root:

```console
cargo qualification validate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/bluetooth-le.toml

cargo qualification validate \
  --manifest qualification/targets/esp32s31/ieee802154.toml
```

Three commands have deliberately different contracts:

- `validate` rejects malformed manifests, unsafe references, invalid
  dependency graphs, mismatched verification inputs and corrupt HIL bundles;
  an incomplete target is still a valid development state;
- `evaluate` emits the same derived verdict and optionally a complete JSON
  report through `--json-report PATH`;
- `gate` returns non-zero unless every required capability and dependency is
  ready.

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
assessment, or `gate --manifest PATH` when every selected capability must be ready.
The catalog-check summary counts all loaded catalog declarations, including
unselected ones; the evaluator report describes only the resolved product set.

## Execution selection

`cargo qualification plan --manifest <program> [--capability <id>]` emits a
read-only JSON selection from the same HIL decisions as `status` and `gate`.
Each obligation explains `satisfied`, `run`, `review` or `investigate` and binds
its property scope. It performs no build, test or hardware action. The HIL
runner consumes this through `cargo hil plan --qualification <program>` and
refreshes it before resuming; already satisfied obligations do not rerun.
Unknown impact requests review rather than silently inheriting success. A focused
capability plan includes only its own obligations and necessary controls;
prerequisite capabilities remain context, not additional execution requests.

## Declared and derived axes

Every capability has five independent axes:

- `implementation` is the reviewed declaration `complete` or `incomplete`;
- `host` is the reviewed declaration `covered` or `incomplete`. The evaluator
  checks consistency with declared gaps; it does not find or run Rust tests.
  Workspace testing remains a separate repository check;
- `vendor` is derived from Blobray's compact evidence index with independently completed suites. Only a
  fresh, baseline-accepted, release-eligible `production-trace` for every
  declared root with matching production source hashes can qualify the axis;
- `hil` is derived from immutable schema-2 HIL bundles. Every required scenario
  must pass with enough repetitions in an independently sealed attempt or run,
  bound to the current source inputs or admitted by an explicit property/build
  applicability review;
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

## Evidence ownership

```text
reviewed capability declarations ─┐
Blobray vendor evidence index ─────┼─> qualification evaluator ─> JSON/verdict
sealed HIL run bundles ────────────┘
```

Blobray and the HIL runner never decide product readiness. Blobray owns vendor
comparison truth; the HIL runner owns hardware execution truth; qualification
maps both into the declared capability graph.

Capability dependencies describe readiness, not instructions to re-execute
every prerequisite scenario. Controlled HIL experiments instead declare their
execution control in the scenario catalog. The independent evaluator validates
that only the supported intervention differs, and requires a passing control
in the same eligible run with the same number of repetitions. An older baseline
or a result from another PHY/profile cannot substitute for that control. The
pair establishes its absolute checks, not a relative non-regression verdict.

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

JSON `hil_decisions` explains each complete obligation: its applicability policy,
completion boundary, status, selected evidence, and every observed scenario's
exclusion reasons and unmet requirements. Console `HIL-OBLIGATION` rows summarize
these decisions. Completed scenarios remain candidates when an unrelated
scenario fails in the same sealed suite. A failed current scenario or repetition
cannot be hidden by selecting a later PASS. The status is `unresolved-failure`
until an explicit [failure disposition](evidence-reviews.md#resolving-a-failure)
binds its resolution or explains why it does not apply. `broken`, `blocked`, `skipped` and `interrupted` alone are
neither PASS nor a product failure, but they do not erase a different repetition's
explicit failure. Controlled experiments also retain their control's decision.

The supported completion boundary remains one complete scenario repetition set.
Named checks are not independently sealed lifecycle phases: a successful early
check in a failed lifecycle cannot qualify that lifecycle or become independent
evidence merely by selecting its name. Reassessment against a weaker numeric
criterion likewise does not turn a failed lifecycle into PASS. A completed
observation below the requested criterion is an unresolved failure; an absent
measurement is missing evidence, not an invented failure.

The initial named checks cover station UDP RX rates, configured maximum RX
silence, a validated maintenance transaction, and absence of station lifecycle
change through that transaction's traffic session. Semantic checks are recorded
only when attempted. They do not establish RF quality, a qualified PHY execution
bound, or relative performance non-regression. Default applicability is
`current-source-composition`: verified current source binding and no unbound
replay. Legacy commit-only provenance still requires a matching clean commit. A `source-snapshot` build can establish this direct binding:
the evaluator independently verifies snapshot identities, every archived file,
and the complete current tracked and nonignored untracked file set, bytes and executable modes. It also
checks the current lockfile and local override pins. No self-review is needed
for a matching snapshot, regardless of dirty state or commit identity. Optional [reviewed applicability](evidence-reviews.md) admits an
original complete observation for a specific capability/property and destination
build after validating an explicit engineering conclusion and its bindings.
Original outcomes and exclusions remain visible; a commit change or another PASS
does not establish that a failure was resolved.

The HIL runner writes bundles below `target/hil/<target>/runs/<run-id>/`.
Qualification independently checks `integrity.json`, every indexed file hash,
manifest/suite identity, current source applicability,
scenario outcome and repetition count. Markdown reports are not proof inputs.
A generated run directory without a manifest is incomplete mutable execution
state, ignored as evidence and counted as `hil-incomplete` in console
output and `evidence_inputs.hil.incomplete` in schema-4 JSON reports;
`hil-directories` counts every entry while `hil-bundles` counts only entries
that have published a manifest. An existing malformed manifest still fails
validation. Whole-invocation evidence requires a valid integrity seal and a
completed run; a running invocation alone supplies no evidence.

When `attempts/` is present, the evaluator consumes its independently published
scenario seals instead of the aggregate suite. It validates their complete
material inventory, image identity, result and repetition-set boundary without
depending on completion of the enclosing campaign. Failed attempts are indexed
too. Temporary publications are not evidence, and a corrupt seal fails closed;
the aggregate suite is not used to replace a missing or invalid attempt.
`hil-sealed-attempts` / `hil.sealed_attempts` counts these records. Each observation
in `hil_decisions` includes its completion seal's path and digest. The same
attempt is indexed once, not again when the enclosing run completes. Fixture
recovery and applicability to another build are separate from this completion.

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
per-obligation decisions still enforce checks, repetitions, controls and failures.
`hil-current-source-producer` / `hil.current_source_producer` counts direct
source bindings independently of dirty state.

See the canonical
[verification and qualification contract](../docs/verification-and-qualification.md)
for evidence strength and the release workflow.

## Functional station maintenance scope

`station-phy-maintenance-continuity` is a separately selected composed capability.
Its reviewed source facts link the calibration, IRQ and bounded-wait owners;
its retained-datapath contract links the RX and station owners. These are code
and hardware-contract dependencies. The focused program does not assert the
broader `interrupt-recovery`, `async-deadlines`, DMA-mode or RF-calibration
qualification promises. Those records retain their own requirements and gaps.

The [scenario](../hil/scenarios/ieee80211/station/station-phy-maintenance-continuity.toml)
requires three complete repetitions: UDP progress before combined maintenance,
a successful physical transaction, fresh ICMP requests/replies afterwards, and
no station-epoch change. It uses HT20, a 2 Mbit/s offered stream and a 100 kbit/s
liveness floor. Loss is allowed. Fresh ICMP exchange establishes bidirectional
IP reachability after maintenance, not resumed UDP throughput. RF quality,
physical execution time, high-load continuity and other PHY profiles remain
separate claims under `runtime-phy-calibration` and its existing scenarios.

```console
cargo qualification status --manifest qualification/targets/esp32s31/wifi-maintenance-continuity.toml --details
cargo qualification next --manifest qualification/targets/esp32s31/wifi-maintenance-continuity.toml
```

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
