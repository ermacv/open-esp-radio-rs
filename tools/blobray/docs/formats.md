# Formats and schemas

Reviewed configuration is TOML; large generated analysis artifacts are JSON
or JSONL. Persistent machine formats have explicit schema numbers and reject
unknown fields.

## Schema-3 composition inputs

- `target.toml`: architecture, calling convention, endianness, pointer width,
  and Rust target only;
- `ecosystem.toml`: reusable ordered semantic catalogs for a vendor/RTOS
  ecosystem, with no chip addresses or executable provider;
- `chip.toml`: reusable memory map, base register model, SVD inputs, chip
  semantic catalogs, and an optional compiled knowledge-provider ID;
- `vendor-project.toml`: composition, reviewed workspaces, and generated
  output selection; its optional `analysis-provider` selects compiled logic
  that is valid only for this investigation, never reusable chip facts;
- `[reviewed-knowledge].packs`: sparse accepted assertions and vendor-bug
  records with stable IDs, evidence, provenance and applicability;
- local run specification: ignored bindings to caller-owned private artifacts.

`project files` schema 3 exposes the resolved portability layer separately
from edit ownership, so a reviewed file can still be identified as reusable
ecosystem/chip knowledge or as investigation-local review.

The schema-3 project, target, ecosystem, and chip formats are a clean break.
Old inline `memory-map`, `svd`, `platform-pack`, `harness`, and
`semantic-catalogs` keys fail closed; no compatibility shim reinterprets them.
At most one compiled provider may be selected: a project-local
`analysis-provider` conflicts with a chip pack `knowledge-provider` instead of
silently layering executable assumptions from different ownership scopes.

## Other reviewed inputs

- `verification-addon.toml` (`schema = 3`): suites, compiled-artifact
  comparison inputs, declarations, and report paths; it has no executable
  verdict provider and grants no analysis knowledge;
- register/interface/function packs: reviewed assertions and evidence links;
- sparse reviewed-knowledge packs (`schema = 1`): opaque subject/kind/value
  facts and vendor bugs. Empty evidence, hints, duplicate IDs, invalid artifact
  hashes and overlapping assertions fail closed; pack and record applicability
  are intersected rather than overridden;
- disposition manifests: reviewed vendor-to-production binding and claim
  declarations, never execution truth;
- verification policy: required comparisons and bounded properties;
- evidence catalogs: provenance links for reviewed claims.

## Durable revision state

- immutable revision snapshots (`revisions/snapshots/NAME.json`) contain only
  artifact digests and normalized derived features, never vendor payloads or
  disassembly;
- `revisions/ledger.toml` is the tracked schema-1 index for those snapshots.
  It stores only project/revision names, relative snapshot locations, SHA-256
  identities, `baseline`/`current` pointers and an optional update-preflight
  marker.

Snapshots are tool-written correspondence maps rather than manually reviewed
facts. Unlike ordinary generated output, they and their ledger must survive a
vendor update; commit them or place them in equivalent durable,
access-controlled storage. Snapshot names are immutable.

## Generated outputs

- symbol, MMIO, and interface observations;
- canonical derived linked-IR bundles and indexes, including structural loop
  regions, explicitly non-proving counted-loop candidates, and raw-bit
  floating value-flow nodes whose operation and rounding mode remain explicit;
- navigation and review-scope indexes; project-wide call associations retain
  source-qualified candidates and a unique/ambiguous/unresolved status without
  claiming linker resolution;
- pseudo-Rust and executable reference artifacts;
- verification reports and evidence index;
- SVD, raw PAC, bindings index, and restricted API output;
- revision diff and rebase plans;
- research-next reports (`schema_version = 3`), including explicit benefit,
  cost and co-blocker score terms, action-aggregated function identities,
  evidence/destination/completion packets, unique follow-up actions, and every
  related finding grouped below its copyable command.
- project-status reports (`schema = 9`) keep shallow artifact readiness
  separate from generated freshness, open research debt and verification
  readiness; `ready` never means that a review scope has no remaining work.

Generated outputs are disposable and reproducible. They must preserve source
artifact identity/provenance and must not contain proprietary payloads or full
disassembly dumps. A generated file cannot replace its reviewed input.

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
