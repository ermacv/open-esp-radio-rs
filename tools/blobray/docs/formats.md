# Formats and schemas

Reviewed configuration is TOML; large generated analysis artifacts are JSON
or JSONL. Persistent machine formats have explicit schema numbers and reject
unknown fields.

## Schema-3 composition inputs

- `target.toml`: architecture, calling convention, endianness, pointer width,
  and Rust target only;
- `ecosystem.toml`: reusable ordered semantic catalogs, capability-rule packs,
  and public interface-template packs for a vendor/RTOS ecosystem, with no
  chip addresses or executable provider; `[applicability].ecosystems` declares
  the stable reviewed-fact identities contributed by this layer;
- `chip.toml`: reusable memory map, base register model, SVD inputs, chip
  semantic catalogs, and an optional compiled knowledge-provider ID;
  `[applicability]` declares its `chips` and `chip-revisions` identities;
- `vendor-project.toml`: composition, reviewed workspaces, and generated
  output selection; its optional `analysis-provider` selects compiled logic
  that is valid only for this investigation, never reusable chip facts;
  `[applicability].artifact-lineages` declares the selected blob lineage and
  `[applicability].artifacts` may bind facts to exact `{ source, sha256 }`
  identities for one vendor revision;
- `[interfaces.capability-context].output`: the required generated destination
  whenever `[interfaces].pack` selects reviewed interface knowledge. Declaring
  either side without the other is invalid; no legacy implicit destination or
  live-evaluation fallback exists;
- `[reviewed-knowledge].packs`: sparse accepted assertions and vendor-bug
  records with stable IDs, evidence, provenance and applicability;
  `default-pack` is mandatory for a non-empty list and exactly selects the one
  configured project pack that receives newly reviewed facts. It is forbidden
  when no packs are configured; pack order is never a destination fallback;
- `[[analysis.public-symbol-families]]`: explicit required or intentionally
  excluded public entry-point families, each with canonical protocol tags,
  source and symbol prefix. Required families name their expected profile;
  exclusions require a reason and must match current symbol-inventory
  identities. Missing artifacts/profiles, analyzed profiles and exclusions
  remain distinct states; exclusions never count as analyzed coverage;
- local run specification: ignored bindings to caller-owned private artifacts.

`project files` schema 3 exposes the resolved portability layer separately
from edit ownership, so a reviewed file can still be identified as reusable
ecosystem/chip knowledge or as investigation-local review.

The schema-3 project, target, ecosystem, and chip formats are a clean break.
Old inline `memory-map`, `svd`, `platform-pack`, `harness`, and
`semantic-catalogs` keys fail closed; no compatibility shim reinterprets them.
A project-local `analysis-provider` and chip-pack `knowledge-provider` compose
only when the installed analysis descriptor explicitly extends that exact chip
provider. Registry validation requires a reusable root, a complete contract
superset and a distinct precomposed harness/cache domain; there is still one
effective provider after resolution. Missing or unrelated descriptors fail
closed instead of using manifest order as executable precedence.

## Other reviewed inputs

- `verification-addon.toml` (`schema = 3`): suites, compiled-artifact
  comparison inputs, declarations, and report paths; it has no executable
  verdict provider and grants no analysis knowledge;
- register/interface/function packs: reviewed assertions and evidence links;
- sparse reviewed-knowledge packs (`schema = 1`): opaque subject/kind/value
  facts and vendor bugs. Empty evidence, hints, duplicate IDs, invalid artifact
  hashes and overlapping assertions fail closed; pack and record applicability
  are intersected rather than overridden. The effective project composition
  selects facts before use. A missing context dimension or two same-subject,
  same-kind facts selected by an ambiguous context is an error;
- disposition manifests: reviewed vendor-to-production binding and claim
  declarations, never execution truth;
- verification policy: required comparisons and bounded properties;
- evidence catalogs: provenance links for reviewed claims.

### Reusable capability packs

A schema-1 capability pack contains `[[rules]]` with a stable dotted `id`, a
`protocol`, a classification `scope`, a human summary, optional `depends`, and
nested `[[rules.requirements]]`. Requirement `kind` is `operation`, `effect`,
or `call`; `value` is reviewed semantic vocabulary and `min-matches` defaults
to one. Rule IDs may be shared across ecosystem and chip packs only when their
complete definitions are identical. Missing dependencies, cycles, duplicate
matchers, zero match counts and conflicting definitions fail during loading.

