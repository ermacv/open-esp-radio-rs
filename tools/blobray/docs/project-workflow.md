# Project workflow

The Blobray is project-oriented. A `vendor-project.toml` names reviewed
configuration and generated outputs; private binary paths belong in the
ignored local run specification, never in the reviewed manifest.

Project manifest schema 4 may declare a stable artifact lineage, but never an
exact artifact digest. Blobray hashes the bytes bound by the active local run
specification for every source referenced by exact reviewed facts and uses
those live `{ source, sha256 }` identities during selection. Replacing a bound
blob therefore changes the active context without editing reviewed project
configuration; missing inputs cannot be replaced by a manifest-authored
digest. Shallow `status` and diagnostic `doctor` remain able to report missing
bindings without authenticating unrelated vendor inputs.
For named roles, the exact-fact `source` is the role suffix: a
`source-artifact:vendor` binding authenticates reviewed applicability with
`source = "vendor"`.

## The normal loop

For an existing project:

```console
cargo blobray project doctor --project path/to/vendor-project.toml
cargo blobray project files --project path/to/vendor-project.toml
tools/blobray/scripts/run-limited \
  project analyze --project path/to/vendor-project.toml --jobs 1
cargo blobray project status --project path/to/vendor-project.toml
cargo blobray project research next --project path/to/vendor-project.toml
```

- `doctor` validates configuration, local inputs and reviewed workspaces.
- `files` explains ownership and whether each input or generated output exists.
- `analyze` refreshes symbol, MMIO, interface, linked-IR and navigation facts.
- `status` reads the current results without regenerating the whole project.
- `research next` ranks concrete human-review actions by downstream impact.

`status` is deliberately shallow and reports generated freshness as unknown;
it does not deserialize every large evidence record. Status schema 11 exposes
three independent workflow dimensions: generated `freshness`, `research`
completeness, and `verification`. Phase readiness only says that configured
artifacts are present and structurally inspectable; an `open` research state
can therefore coexist with a ready review artifact. `doctor` is deliberately
deep but still does not claim reproducible freshness. Its JSON schema 3 report
includes per-section timings so expensive symbol, register, interface or input
validation is attributable. Use `project check` when current bytes must be
reproduced, rather than interpreting either inspection command as a freshness
proof.

`project files` reports both `ownership` and `layer`. They answer different
questions: a chip register model and a project disposition are both reviewed,
but the former is reusable `chip` knowledge while the latter is an
`investigation` decision. The layers are composition, architecture,
ecosystem, chip, investigation, local binding, external artifact and
generated. Generic Blobray code does not appear as a project-data layer: move
reusable vendor vocabulary to an ecosystem add-on and reusable addresses or
register geometry to a chip add-on, not into the generic crate. Keep binary
profiles, review scopes, dispositions, verification policy and sparse
artifact-bounded conclusions in the investigation.

The project must select one exact destination for newly accepted sparse facts:

```toml
[reviewed-knowledge]
packs = ["reviewed/project-facts.toml"]
default-pack = "reviewed/project-facts.toml"
```

Every configured pack remains an input, but `default-pack` is the only file
Blobray may recommend as the destination for new project facts. It is required
whenever `packs` is non-empty and must exactly name one entry from that array.
No list-order fallback exists. A project with no packs must not declare a
default. Prefer a project-wide destination over protocol-named files: a single
reviewed assertion can affect Wi-Fi, Bluetooth/BLE, IEEE 802.15.4, coexistence,
or shared PHY logic without moving between files as understanding improves.

Use this ownership rule when Wi-Fi, BT, or 802.15.4 analysis produces new
knowledge:

| Finding | Durable owner | Hand-edit? |
|---|---|---|
| Generic lifting/query algorithm | Blobray source | yes, with tests |
| Public ESP-IDF/NimBLE interface layout/ABI independent of one chip/blob | ecosystem interface-template pack | yes, reviewed reusable fact with header revision/path |
| Chip/revision MMIO geometry, ROM address, base SVD | chip pack | yes, reviewed reusable fact |
| Exact blob source/root/digest/runtime binding, body-identity summary, scope/profile, disposition | investigation project | yes, reviewed and applicability-bounded |
| Register name, W1C/self-clear semantics, vendor access bug | project `reviewed-knowledge.default-pack` | yes, one record per accepted fact |
| Symbols, MMIO candidates, linked IR, review pages, SVD/PAC/reference code | `generated/` | no; reproduce it |
| Vendor binaries and paths | ignored local run spec / `_oracles` | no publication |

