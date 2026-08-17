# Formats and schemas

Reviewed configuration is TOML; large generated analysis artifacts are JSON
or JSONL. Persistent machine formats have explicit schema numbers and reject
unknown fields.

## Schema-3 composition inputs

- `target.toml`: architecture, calling convention, endianness, pointer width,
  and Rust target only;
- `ecosystem.toml`: reusable ordered semantic catalogs for a vendor/RTOS
  ecosystem, with no chip addresses or executable provider;
- `chip.toml`: reusable memory map, SVD inputs, chip semantic catalogs, and an
  optional compiled knowledge-provider ID;
- `vendor-project.toml`: composition, reviewed workspaces, and generated
  output selection; it does not own reusable chip facts;
- local run specification: ignored bindings to caller-owned private artifacts.

The schema-3 project, target, ecosystem, and chip formats are a clean break.
Old inline `memory-map`, `svd`, `platform-pack`, `harness`, and
`semantic-catalogs` keys fail closed; no compatibility shim reinterprets them.

## Other reviewed inputs

- `verification-addon.toml` (`schema = 3`): suites, compiled-artifact
  comparison inputs, declarations, and report paths; it has no executable
  verdict provider and grants no analysis knowledge;
- register/interface/function packs: reviewed assertions and evidence links;
- disposition manifests: reviewed vendor-to-production binding and claim
  declarations, never execution truth;
- verification policy: required comparisons and bounded properties;
- evidence catalogs: provenance links for reviewed claims.

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
- SVD, raw PAC, bindings index, and restricted API output.

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
share.

Human output is not an automation API. Scripts use `--format json` and check
the reported schema.