`protocol` and `scope` are report labels, not evidence filters or coverage
claims. A rule searches all validated interface bindings. `operation` matches
a reviewed semantic binding, `effect` matches one of that binding's reviewed
effects, and `call` additionally requires at least one concrete resolved call
site. Machine reports contain the exact binding/call evidence and sort packs,
rules and matches deterministically.

A `matched` result means only that the declared rule matched current reviewed
interface evidence. It does not establish hardware support, runtime ordering,
semantic completeness, or qualification. Known vocabulary without enough
current evidence is `incomplete`; vocabulary absent from the configured
semantic catalogs is `unknown`. Either state propagates through dependent
rules and never becomes a positive capability claim.

### Reusable interface templates

A schema-1 interface-template pack contains public, versioned callback-table
layouts. Each template owns only its stable ID, public header provenance
(`repository`, exact 40-hex `revision`, and relative `path`), layout version,
pointer width, size, stride, and slot offsets/ABI/semantic IDs. It cannot name
an artifact source, symbol/address root, container path, digest/runtime guard,
execution contract, or compiled execution model. Duplicate pack IDs and
same-ID template conflicts fail closed; identical template definitions from
distinct pack IDs may be deduplicated.

An interface pack schema-3 anchor opts in with `template = "..."`. That
project anchor still owns the exact source/root/container binding, exactly one
artifact SHA-256 guard, any runtime guards, and its execution contract.
`[[anchors.overrides]]` entries are keyed by a template slot `offset`, require
a one-line `reason`, and may explicitly change provenance, ABI/semantic fields,
or attach a project provider execution model. Template slots start as reviewed
public-header assertions: only an explicit `origin = "observed"` override may
classify matching generated artifact evidence. Unknown/duplicate offsets,
unexplained overrides, local layout duplication, and missing digest bindings
fail closed. Validation JSON preserves sorted template pack IDs, source
provenance, and every overridden offset/reason/field; reasons are diagnostic
provenance, never executable semantics.

## Durable revision state

- immutable schema-2 revision snapshots (`revisions/snapshots/NAME.json.gz`)
  contain only typed vendor artifact/inventory/companion digests and normalized
  vendor-derived features, never vendor payloads, disassembly, or local Rust
  verification ELF identities;
- `revisions/ledger.toml` is the tracked schema-2 index for those snapshots.
  It stores only project/revision names, relative snapshot locations, SHA-256
  identities, `baseline`/`current` pointers and an optional update-preflight
  marker. `snapshot-sha256` identifies normalized logical snapshot content,
  not the encoded `.json.gz` bytes; gzip is a replaceable storage codec.

Snapshots are tool-written correspondence maps rather than manually reviewed
facts. Unlike ordinary generated output, they and their ledger must survive a
vendor update; commit them or place them in equivalent durable,
access-controlled storage. Snapshot names are immutable.

Linked-IR schema 61 records the primary artifacts, symbol inventories and
companions that affected each generated bundle. Revision capture compares all
three dependency classes with the current typed run-spec and rejects stale
generated evidence.

Only schema-2 snapshots and ledgers are accepted. Schema-1 state and migration
maps are not parsed or upgraded. Archive or remove an older ledger and capture
a fresh schema-2 baseline from the live typed vendor bindings.

## Generated outputs

- symbol, MMIO, and interface observations;
- interface capability context (`schema_version = 1`, command `interfaces
  capability-context`): a compact, deterministically sorted projection of
  unresolved interface observations and existing capability links for
  `research next`. Its `input_digest` covers project identity, calling
  convention, compiled-knowledge identity, generated interface facts, the
  reviewed interface pack, and configured semantic, capability and interface
  template packs. `project analyze` writes it after interface validation and
  `project analyze --check` verifies it. It is disposable derived state, never
  reviewed authority; a missing, malformed, wrong-project or stale document is
  omitted with an explicit partial-prioritization diagnostic rather than being
  reconstructed from live inputs;