Consequently, identifying one register should normally add one sparse reviewed
assertion (plus its evidence), not copy or edit a complete generated register
catalog. The generated SVD/PAC and review output may change broadly without
inflating the reviewed diff. A compiled provider is selected by `chip.toml`
only when all of its assumptions are reusable for that chip; use the project's
`analysis-provider` for exact blob-lineage ABI contracts and body guards. When
both are selected, the installed project descriptor must explicitly extend the
exact chip provider and publish a checked contract superset through a
precomposed harness. Arbitrary pairs and implicit manifest layering fail
closed.

Reusable ecosystem/chip capability packs can summarize reviewed interface
evidence without copying project addresses or symbol spellings into generic
code. Inspect them with `advanced interfaces validate --format json`: every
positive rule lists its operation/effect/call matches, while missing bindings
remain `incomplete` and unknown vocabulary remains `unknown`. `protocol` and
`scope` are classification labels rather than matching filters or
qualification claims. Capability packs are tracked project inputs, so editing
one invalidates interface-dependent analysis/cache generations.

A project that selects a reviewed interface pack must also select exactly one
generated research projection:

```toml
[interfaces]
facts = "generated/findings/interfaces.json"
pack = "interfaces/reviewed.toml"

[interfaces.capability-context]
output = "generated/findings/capability-context.json"
```

This is a hard-cut contract: a reviewed interface pack without
`[interfaces.capability-context]`, or a capability-context output without that
pack, is invalid. `project analyze` writes the compact projection only after
interface validation succeeds; `project analyze --check` reproduces and checks
the same bytes. The file contains the unresolved interface observations needed
to form research actions and the existing capability links needed to annotate
them. It does not replace interface facts, reviewed anchors, semantic catalogs,
templates, or capability rules, and must not be hand-edited.

The projection carries a freshness digest over the project identity, calling
convention, compiled-knowledge identity and every relevant interface fact or
reviewed pack. `research next` rejects a missing, malformed, wrong-project or
stale projection and does not silently rebuild it from live reviewed inputs.
It continues with register and other available research domains, emits a
partial-prioritization diagnostic, and omits interface observations and
capability links until `project analyze` succeeds. This keeps a fast everyday
research query from repeatedly loading and evaluating the complete interface
workspace while preserving a fail-closed knowledge boundary.

Public interface templates use the same composition boundary. The ecosystem
template owns the versioned layout, slot ABI and semantic IDs; the project
anchor owns the exact artifact/source/root/container, digest/runtime guards,
execution contract and keyed overrides. A template slot is a reviewed header
assertion until the project explicitly marks that offset `origin = "observed"`.
Private pointer cells and runtime callbacks never move into the reusable pack.
Template packs are exact tracked inputs for interface-dependent stages, so a
template edit invalidates freshness and cache signatures.

`research next` combines root-cause blockers, the reverse call graph,
unreviewed MMIO and interface observations, sparse unknown hardware semantics,
publication scopes and verification surfaces. Its `G/O/M` metrics mean
guaranteed unlock, optimistic reverse-reachable impact, and marginal benefit
after other blockers are resolved. Co-blockers and an estimated research cost
are explicit score penalties; the JSON report exposes every weighted term so
the ranking is auditable rather than an opaque recommendation. Restrict the
result to one radio area when needed:

```toml
[[review.scopes]]
id = "shared-phy-init"
protocols = ["wifi", "bluetooth", "ble", "ieee802154", "shared"]
profiles = ["radio-all"]
roots = ["radio:phy_init"]
```

Every review scope must declare one or more canonical protocol memberships:
`wifi`, `bluetooth`, `ble`, `ieee802154`, `coex`, or `shared`. Membership is
many-to-many and is not inferred from the scope ID. Human CLI input additionally
accepts `bt`, `802.15.4`, and `802154` aliases.

Public radio entry-point families that are expected but do not yet have an
analysis profile must also be explicit:

```toml
[[analysis.public-symbol-families]]
id = "ieee802154-public-controller"
protocols = ["ieee802154"]
source = "ieee802154"
symbol-prefix = "esp_ieee802154_"
disposition = "required"
profile = "ieee802154-controller"
```

Status reports this family as `missing-vendor-artifact` until the exact source
binding exists, then as `missing-profile-definition` until a linked-IR profile
is declared, as `missing-profile-output` until it is built, and as
`invalid-profile-output` if the generated profile is empty or unreadable. A
profile build is offered only for the latter two states; manual configuration
states remain explicit prerequisites rather than non-runnable commands. The
symbol family becomes analyzed only when that profile is generated and
non-empty. A deliberately omitted family uses `disposition = "excluded"`,
omits `profile`, and requires a one-line `reason`.
Exclusion is not analyzed coverage: Blobray checks the prefix against the
generated symbol inventory, reports every matched identity, and marks a
zero-match declaration `stale-exclusion`. Missing inventory leaves the
exclusion unverified and the analysis phase incomplete. Undeclared public
families are never inferred to be covered or excluded.

Surface states distinguish work ownership. Missing source bindings, stale
reviewed prefixes and missing profile definitions are `coverage-blocked` and
carry one manual prerequisite pointing at the owning run spec or project
manifest. Missing symbol inventory and missing/invalid generated profile
outputs are `ready`, carry no duplicate prerequisite, and expose the exact
`project analyze` or `advanced ir build --profile ...` `next_action`.

```console
cargo blobray project research next --scope ieee802154-baseband-leaves \
  --project path/to/vendor-project.toml --limit 10
```

The command is read-only unless `--output` is supplied. Each candidate names
the missing knowledge, confidence, affected scopes, expected impact and a
typed next action. Report schema 16 ranks unique user actions, exposes the
reviewed names and roles of referenced functions, and includes required public
analysis-surface coverage gates:
independent findings coalesce only when they lead to the same executable action
and have the same typed resolution owner and exact required model. Findings
with one command but different ownership or models remain separate actions, so
the ranking never hides distinct work behind a shared inspection target. Impact is
aggregated across the complete action before ranking, not inherited from the
single highest-scoring finding. Every research packet includes exact direct,
guaranteed, optimistic, marginal and co-blocker identities together with the
required evidence, reviewed destination and completion condition. Low
confidence on write semantics is
intentional: W1C, self-clear and hardware-owned behavior require reviewed HIL
or authoritative documentation and are not inferred from vendor writes.

Schema 16 also imports typed incomplete event-route blockers. These findings
retain the route ID, blocker kind, matching review scope and exact causal inspection
functions, and execute `inspect flow --event-route ID`. They are
`inspection-only`: until the event-flow producer publishes typed impact
evidence, their direct, guaranteed, optimistic, marginal, affected-root and
publication-scope sets remain empty and contribute no unlock weight. Completion requires the exact
route/blocker pair to be absent from the current authenticated event-route
report; a human message or a successful inspection command is not proof.

Preview the exact stage order and cache work before a large analysis:

```console
cargo blobray project analyze --plan --project path/to/vendor-project.toml
```

The plan is read-only. It reports configured dependencies and distinguishes
current outputs, CAS restoration, cold computation, check-mode verification,
deferred decisions, blocked work, failures, and unconfigured stages. If an
earlier stage must materialize an input, a dependent cache decision is reported
as `deferred`; Blobray does not guess the digest of an output that has not been
produced yet. JSON output includes the exact stage signatures that were safe to
evaluate. Add `--details` to the human view to see every profile/work item,
output and cause without grouping.

Before publishing or merging a replacement, run:

```console
cargo blobray project publish --project path/to/vendor-project.toml
cargo blobray project verify --project path/to/vendor-project.toml
cargo blobray project check --project path/to/vendor-project.toml
```

`project check` is the fail-closed reproducibility gate. It verifies generated
outputs and the configured verification policy. It does not decide whether the
product is ready to ship; the repository qualification evaluator is the sole
readiness authority. Unqualified implementations remain reported as coverage
debt in review-scope details but do not make `project status` incomplete and
are not silently promoted into mandatory verification claims. Only an explicit
verification-policy requirement makes a production trace a Blobray gate.
Large JSON outputs are serialized directly against their existing bytes with
bounded buffers in check mode; verification does not need a second copy in
`/tmp` and does not write a staging file into the project.

