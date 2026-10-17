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
the vendor evidence shard format, validation, currency and reader/writer that
verification producers write and the evaluator reads. HIL evidence is the
sealed runs of the [evidence archive](#evidence-ownership).

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
  --capability peripheral-acl \
  --json-report target/qualification/peripheral-acl-map.json

cargo qualification next \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --capability runtime-phy-calibration
```

Read existing evidence for a selected program, without running any checks
(no program exists since #770; `<chip>/<program>` names one when it does);
HIL evidence comes only from `--archive`, a verified copy of the pinned
evidence archive version (`cargo hil archive fetch --out DIR`, into a
directory that does not exist yet: it never writes over an earlier copy):

```console
cargo qualification status \
  --manifest qualification/targets/<chip>/<program>.toml \
  --archive target/hil/evidence-archive \
  --json-report target/qualification/<program>.json

cargo qualification next \
  --manifest qualification/targets/<chip>/<program>.toml \
  --capability <capability>
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
selected map. Program mode validates the evidence archive copy it is given and
can take longer than the declarations-only view; it does not regenerate
evidence. Without `--archive` it reads no HIL run, says so
(`INPUT hil-archive=none`), and no HIL obligation is satisfied.

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

Focused links cover calibration/RFPLL and peripheral BLE
maintenance/security. Other declarations remain visible with missing links;
missing navigation is not an absent implementation or an unexplored chip.

No qualification program exists: the Bluetooth LE, peripheral/ACL, secure
peripheral GATT and IEEE 802.15.4 programs went with the orchestration in
#770 (tag `legacy-radio-2026-10-16`), and no Wi-Fi program exists. The
catalogs keep their capability declarations; selecting none changes no
source coverage and makes no hardware evidence current.


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

The ESP32-S31 [Wi-Fi/PHY catalog](catalog/esp32s31/wifi-phy.toml) owns the
shared-PHY capabilities and inventory and the Bluetooth source facts the
Bluetooth catalog imports; it holds no Wi-Fi declaration and no program
selects it directly. Source-facet
inventory status and native/lower/composed level are separate from evaluator
axes; unselected inventory entries have no readiness value.

The [Bluetooth catalog](catalog/esp32s31/bluetooth.toml) owns all 64 Bluetooth
LE capability declarations and the wider LE, Classic and Host-only source
inventory. It imports the Wi-Fi/PHY and
[coexistence](catalog/esp32s31/coex.toml) catalogs to reuse the exact
Bluetooth initial handoff and idle and periodic PHY maintenance facts, alongside
the diagnostic `coex-timer-validation-bridge`. Those facts are implemented lower
primitives that no composition runs since #770 (tag `legacy-radio-2026-10-16`);
the powered-release and same-storage restart facts went with the Bluetooth
composition, and #771 brings a composer back. Qualified timing/RF bounds and
encrypted recovery remain separate limits. Implemented subsets do not promote
the incomplete common-PHY, powered teardown or coexistence lifetimes.

The coexistence catalog and the
[whole-radio catalog](catalog/esp32s31/whole-radio.toml) are source-inventory
owners, not qualification programs. They add no capability declaration or
readiness authority. Whole-radio views load the Wi-Fi/PHY and coexistence
catalogs explicitly because they project facts owned there; a missing fact
owner is an error rather than an omitted row.

The [IEEE 802.15.4 catalog](catalog/esp32s31/ieee802154.toml) owns five
radio/MAC capability declarations, which no program selects since #770, and
the wider PHY, CCA, dataplane, filtering, timing, security, power, coexistence
and Host-only source inventory. Its
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
  --manifest qualification/targets/<chip>/<program>.toml \
  --out target/qualification/catalog/<program>

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/wifi-phy.toml \
  --catalog qualification/catalog/esp32s31/coex.toml \
  --catalog qualification/catalog/esp32s31/bluetooth.toml

cargo qualification catalog render \
  --manifest qualification/targets/<chip>/<program>.toml \
  --out target/qualification/catalog/<program>

cargo qualification catalog check \
  --catalog qualification/catalog/esp32s31/ieee802154.toml

cargo qualification catalog render \
  --manifest qualification/targets/<chip>/<program>.toml \
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



### Validate and evaluate a program

Validate each program from the repository root:

```console
cargo qualification validate \
  --manifest qualification/targets/<chip>/<program>.toml
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

No program selects these products since #770 (tag
`legacy-radio-2026-10-16`); the catalog keeps their criteria and gaps,
including the [periodic maintenance contract](catalog/esp32s31/wifi-phy.toml),
and no HIL scenario serves them.

Check a selected program without scanning the evidence archive:

```console
cargo qualification catalog check --manifest qualification/targets/<chip>/<program>.toml
```

Use `evaluate --manifest PATH --archive DIR --json-report PATH` for a current
evidence-backed assessment; its verdict says which selected capabilities are
ready. On the stand, `verdict --run PATH` judges one run bundle against the
HIL requirements of every program with the same evaluator; its output is
marked not published and not evidence, and no other command reads it.
The catalog-check summary counts all loaded catalog declarations, including
unselected ones; the evaluator report describes only the resolved product set.

## Execution selection

`cargo qualification plan --manifest <program> [--archive DIR] [--capability <id>]`
emits a read-only JSON selection from the same HIL decisions as `status` and
`evaluate`. Without `--archive` it reads no run (`INPUT hil-archive=none` on
stderr), so no obligation is `satisfied`: a qualification scenario the
firmware supports is `run`, an unsupported or investigation one keeps its
state.
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
  checks consistency with declared gaps; it does not find or run Rust tests;
- `vendor` is derived from the vendor evidence index of the program's chip,
  derived data in `target/verification/<chip>/evidence` that
  `cargo verification evidence --chip <chip>` computes for the checkout. It
  qualifies only when every declared root has an evidence reference with a
  MATCH entry in that index and no source-only anchor remains. One stale
  shard (a recorded source changed or gone, or another format schema) voids
  the whole index;
- `hil` is derived from the sealed schema-2 HIL run bundles of the pinned
  evidence archive version (`--archive`). Every required scenario
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
to describe its hardware-facing implementation paths. Cold calibration/cache,
Bluetooth DTM and PHY lifetime, coexistence, and IEEE 802.15.4 lower operation
contracts make implemented subsets visible without promoting their broader parents.
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
facts or catalog capabilities that item owns:

```rust
/// Fixed pool of role instances of one kind.
// CAPABILITY: bluetooth-controller-sram-ownership
pub struct SchedulerRolePool { /* ... */ }
```

`cargo qualification catalog anchors --catalog PATH ...` scans every Rust file
of the repository and checks the anchors against the selected catalogs; pass
all of them, because an anchor may name an entry of any catalog. The rules follow the declared source status:

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
reviewed capability declarations ───────┐
vendor evidence shards ──────────────────┼─> qualification evaluator ─> JSON/verdict
sealed HIL runs of the pinned archive ───┘
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
referenced by no program. A requirement on an
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

### Measurement bounds

A scenario defines its experiment and its own success; its named checks
decide whether the run passed. Qualification additionally bounds what the run
observed: a HIL requirement's `bounds` limit recorded measurement values the
producer publishes without a verdict (owner decision on #363). Changing a
bound touches no file of a run's closure, so collected evidence stays current
and is judged again. The thresholds the existing named checks derive from
scenario documents move to this mechanism in #554.

The secure GATT requirement, as its catalog entry declared it until the entry
went in #770 (tag `legacy-radio-2026-10-16`):

```toml
hil-requirements = [
  { scenario = "bluetooth-trouble-secure-gatt", minimum-repetitions = 3, bounds = [
    { ratio = { numerator = { name = "connections-not-established", unit = "count", semantics = 1, better = "lower" },
                denominator = { name = "connection-attempts", unit = "count", semantics = 1 } },
      over = "current-runs", at-most-basis-points = 2000, minimum-denominator = 45 },
  ] },
]
```

- **Quantity.** `measurement` bounds one measurement in each repetition, in
  its own unit (`at-most` or `at-least`): its sum would grow with every run;
  `ratio` bounds a numerator over a denominator in
  basis points (`at-most-basis-points` or `at-least-basis-points`, at most
  10000) and needs a positive `minimum-denominator`.
- **Identity.** Each measurement is named with its unit, semantics version and
  declared `better`; a repetition that lacks it, records it twice or with
  another identity leaves it unavailable. The bounded measurement (a ratio's
  numerator) must declare the direction its limit implies: `lower` for an
  upper limit, `higher` for a lower one.
- **Scope.** A bound is judged on every current applicable run of its
  scenario together, never on a selected one: each run that is otherwise
  complete (passed, enough repetitions, its named checks met, every bound
  measurement recorded) joins, and runs whose sources changed drop out by
  themselves. `over = "repetition"` holds in each of their repetitions;
  `over = "current-runs"` holds on the sums over all of them, so a noisy run
  averages out and rerunning until a lucky run proves nothing (owner decision
  on #363).
- **Decision.** A run that lacks a bound measurement, records it twice or
  with another identity cannot join (`bound-measurements-unavailable`). A sum
  below its minimum denominator, or outside the bound, leaves the obligation
  missing; more current runs move it, and none is picked. Only a bound met by
  all joined runs satisfies it, never an invented pass. `hil_decisions` lists
  each bound and `bound_findings`, the joined runs with the values judged; the
  evidence reference names those runs and the bounds they met.

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

No named check has an evaluator contract now: the Wi-Fi contracts (station
UDP RX rates, RX silence, maintenance transactions) were deleted with their
scenarios (#762), and typed criteria replace the mechanism in #761. Semantic
checks are recorded only when attempted. They do not establish RF quality, a qualified PHY execution
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

### Run bundles and the evidence archive

A run bundle stays in ignored output, in the run store every checkout of the
user shares (see [find and compare runs](../hil/host/runs.md#find-and-compare-runs)),
and qualifies only for a checkout whose sources it binds. Qualification
evidence never comes from the run store (`verdict --run` reads a run there,
but its result is no evidence). Its HIL evidence is the runs a decision cites, kept in
the evidence archive ([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761)
Q7, [`oer-hil-evidence-archive`](../hil/host/evidence-archive/README.md)):

1. a decision, a measured bound or the stand profile cites a run by its
   reference, its ID and the SHA-256 of its seal (`integrity.json`);
2. `cargo hil archive pack` writes an immutable version of exactly the cited
   runs, named `evidence-<SHA-256 of its manifest>`, with the observer
   builds and source objects they name: the runs the pinned version already
   holds from its downloaded, verified copy, so a version keeps them after
   the run store pruned them, the others from the run store (owner decision
   [6231504542](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6231504542)).
   `cargo hil archive ref` takes a run the pinned version holds and the run
   store no longer has from the pin's manifest. Before `pack` and `publish`
   create anything, `cargo qualification check references --candidate FILE`
   holds the version's runs to rules 1 and 2 against the repository's
   citations, so a release `check references` would reject is never made.
   The run store keeps the version's runs pinned until it is published:
   `cargo hil archive ref` pins the runs it names at once for its own
   selection, `pack` hands that selection's pins to the version, and `cargo
   hil archive unpin VERSION|SELECTION` drops the pins of a version packed but
   never published or of a selection;
3. `cargo hil archive publish` releases it in the private archive
   repository, never overwriting or deleting one; a draft whose upload was
   interrupted is deleted by hand, which keeps rule 5 because a draft is not
   yet in the archive. It then writes the pin `evidence/archive.toml` in
   `qualification/`, which repeats the manifest;
4. `cargo qualification check references` checks
   offline that the pin hashes to its version, that every cited run is in it
   with the same digest (rule 2) and that it holds no run nothing cites
   (rule 1) (owner decision
   [6225000924](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6225000924));
   it reads no runs;
5. `cargo hil archive fetch --out DIR` downloads the pinned version where
   the archive is reachable, and `status`, `next`, `plan`, `readiness` and
   `evaluate --archive DIR` read only that copy, verified against the pin
   when opened, with the checkout's current observer descriptor
   (`target/hil/current-observer.json`, which `cargo hil observer` prepares)
   (owner decision
   [6225298347](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6225298347)).

No tracked record cites a run yet. The stand profile's type
(`oer-hil-model`) already holds each span's `evidence` references, but no
profile is tracked; the PR that tracks the first one adds its reader to
`check references`. A qualification decision or a measured bound gets its
citing field with the first record that cites, in one PR together with its
reader in `check references` and its test (owner decision
[6226361520](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6226361520)). Until then
catalogs, programs and the stand profile refuse every field their reader does
not know, so no reference can be written past it, and `check references`
prints how many citations it checked (`citations=0`).

On the stand, `cargo qualification verdict --run PATH` judges one run bundle
against the HIL requirements of every program with the same evaluator. Its
output says the run is not published and not evidence, and no other command
reads it. Its exit status is 1 when the run does not meet a requirement it
observed, observed none a program requires, is of no program's chip or its
bundle is not valid (the reason is printed), 2 for a usage error, 3 when the
command itself failed and 130 on Ctrl-C.

The runner's observer proof names the build its executable was compiled from,
which most runs share. A run's manifest names that build by
digest, and the build is stored once in `observers/<build_sha256>.json` next to
the `runs` directory of a store or an archive copy, as the exact bytes the
digest is computed over.
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
and status-change time (`OER_QUALIFICATION_HASH_CACHE=0` disables it).
The runner records, per image, the repository files its build read in the
bundle's `firmware/<image>/source-inputs.json` (schema 2): the sources Cargo's
dep-info lists for the runtime and bootstrap binaries, the repository inputs
their build scripts declare (linker scripts), each compiled package's manifest
and build script, both firmware workspaces' manifests (release profile and
`[patch]`) and lock files, the Cargo configuration and toolchain files above
them, the workspace manifest the compiled packages inherit from, the stack
policy and partition table, and the sources of the image builder, packer and
memory auditor. Firmware builds drop inherited `CARGO_PROFILE_*`,
`CARGO_BUILD_*` and `CARGO_TARGET_*` variables except the job count and
target directory. `INPUT` reports `hil-archive`, the version read or `none`.

### Bundle validation and attempt seals

The HIL runner writes bundles into the run store (`target/hil/runs/<run-id>/`
links to it); qualification reads those of an archive copy, or the one
`verdict` names, through the run bundle's one typed reader
(`oer-hil-run-bundle`): its independence is the seals it verifies and the
files it hashes again at admission, not a second reader. It checks
`integrity.json`, every indexed file hash,
manifest/suite identity, current source applicability,
scenario outcome and repetition count. Markdown reports are not proof inputs.
A generated run directory without a manifest is incomplete mutable execution
state, ignored as evidence and counted as `hil-incomplete` in console
output and `evidence_inputs.hil.incomplete` in schema-5 JSON reports;
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

Use a JSON report for downstream presentation:

```console
cargo qualification evaluate \
  --manifest qualification/targets/<chip>/<program>.toml \
  --archive target/hil/evidence-archive \
  --json-report target/qualification/<program>.json
```

Without `--archive` the HIL counts are zero: no run is read.

The console `INPUT` row and JSON `evidence_inputs` object expose how many
verification rows and HIL directories were observed, how many are incomplete
or current, together with repository dirty state and source applicability. `hil-qualifying` / `hil.qualifying` counts
eligible runs containing at least one passed scenario, not qualified products;
per-obligation decisions still enforce checks, repetitions and failures.
`hil-current-source-producer` / `hil.current_source_producer` counts direct
source bindings independently of dirty state.

A vendor evidence index that does not exist holds no evidence: the evaluator
prints `EVIDENCE-DIR absent kind=vendor-evidence path=… shards=0`, and every
obligation it would serve stays `missing`; an evaluation without `--archive`
satisfies no HIL obligation. A vendor evidence index
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

evaluates every program (`qualification/targets/<chip>/<program>.toml`) into
one JSON report each (`<chip>-<program>.json`) in `--out`, and compares each
with its report in `--base`, the reports of an earlier evaluation. A
capability's readiness drops when one of its axes was terminal and is no
longer, or when it was proof-ready or ready and is no longer; a capability or
program only one side has is a reviewed catalog change, not a drop. Each drop
prints as `READINESS-DROP`, and with `--drops` they are written as a Markdown
table (the file is removed when nothing dropped).

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
only for an observation nothing else excludes: an observation already
excluded, for example by its commit, is listed without an observer exclusion,
so evaluation time follows the current runs, not every run read.