- canonical derived linked-IR bundles and indexes, including structural loop
  regions, explicitly non-proving counted-loop candidates, and raw-bit
  floating value-flow nodes whose operation and rounding mode remain explicit;
- navigation and review-scope indexes; project-wide call associations retain
  source-qualified candidates and a unique/ambiguous/unresolved status without
  claiming linker resolution. Review-scope schema 12 persists the mandatory
  many-to-many `protocols` membership from project configuration; protocol
  membership is never reconstructed from scope IDs;
- pseudo-Rust and executable reference artifacts;
- verification reports and evidence index;
- SVD, raw PAC, bindings index, and restricted API output;
- revision diff and rebase plans. Diff reports use their own schema 1,
  independently of schema-2 stored snapshots, and include a typed function
  delta (`changed`, `added`, `removed`, `{ before, after }` remaps and uncertain
  identities) plus research invalidation areas with affected subjects and
  reviewed-record IDs. `@live` is a read-only operand for validating and
  comparing current analyzed bindings before publishing a new immutable
  snapshot;
- research-next reports (`schema_version = 11`) contain one deterministic,
  SHA-256-identified full inventory of findings, actions and prerequisites.
  Actions refer to the single typed finding catalog by ID and prerequisites
  carry no rank; the bounded `selection.steps` list contains only ordered typed
  IDs. Strategy, limit and budget do not change the inventory digest.
  The always-present exact-finding query distinguishes `all`, `open`,
  `condition-satisfied`, `input-not-observed`, `filtered-out`, and
  `not-present`; typed register resolution evidence never claims completion
  or historical occurrence. Findings retain subjects,
  executable-consumer resolution, actionability,
  evidence, impact sets, a typed exact-finding requery action and typed
  revalidation actions. Every executable action stores exact argument
  boundaries, the absolute invocation working directory and its project-context
  level; rendered shell text is never part of the machine schema. Capability matches and
  verification surfaces are context-only links with zero ranking weight, and
  the report makes no completion claim.
- project-status reports (`schema = 11`) keep shallow artifact readiness
  separate from generated freshness, open research debt and verification
  readiness; review-scope details expose their explicit protocol memberships,
  while `radio_surfaces` reports analyzed profiles, missing vendor
  artifacts/profiles, and audited public-family exclusions. `ready` never means
  that a review scope has no remaining work.

Ordinary generated analysis outputs under `generated/` are disposable and
reproducible. They must preserve source artifact identity/provenance and must
not contain proprietary payloads or full disassembly dumps. A generated file
cannot replace its reviewed input.

Two machine-written records are intentionally durable rather than disposable:
the immutable revision snapshots described above, and the complete-run vendor
evidence index selected by a verification add-on. The evidence index is a
compact qualification publication whose source hashes are rechecked by the
qualification ledger; preserve both record classes in version control or
equivalent controlled storage. They remain tool-owned records, not manually
reviewed fact packs.

## Internal persistent query store

`generated/.blobray-cache/queries.sqlite3` and `objects-*.pack` are
disposable local implementation state, not project formats and not evidence.
SQLite owns query keys, dependency edges, project-local output bindings, and
the active immutable pack generation. Small query values remain inline in
SQLite. Pack generations own larger immutable query values and every
restorable generated output, addressed by SHA-256. Reachability compaction
atomically switches generations and removes unreferenced objects. A
store-schema change recreates the database and packs. No reader for an
obsolete cache schema is maintained.

Do not commit, publish or hand-edit the store. Generated linked-IR bundles are
the portable/public artifacts; reviewed TOML remains the accepted knowledge.
The SQLite WAL requires a local filesystem and is not supported on a network
share. Linux writer and manual-compaction paths enforce this against known
network filesystem types and conservatively reject FUSE because its backend
locality is not visible. Manual compaction also reserves space for a complete
new live pack, the SQLite working set and a fixed safety margin before writing.
Other platforms keep destructive compaction disabled. JSONL and ZIP are
intentionally not cache backends: they do not provide
the indexed point lookup, multi-table atomic binding update and incremental
append/recovery contract required here. Use JSON/JSONL for portable reports and
bundle files, and use revision snapshots plus reviewed TOML to preserve research
across vendor releases.

Human output is not an automation API. Scripts use `--format json` and check
the reported schema.