## Focused and full analysis

Focused inspection, a selected IR profile and `project analyze` use the same
recovery engine. Full project analysis enumerates all configured roots; it
does not switch to a second bulk algorithm. The persistent store reuses a
complete immutable profile/stage projection, including explicit
incomplete/blocker results, and restores its generated files from CAS. It also
stores direct-function facts, so partially overlapping root sets can reuse the
functions they share. All stale IR profiles selected by one `project analyze`
invocation enter the builder together, so shared catalogs and reviewed
interface knowledge are loaded once. Cache-current profiles are not rebuilt.

Each direct-function fact is bound to its exact owner identity and body and to
conservative resolver, MMIO and harness-semantic fingerprints. Other function
bodies are excluded from that resolver fingerprint: changing one body can
reuse unchanged facts for the other functions. Resolver layout, MMIO or
harness changes may conservatively invalidate a wider set; the cache does not
claim an exact per-function dependency DAG. A provider with summary hooks must
publish a stable, versioned semantic cache domain. Missing domain identity
fails closed to cold analysis instead of reusing a potentially stale fact.

The local persistent store lives below `generated/.blobray-cache/`. Removing
that directory only causes a cold recomputation. It never removes reviewed
knowledge or generated publication artifacts. Profile IDs and output paths
are bindings, so renaming an otherwise identical investigation does not change
its analysis query key. Changing artifact bytes, provenance, ABI/backend
revision or a semantic input covered by the current query domain creates a new
key. Direct-function reuse remains intentionally conservative across resolver,
MMIO and harness-semantic changes.

## Updating vendor artifacts without losing review

Capture an immutable snapshot and run the preflight **before** replacing
caller-owned artifacts. The preflight hashes the live bindings, verifies them
against the current immutable snapshot and records that the old baseline is
safe. After replacement, run normal analysis, capture the new revision and
compare the state's adjacent `baseline` and `current` entries:

```console
cargo blobray project revision snapshot vendor-2026-05 --project path/to/vendor-project.toml
cargo blobray project revision prepare-update --project path/to/vendor-project.toml
# only now update local artifact bindings, then run project analyze
cargo blobray project revision diff vendor-2026-05 @live \
  --project path/to/vendor-project.toml --details
cargo blobray project revision rebase vendor-2026-05 @live \
  --project path/to/vendor-project.toml \
  --lineage generated/revisions/ble-symbol-lineage.json \
  --output generated/revisions/vendor-2026-08.rebase.json
# after reviewing the delta and rebase plan, publish the immutable new snapshot
cargo blobray project revision snapshot vendor-2026-08 --project path/to/vendor-project.toml
```

`--lineage` is optional, but without it entity bindings remain review-required
because their artifact-bound occurrences cannot survive a blob replacement by
name alone. A `confirmed` direct-plus-chain mapping becomes a generated
`carry-remapped` proposal with the exact target occurrence. `direct-only` and
`chain-only` mappings include the proposed occurrence but remain
`review-required`. The rebase command rebuilds the complete current-schema
lineage from the artifact paths embedded in the report and requires canonical
byte equality before trusting any `confirmed` status. It then validates the
report digest, endpoint artifacts, occurrence domains, locators and one-to-one
target ownership. A missing artifact or manually changed report fails closed.
It never edits the reviewed pack.

When a binding is remapped, only the lineage source artifact constraint is
projected onto the target artifact; chip, chip-revision, ecosystem, lineage and
any unrelated artifact constraints must still match the target snapshot.

When a vendor regenerates private symbol names, correlate the public or older
named archive with the obfuscated revision before reviewing the update:

```console
cargo blobray advanced symbols correlate \
  --from named=/path/to/older-named.a \
  --to current=/path/to/current.a \
  --output generated/revisions/named-to-current.json
```

The correlator distinguishes a source name, a generated obfuscation token and
a semantic identity. A unique non-generated name is an identity anchor even
when its implementation changed. A generated 20-character token is an anchor
only when archive-wide evidence proves that both artifacts belong to the same
obfuscation epoch: in at least one domain, 64 tokens must overlap and at least
90% of the smaller token set must survive. The report publishes the token
counts, retention and `compatible`, `distinct` or `inconclusive` evidence
separately for functions and data objects, plus one archive-wide decision. A
strong function overlap can therefore prove the epoch even when data objects
were aggressively removed; only exact unique shared data tokens are then
carried. A hard token regeneration disables all token-based automatic matches
instead of guessing across the boundary.

