# Verification and qualification contract

This document defines the boundary between production code, host contracts,
Blobray verification, HIL execution and product qualification.

The everyday engineering map is `cargo qualification status|next`. It connects
reviewed knowledge references, implementation owners, checks, saved observations
and next-work reasons. Catalog mode reads declarations only; program mode uses
the same independent evaluator described below. Neither mode emits a global
project gate or runs missing checks. Research results remain useful before an
application path exists, and incomplete programs are ordinary development states.

## Authorities

| Source | Question it answers | May decide readiness? |
| --- | --- | --- |
| Reviewed implementation/host/async declarations | What reviewed state does the capability program declare, consistently with its gaps? | Required declarations, not test-run evidence |
| Blobray `production-trace` | Does concrete vendor execution match the exact compiled production entry under the declared bounded contract? | Supplies vendor evidence only |
| Blobray `shared-core` or static analysis | Does supporting code or a model agree? | No |
| HIL sealed run | Did the current production composition pass the declared scenario on hardware? | Supplies HIL evidence only |
| Qualification v4 | Are all required axes and dependencies closed by acceptable current evidence? | **Sole readiness authority** |

No evidence producer imports product-readiness policy. Qualification consumes
their typed outputs and derives the verdict.

Schema 4 declares implementation, host and async states explicitly. The
qualification evaluator checks their consistency with gaps and dependencies;
it does not discover production owners or host tests by source text and does
not execute those tests. Vendor and HIL states are independently derived.
Workspace tests and review are responsible for the declared source states.
The evaluator lives in `qualification/evaluator`; its CLI is
`cargo qualification validate|evaluate|gate`.

Schema-4 programs may select canonical capability records from schema-2 files
under `qualification/catalog/`. Resolution includes the selected stable IDs
and their catalog-owned dependency closure before the same evaluator applies
the required-set, dependency, declaration and evidence rules. Programs either
provide an exact `required-capabilities` list or explicitly select
`required-capabilities-from = "catalog-closure"`; the latter derives every
required ID from catalog roots and dependencies and rejects explicit required-ID
additions. A program declares no capability itself. Missing policy does not enable this mode. Catalog structure
and generated inventories are not evidence and cannot promote an axis or a
readiness verdict. Catalog scope metadata records chip, role, PHY, security,
composition level, activation boundary and limitations separately from those
axes. The same catalog may contain a wider source inventory whose source
status is not a readiness result; static catalog checking validates all
declarations without consuming vendor evidence indexes or HIL runs.

When two views describe the exact same source scope, a catalog source fact owns
its status, level, limits and source contract. Inventory projections and
qualification declarations reference that fact explicitly and cannot override
it. This reference is not a readiness dependency: narrower implemented facts
do not close broader incomplete capabilities, and related scopes remain
separate assertions.

A source-only catalog may own such a fact without creating a qualification
target. Every static or program input that consumes the fact names its owner
catalog explicitly. For example, the coexistence catalog owns the diagnostic
timer bridge used by the Bluetooth declaration and the coexistence and
whole-radio views. Its validation-only MMIO composition remains distinct from
a production RF grant or joint-radio lifecycle.

## Agreement with feature inventories

`FEATURES.md` and qualification describe the same implementation. For an
identical scope, an `IMPLEMENTED` feature must agree with
`implementation = "complete"`; missing vendor/HIL evidence may prevent readiness
but is not an implementation gap. Conversely, a missing production owner must
not be hidden by a complete lower register or protocol primitive.

Qualification roots can cover a broader lifetime than individual feature rows.
In that case, name the additional requirements in the root scope and gaps, and
use `source-contracts` for the implemented subset. Link the feature inventory
to those capability IDs so that an incomplete parent is not read as an absent
child operation. `PARTIAL`, `FAIL-CLOSED` and `ABSENT` must retain their exact
missing or rejected operations; none means the hardware cannot support them.
`HOST-ONLY` rows do not create Controller qualification obligations.

