# Project workflow

The Blobray is project-oriented. A `vendor-project.toml` names reviewed
configuration and generated outputs; private binary paths belong in the
ignored local run specification, never in the reviewed manifest.

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
binding exists, then as `missing-profile` until the declared linked-IR profile
is generated and non-empty. A deliberately omitted family uses
`disposition = "excluded"`, omits `profile`, and requires a one-line `reason`.
Exclusion is not analyzed coverage: Blobray checks the prefix against the
generated symbol inventory, reports every matched identity, and marks a
zero-match declaration `stale-exclusion`. Missing inventory leaves the
exclusion unverified and the analysis phase incomplete. Undeclared public
families are never inferred to be covered or excluded.

```console
cargo blobray project research next --scope ieee802154-baseband-leaves \
  --project path/to/vendor-project.toml --limit 10
```

The command is read-only unless `--output` is supplied. Each candidate names
the missing knowledge, confidence, affected scopes, expected impact and a
copyable next inspection command. Report schema 6 ranks unique user actions:
independent findings that lead to the same inspection command remain listed as
`related_findings` instead of consuming duplicate top-N slots. Impact is
aggregated across the complete action before ranking, not inherited from the
single highest-scoring finding. Every research packet includes exact direct,
guaranteed, optimistic, marginal and co-blocker identities together with the
required evidence, reviewed destination and completion condition. Low
confidence on write semantics is
intentional: W1C, self-clear and hardware-owned behavior require reviewed HIL
or authoritative documentation and are not inferred from vendor writes.

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
product is ready to ship; the repository qualification ledger is the sole
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
compare the ledger's adjacent `baseline` and `current` entries:

```console
cargo blobray project revision snapshot vendor-2026-05 --project path/to/vendor-project.toml
cargo blobray project revision prepare-update --project path/to/vendor-project.toml
# only now update local artifact bindings, then run project analyze
cargo blobray project revision diff vendor-2026-05 @live \
  --project path/to/vendor-project.toml --details
cargo blobray project revision rebase vendor-2026-05 @live \
  --project path/to/vendor-project.toml \
  --output generated/revisions/vendor-2026-08.rebase.json
# after reviewing the delta and rebase plan, publish the immutable new snapshot
cargo blobray project revision snapshot vendor-2026-08 --project path/to/vendor-project.toml
```

`@live` is a read-only revision operand. It builds and validates the same
projection as `revision snapshot`, including current artifact identities and
generated evidence, but does not write a snapshot or advance the ledger. This
lets a named, durable baseline be compared with newly analyzed bindings before
they are published. The diff has an explicit function delta and typed research
invalidation areas (including affected reviewed records); the rebase plan says
which reviewed facts remain exact, remap uniquely, or require review.

Snapshots default to deterministic `revisions/snapshots/NAME.json.gz`; the small
`revisions/ledger.toml` is updated atomically with the snapshot location,
logical-content SHA-256, artifact-set SHA-256 and explicit `baseline`/`current`
pointers. The logical digest is independent of the replaceable gzip storage
codec. Both paths are outside disposable `generated/` state and should be
committed or backed by equivalent durable, access-controlled storage. `project
status` and `project doctor` warn when no baseline exists; deep doctor also
checks immutable snapshot digests and reports current binding drift.
Snapshot creation hashes the live caller-owned bindings and rejects stale
analysis evidence whose recorded artifact identities are no longer present.
Schema 2 deliberately hashes only typed `vendor-*` and
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
accepted review decision. Blobray accepts only schema-2 snapshots and ledgers.
Older state is not migrated or interpreted: archive or remove it, then capture
a fresh current snapshot from the live typed vendor bindings.

`source-artifact:*` may identify a locally linked analysis container around
vendor objects. It is still part of exact analysis provenance: rebuilding or
relinking that container can require a revision preflight even when the raw
vendor inventory is unchanged. The durable raw-vendor identity comes from the
corresponding `source-inventory:*` inputs; do not interpret exclusion of Rust
probes as proof that every analysis container is itself vendor-distributed.

Snapshots contain artifact
digests, address-independent function feature fingerprints, MMIO/interface
observations and complete serialized reviewed records, but no vendor bytes or
disassembly. The portable fingerprint is a cross-version correlator and never
replaces the address-bound evidence/cache identity.

Diff classifies stable identities, unique moves, modifications, additions,
removals, split/merge candidates and ambiguity. Only an unchanged stable
subject or a one-to-one exact normalized-feature move is automatically
carryable. Split, merge, ambiguity, removal, modified semantics and stale
artifact applicability remain `review-required`. A rebase plan retains every
old assertion/vendor-bug record with its provenance even when it cannot be
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
status. These are deliberately separate questions.

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