The correlator also hashes complete relocatable function bytes plus relocation
offset/kind/addend while deliberately excluding relocation target names. A
body-only match is published only when that fingerprint is unique. An
iterative second pass may resolve otherwise identical bodies when already
unique caller/callee pairs prove the corresponding call edge. Disagreement
between stable identity and unique-body evidence remains an explicit conflict.
Other ambiguous and changed bodies remain review work. A stable generated
token is a revision locator, not a semantic name; the report never rewrites the
artifact or silently promotes it into reviewed knowledge.

For archive revisions that replace alphabetically sorted source-object names
with `0.o`, `1.o`, and so on, the report also publishes the complete inferred
member-order table and measures it against unique exact-body matches. This is
module provenance and never promotes an ambiguous function: functions can
move between modules across releases. When at least 64 exact function bodies
support the table, at least 90% of all measured bodies agree, and none
conflict, an ambiguous exact-body static object may be reduced to the single
candidate in its proven renamed member. Member order cannot rescue a changed
or absent object. Every function and object record includes an exact
artifact-bound revision occurrence and its derivation locator for a later
reviewed pin.
Static data objects are correlated separately from functions using stable
non-generated names, epoch-gated generated tokens, bounded initializer bytes,
size/properties, and relocation shape with target names removed. Exact mapped
function relocations may resolve otherwise identical or changed objects.
Repeated zero-initialized state and tables without a unique identity or
reference remain ambiguous. The complete data correspondence keeps local
compiler labels such as `.LANCHOR*` and `.LC*` as provenance, but does not
offer those unstable labels as semantic pin candidates. Generated obfuscation
tokens are likewise excluded from semantic-name suggestions. The generated
`pin-candidates` list contains only meaningfully named function and
memory-object occurrences, but every candidate remains explicitly
`review = required`;
Blobray never promotes generated correspondence into a reviewed fact.

When more than one obfuscation epoch or intermediate vendor release is
available, build one lineage report instead of manually joining pairwise JSON:

```console
cargo blobray advanced symbols lineage \
  --source ble-controller \
  --revision named=/path/to/named.a \
  --revision old-entry=/path/to/first-obfuscated.a \
  --revision old-exit=/path/to/last-old-epoch.a \
  --revision new-entry=/path/to/first-new-epoch.a \
  --revision current=/path/to/current.a \
  --output generated/revisions/ble-symbol-lineage.json
```

`--source` is the stable logical artifact identity used by project snapshots
and rebase. Each `--revision` label is only the unique human-readable release,
tag, or commit name shown in reports; labels such as `5e37d4d` are valid and do
not change occurrence identity.

Lineage runs every adjacent correlation plus an independent first-to-last
correlation. It composes only unique one-to-one occurrences. Agreement is
`confirmed`; a result available through only one route is `direct-only` or
`chain-only`; disagreement is a `conflict` with no resolved target. Partial
paths retain the exact edge, status, evidence basis and candidate count that
blocked composition. The report stores artifact digests and every successful
hop, but not vendor bytes or disassembly. Its pin candidates still require
review and exclude generated token names.

Schema 5 also retains the failed independent direct correspondence and ranks
`review-frontiers` that lack any resolved target before routes that need only
independent corroboration, then by reviewable semantic names and finally by
all affected functions or objects. Compiler labels remain counted but cannot
outrank a frontier that unlocks project-owned facts. An
`adjacent-chain` frontier identifies the exact release boundary that blocks a
complete history; `direct-endpoint` means the ordered history resolves the
entity but the independent endpoint proof is absent; `endpoint-conflict`
means both routes resolve to different targets. `--details` prints these
frontiers highest-impact first and includes the direct endpoint comparison in
the edge table.

For `project revision rebase`, the first and last lineage artifact identities
(the shared source ID plus each digest) must match artifacts in the accepted
baseline and target snapshots.
If the useful named archive predates that baseline, keep its naming lineage as
research evidence and generate a second, smaller lineage beginning at the
actual baseline artifact. Pass the project's logical source ID once with
`--source`; the exact digest distinguishes its revisions.