Changes to source coverage require review of both the inventory and applicable
qualification scopes, source contracts and implementation gaps. Preserve
vendor/HIL requirements and provenance; updating an implementation description
does not supply evidence. A feature outside the target's scope has no readiness
claim from that target. The evaluator validates structured declarations and
references, not semantic agreement with Markdown; that agreement remains a
source-review obligation.

[Code anchors](../qualification/README.md#code-anchors) bind each catalog
entry to the code that owns it: an implemented, partial, fail-closed or
diagnostic entry must be anchored, an absent one must not be, and
a capability declared complete must be anchored itself or through implemented
source facts. Removing the owning code therefore fails the check until the
catalog states the new status, and a claim cannot be added without naming its
owner.

## Vendor verification path

Vendor comparison proves selected hardware contracts: register effects,
hardware-consumed SRAM, IRQ/DMA state and physical transition preconditions.
Protocol decisions, retry/rate policy, futures, Rust resource ownership and
runtime reconstruction use their own contracts, host tests and HIL. Research
into vendor software belongs in development knowledge links; an informational
comparison does not make software equivalence a qualification requirement.
Mixed capabilities retain vendor gaps only for their hardware operations and
state that boundary in their scope limitations. WPA2 depends separately on
hardware key publication, crypto activation and retirement; its handshake and
replay/deadline policy do not require vendor equivalence.

Vendor evidence comes from the typed vendor scenarios in
[`verification/esp32s31/scenarios`](../verification/esp32s31/scenarios).
The native evidence index is the directory
`verification/esp32s31/evidence/scenarios`
([schema](../verification/schema/scenario-evidence.rs)), one shard per
scenario named after it. `vendor-scenario <scenario> --index <directory>`
writes that scenario's shard when it passes with no unmet obligation;
`vendor-scenario all --index <directory>` runs every scenario under one budget
and, only when all of them pass, writes every shard. A scenario run rewrites
only its own shard, so work on different scenarios does not touch the same
file. Each shard records:

- one entry per claimed vendor root: scenario (`suite`), vendor source
  (`archive` or `rom`), root symbol, compiled production entry, the number of
  compared cases and the digests of the reviewed content of the effect
  contracts and output projections those comparisons selected, which leave out
  the endpoints naming one run's imported revision so that unchanged sources
  reproduce every shard byte for byte. Contracts and projections are typed
  values in the scenario code, reviewed through git. A claim exists only when
  every case of the run comparing that exact root/entry pair is MATCH; an
  unsupported claim fails the run instead of being written. The run keeps only
  these results, not executions that could be reopened. Each entry also
  carries the vendor coverage of the root's closure over those comparisons
  (Blobray in-process coverage): basic blocks and branch directions reached out of
  all, and how many uncovered ones a reviewed decision excludes or remain
  untriaged. It also counts the production hardware source lines (the
  ESP32-S31 PHY, HAL, PAC and MAC driver crates) those
  comparisons executed, the lines a compared observation depends on (Blobray
  observation dependence), and how many of the rest a reviewed decision
  covers or remain untriaged. Finally it counts the persistent vendor bytes
  those comparisons wrote (Blobray written ranges), the bytes every writing
  case compares through a projection field, memory pair or write timeline,
  and how many of the rest a reviewed decision covers or remain untriaged;
- every untriaged uncovered location of the scenario's claimed closures that
  no claim of the scenario whose closure contains its function reaches, by
  vendor function, offset
  and kind (block, taken or fallthrough direction, or a transfer site the
  closure leaves open: an indirect transfer followed only to its executed
  targets, so that other targets such as further jump-table arms are outside
  the closure, or an unresolved transfer);
- the functions of the scenario's claimed closures;
- every production hardware line the scenario executed without its compared
  observations depending on it and that no reviewed decision covers, and every
  line they do depend on, by path and line;
- every persistent vendor byte a claim's cases write without comparing it and
  no reviewed decision covers, coalesced by data symbol and offset; a byte
  outside every sized data symbol is named by the address of the nearest
  symbol below it;
- SHA-256 identities of every input the scenario captured, including the
  production probe ELF;
- directory digests of every source the verdicts depend on: the resolved
  path-dependency closure of the probe ELF the scenario compared (production
  crates and probes), the scenario package, the shared schema and the Blobray
  engine.

Two views span scenarios and are derived from all shards together: a location
is untriaged when some scenario lists it and every scenario whose closures
contain its function lists it too, and a line is unobserved when some
scenario lists it and no scenario observes it. The index carries identities
and verdicts only, never vendor bytes.
Qualification reads the index directory named by the program's
`[verification] evidence-index` (catalogs name it in `[validation]
evidence-index`), checks each shard's schema, producer command, chip target
and scenario name, requires every entry to belong to its shard's scenario and
to be a MATCH
claim with compared cases whose coverage accounts for every uncovered location,
whose observation counts account for every executed line and whose state
counts account for every written byte, and recomputes every recorded directory
digest. Coverage, observation and state are reported, not readiness gates: they
show which vendor behavior the comparisons never exercised, which executed
production lines they cannot notice and which vendor state they never compare. Any
change to a shard's sources makes that shard stale: its evidence supports no
claim until its scenario runs again, while shards whose sources are unchanged
stay current. The scenario code a shard records is what Cargo compiled into
the libraries that decide verdicts: the scenario engine and the chip's
scenario library, whose dep-info `cargo xtask vendor-scenario` passes to the
scenario binary. Every verification package declares
`package.metadata.open-radio.evidence` as `verdict` or `report`; a report
package only renders reviewer aids, a verdict package may not depend on one,
and writing or checking a shard fails when it would record a report
package's file, so editing a report leaves every shard current. An absent index directory means no vendor evidence is
available: affected capabilities remain unqualified while status and HIL
planning still work. An unreadable, malformed or inconsistent existing shard,
or a file in the directory that is not a shard, remains an error.

The IEEE 802.15.4 host stand
([`verification/esp32s31/host/ieee802154`](../verification/esp32s31/host/ieee802154/README.md))
writes the `ieee802154-host` shard of the same schema: one MATCH entry per
stand scenario comparing the compiled public ESP-IDF driver with the
production engine, over the pinned `esp-idf` sources. Its entries carry no
Blobray coverage, observation or state counts.

`cargo xtask evidence --chip <chip>` rewrites every stale shard, or the shards of the
scenarios it names. The verification owner runs it; other changes leave shards
alone, and a shard their sources made stale stays stale until then. Git merges
shards as binary files, so a conflicting merge keeps one side intact instead of
inserting markers, and the next regeneration replaces it.

The qualification manifest names vendor roots and explicit evidence rows:

```toml
vendor-roots = [
  { source = "archive", symbol = "phy_chip_set_chan" },
]
vendor-evidence = [
  { suite = "channel", source = "archive", symbol = "phy_chip_set_chan" },
]
```

An evidence reference must name one of the capability's roots. The evaluator
derives:

- `qualified` only when every root has a reference with a MATCH entry in the
  current index and no source-only anchor remains;
- `mapped` when reviewed roots or anchors exist but current evidence is absent;
- `unmapped` when neither exists;
- `not-applicable` only from an explicit reason and with no vendor references.

## HIL evidence path

Qualification reads sealed run bundles from the shared run store and tracked
HIL evidence shards (`hil/evidence/<target>`), which `cargo hil evidence record`
writes explicitly from qualifying runs; running a scenario never writes tracked
files. The [qualification reference](../qualification/README.md) describes the
shard binding. Qualification manifests name exact scenario requirements rather
than dated narrative files:

```toml
hil-requirements = [
  { scenario = "station-reconnect", minimum-repetitions = 1 },
]
```

The evaluator first checks that every requirement exists in the typed HIL
scenario catalog and is achievable by its declared repetition count. It then
retains observations separately from their applicability. A scenario can supply
evidence only when:

1. an invocation integrity seal or an independent scenario-attempt seal covers
   the complete inventory of its evidence boundary;
2. every size and SHA-256 digest matches;
3. manifest, suite, run directory and target identities agree;
4. the evidence boundary is completed and the required scenario passed (an
   unrelated scenario or the enclosing campaign may have failed or been interrupted);
5. every required repetition passed;
6. a verified snapshot fully matches the current source selection, regardless
   of dirty state or commit identity, or legacy provenance binds a matching clean
   commit, or an explicit
   property-scoped applicability review binds that observation to the destination
   build and the current reviewed owner inputs;
7. no applicable failure of that scenario or its repetitions remains unresolved;
8. selected named checks satisfy their current criteria.

The evaluator checks recorded measurement verdicts against their original
thresholds independently of the requested criteria. Sufficient numeric
observations can be reassessed without a new experiment or alteration of the
sealed run. Missing measurements remain missing. A changed criterion cannot
rescue a failed lifecycle, and repetition sets cannot be assembled from several
runs. An aggregate infrastructure failure does not erase an explicit failed
repetition. Details are exposed in `hil_decisions` in the qualification report.

An independently sealed attempt covers a whole scenario repetition set, not
an arbitrary successful prefix. It retains its own completion snapshot and
checksums of the immutable subject, procedure and observations. It remains
available before the aggregate campaign seal exists. Qualification consumes
one completion record per scenario in that invocation, never both the attempt
and the later aggregate result. Closing this evidence boundary does not declare
fixture resources healthy after an interrupted subsequent experiment.

Markdown descriptions do not enter this decision; no hand-edited `qualified`
field exists. Applicability defaults to the verified current source composition. An
explicit [review record](../qualification/evidence-reviews.md) can admit an old
observation for one property after checking both builds, current owner hashes,
and the property fingerprint. The same record can bind an individual failure's
resolution or explain its inapplicability. Original observations and exclusions
remain visible; a new relevant failure is not hidden by an older PASS. The
engineering map and qualification consume the same decision.

## Normal workflow

1. Select the capability or source scope being developed. Inspect its status,
   source contracts, knowledge and next-work reasons in the engineering map.
2. Research an unknown hardware contract or implement the selected change in
   its owner. Record accepted hardware definitions in `registers/` and semantic
   contracts with their code owners; retain research provenance in verification.
3. Run focused host tests and the checks appropriate to the affected boundaries.
   Host-test links are selectors, not evidence that those tests have run.
4. Use compiled vendor comparison when the changed contract needs that comparison.
   Use an addressed HIL experiment when the behavior needs hardware observation.
   Choose scenarios supported by the available fixture; a functional observation
   does not establish an unmeasured RF or worst-case timing property.
5. Read the saved results through the selected program and inspect applicability,
   unresolved failures and remaining gaps. A dependency is context for this
   decision, not an instruction to execute every prerequisite scenario.

Broader checkpoints select their full scope explicitly. The strict `gate`
assesses that scope against its declared requirements; ordinary changes do not
implicitly launch a complete vendor/HIL campaign. Missing equipment limits the
properties that can be observed, not the recorded implementation state.
Direct HIL eligibility accepts a validated snapshot matching the current source
inputs the observation depends on. A missing commit or dirty state alone does not require review. An explicit property/build review can
establish applicability of earlier observations. The engineering map exposes
original exclusions and review decisions without claiming that a commit change
resolved a failure.

The commands below produce and assess evidence for an explicitly selected
checkpoint. They are not prerequisites for reading project status.

```console
cargo xtask evidence --chip esp32s31

cargo qualification evaluate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml \
  --json-report target/qualification/wifi-sta.json

cargo qualification gate \
  --manifest qualification/targets/esp32s31/wifi-sta.toml
```

`validate` is the normal source-tree consistency check. `gate` is intentionally
expected to fail while any required claim remains incomplete.