Accept a candidate only by adding a sparse `[[bindings]]` record to the
project's reviewed-knowledge TOML. The record must repeat the target
occurrence, assign a domain-matching `function:...` or `memory-object:...`
semantic identity, constrain applicability to the exact artifact digest, and
cite evidence whose locator re-derives that occurrence. The next Linked-IR
build publishes `semantic` beside the raw symbol, member, artifact digest,
locator and occurrence. Linked-IR indexes resolve either identity while raw
call-graph and xref keys are retained. Changing the reviewed pack invalidates
the Linked-IR cache. Missing, stale, cross-domain, forged or colliding bindings
fail closed.

One accepted candidate is the only project-owned growth required for one new
fact. Copy `target_occurrence` and `target_locator` from the candidate and the
target source/digest from the report header; choose the semantic path only
after review:

```toml
schema = 2
id = "esp32s31-ble-reviewed-identities"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[[bindings]]
id = "ble.scheduler-state.binding"
occurrence = "occurrence:memory-object:sha256:<candidate-digest>"
semantic = "memory-object:esp-idf/ble/controller/scheduler-state"

[bindings.applies-to]
artifacts = [
  { source = "ble-controller", sha256 = "<report-target-sha256>" },
]

[[bindings.evidence]]
source = "manual-symbol-correspondence-review"
locator = "<candidate-target-locator>"
occurrence = "occurrence:memory-object:sha256:<candidate-digest>"
```

Do not copy `suggested_name` into `semantic` mechanically. It is old-archive
nomenclature and remains a review hint, not an accepted hardware or controller
model.

Revision snapshots use a reviewed function ID as the comparison key while
retaining the raw identity, exact artifact digest, locator and occurrence as
separate provenance. Calls to reviewed callees and effects on reviewed static
objects use their semantic targets in fingerprints. A vendor-only rename then
stays unchanged; changed behavior under the same reviewed identity remains a
modification. Snapshot creation rejects stale generated semantics and requires
the Linked-IR to be rebuilt after any accepted pin change.

After rebuilding Linked IR, use reviewed identities directly instead of
copying obfuscated names from generated files:

```console
cargo blobray inspect function \
  ble:function:esp-idf/ble/controller/advertising-start \
  --project radio-project/vendor-project.toml
cargo blobray inspect flow \
  ble:function:esp-idf/ble/controller/advertising-start \
  --effects memory --project radio-project/vendor-project.toml
cargo blobray inspect object \
  ble:memory-object:esp-idf/ble/controller/advertising-state \
  --project radio-project/vendor-project.toml
```

The generated reports retain both the semantic identity and the exact raw
symbol/member. A missing or conflicting resolution fails closed and asks for
a fresh IR build; Blobray never guesses a raw occurrence from a semantic path.

`@live` is a read-only revision operand. It builds and validates the same
projection as `revision snapshot`, including current artifact identities and
generated evidence, but does not write a snapshot or advance the state. This
lets a named, durable baseline be compared with newly analyzed bindings before
they are published. The diff has an explicit function delta and typed research
invalidation areas (including affected reviewed records); the rebase plan says
which reviewed facts remain exact, remap uniquely, or require review.

Snapshots default to deterministic `revisions/snapshots/NAME.json.gz`; the small
`revisions/state.blobray` is a tool-written custom DSL whose
`blobray-revision-state 1` header is followed by typed directives. It is
updated atomically with the snapshot location, logical-content SHA-256,
artifact-set SHA-256 and explicit `baseline`/`current` pointers. The logical
digest is independent of the replaceable gzip storage codec. Both paths are
outside disposable `generated/` state and should be
committed or backed by equivalent durable, access-controlled storage. `project
status` and `project doctor` warn when no baseline exists; deep doctor also
checks immutable snapshot digests and reports current binding drift.
Snapshot creation hashes the live caller-owned bindings and rejects stale
analysis evidence whose recorded artifact identities are no longer present.
Schema 3 deliberately hashes only typed `vendor-*` and
`source-artifact:*`/`source-inventory:*`/`source-companion:*` roles. Ambiguous
generic `artifact`/`companion` roles fail closed in the revision workflow.
Local `rust-artifact` probes remain freshness-checked verification inputs, but
rebuilding the driver or a probe cannot create a false vendor revision.
Reusing one source ID for both a vendor and Rust role also fails closed.
Linked-IR bundles retain exact primary, inventory and companion provenance, so
revision capture rejects a bundle produced from stale dependency bytes.

`prepare-update` fails if the caller has already replaced a bound vendor
artifact, if a snapshot is missing or modified, or if the previous revision
transition has not been accepted. After completing diff/rebase review, use
`prepare-update --accept-current` to make `current` the next baseline and
record a new preflight. `prepare-update --check` is read-only and verifies an
existing marker. A snapshot with changed artifact identities is rejected
unless its predecessor has a matching marker, so cleanup or an accidental
binding edit cannot silently erase the old correspondence map.
While `baseline` and `current` differ, `project status` and deep doctor report
`revision-review-pending` instead of `ready`; a captured snapshot is not an
accepted review decision. Blobray accepts only schema-4 snapshots and
revision-state DSL version 1. TOML and older state are not migrated or
interpreted: remove invalid state, then capture a fresh current snapshot from
the live typed vendor bindings.

`source-artifact:*` may identify a locally linked analysis container around
vendor objects. It is still part of exact analysis provenance: rebuilding or
relinking that container can require a revision preflight even when the raw
vendor inventory is unchanged. The durable raw-vendor identity comes from the
corresponding `source-inventory:*` inputs; do not interpret exclusion of Rust
probes as proof that every analysis container is itself vendor-distributed.

When no linked container is needed, one logical source may instead bind an
ordered set of primary archives by repeating `source-artifact:ID`. Blobray
analyzes every member in place and links uniquely named relocation targets
across the set without synthesizing an executable or assigning runtime
addresses. The same `source-artifact:ID=PATH` pair may occur only once, and an
archive set cannot also use `source-companion:ID`: add every required code
archive to the primary set explicitly. Stable function identities retain the
logical source, archive member and symbol, so a blob update remains eligible
for normal revision diff and rebase.

Snapshots contain artifact
digests, address-independent function feature fingerprints, MMIO/interface
observations and complete serialized reviewed records, but no vendor bytes or
disassembly. The portable fingerprint is a cross-version correlator and never
replaces the address-bound evidence/cache identity.

Diff classifies stable identities, unique moves, modifications, additions,
removals, split/merge candidates and ambiguity. Only an unchanged stable
subject or a one-to-one exact normalized-feature move is automatically
carryable. Entity bindings additionally require exact occurrence
correspondence: a stable semantic name alone never carries a blob-local
binding. Split, merge, ambiguity, removal, modified semantics and stale
artifact applicability remain `review-required`. A rebase plan retains every
old assertion, vendor-bug and entity-binding record with its provenance even when it cannot be
carried, so an upgrade cannot silently discard research progress.

Inspect the store before deciding whether cache growth needs investigation:

```console
cargo blobray project cache stats --project path/to/vendor-project.toml
cargo blobray project cache gc --dry-run --max-size 4294967296 \
  --project path/to/vendor-project.toml
cargo blobray project cache gc --dry-run --retention-days 30 \
  --max-size 4294967296 --project path/to/vendor-project.toml
cargo blobray project cache gc --apply --retention-days 30 \
  --max-size 4294967296 --project path/to/vendor-project.toml
cargo blobray project cache compact --max-size 4294967296 \
  --project path/to/vendor-project.toml
```

The command reports cache-file, database and pack sizes, query kinds, dependency
and object counts, live records, and currently reclaimable pack bytes. It opens
an existing cache read-only. If the store has not been created, that state is
reported without creating `generated/.blobray-cache/` or any SQLite sidecar.
`cache stats` does not perform garbage collection, compaction, pruning or quota
enforcement, and it neither creates nor upgrades a cache schema; it only
describes the state already on disk.
Its assessment reports the platform support and exact thresholds for automatic
pack compaction. `cache gc --dry-run` reports the reachable-pack or explicit
retention projection, temporary space requirement, available space and an
optional byte-exact `--max-size` guard without creating or changing cache
state. `cache gc --apply` additionally requires `--retention-days` and prunes
only CAS objects with a persisted obsolete timestamp at or before that cutoff.
`cache compact` is an explicit reachability mutation; both mutations are
supported only on Linux local filesystems, hold the cache writer lock, rewrite
and verify every preserved CAS digest through the pinned cache root, atomically
switch the SQLite index, then remove old packs. They refuse known
network/userspace filesystems,
insufficient working space and a size limit that cannot be met without
evicting live results. The size limit is therefore an actionable guard, not a
silent lossy quota.

Age retention is never guessed from file mtimes. Schema 10 persists the time at
which a CAS object loses its final stage/query reference and requires every
query result to belong to at least one analysis epoch. An unowned result is
invalid cache state, not a GC candidate. Current results and unretired function
facts are not per-result LRU candidates.

Schema 10 is created only from a cold store. A cache at any other schema must be
removed explicitly after reviewed TOML, revision snapshots, linked IR and other
durable artifacts have been preserved. Remove the entire
`generated/.blobray-cache/` directory, not selected SQLite or pack files, and
rerun analysis; there is no cache import or upgrade path. The complete
measurement, hard-cutover, retention and 64-KiB storage rationale is in
[`cache-policy.md`](cache-policy.md). `project analyze --plan` is the
read-only way to see whether
each stage is current, restorable from CAS, or requires recomputation. Plan
schema 2 keeps the default decision summary bounded while exposing every
deferred generated input and its producer stage as structured
`awaiting-inputs` in JSON and in `--details` output.

Writing analysis emits structured `cache_outcome` events for current hits,
restored hits, misses followed by recomputation, and publication of recomputed
results. The normal pipeline report independently keeps `up-to-date`,
`restored` and `written` stage counts; use `-v` when the per-stage cache events
are needed.

SQLite and the append-only pack have different jobs. SQLite provides indexed
function queries, transactional stage bindings, dependency edges and locking;
the pack keeps large immutable values and generated outputs out of the database
WAL. JSONL would require linear scans or a second index and cannot atomically
replace several bindings. ZIP central-directory rewrites make it unsuitable for
incremental writes and crash recovery. Both remain useful interchange formats,
not query-store replacements. The store is local and disposable: portable
research progress lives in sparse reviewed TOML, generated linked-IR bundles
and revision snapshots. Never copy the SQLite/WAL directory between machines.

For performance measurements, set `BLOBRAY_REPORT_USAGE=1` when
calling `scripts/run-limited`. This selects its process-session watchdog and
prints elapsed time plus peak RSS for the complete Blobray process tree.
External `/usr/bin/time` otherwise measures only the `systemd-run` wrapper on
hosts where the systemd limiter is available.

## Creating a project

```console
cargo blobray project init \
  --directory radio-project \
  --id radio \
  --source vendor \
  --mmio radio=0x60000000..0x60010000

cargo blobray project inputs init \
  --project radio-project/vendor-project.toml \
  --bind source-artifact:vendor=/opt/vendor/libvendor.a
```

Generated analysis files under `generated/` are disposable results. Durable
machine-written revision snapshots and complete-run vendor-evidence indexes are
separate correspondence/publication records: preserve them in version control
or equivalent controlled storage, but do not edit them as reviewed facts.
Reviewed packs, policies, register models and dispositions are source inputs.
`_oracles/`, vendor binaries, disassembly dumps and credentials remain private.

## Investigating one function

```console
cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml

cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml --replacement
```

The ordinary view is for understanding vendor behavior. The replacement view
adds reviewed ownership, production binding, proof strength and verification
status. These are deliberately separate questions. Function investigation
schema 17 attaches the same typed blocker-resolution route used by research
ranking: owner, producer effect, minimum evidence, optional consumed record
destination, and exact authenticated root completion predicate. A missing
backend capability therefore never appears as an instruction to edit the
function pack. It also exposes a reviewed function's name, role and summary
independently of whether its ABI signature has already been reviewed.

Use `project browse` for navigation and `advanced ...` only for backend
debugging or a focused low-level experiment.

## Resource limits

Real binaries must be analyzed through `scripts/run-limited`. The wrapper
limits aggregate memory and runtime; a large analysis should fail visibly
rather than make the development machine unusable. Build the optimized host
once when iterating repeatedly:

```console
CARGO_BUILD_JOBS=2 cargo build --profile blobray \
  -p blobray-esp32s31 --bin blobray
```
