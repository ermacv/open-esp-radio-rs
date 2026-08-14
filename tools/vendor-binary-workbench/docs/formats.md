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
- canonical derived linked-IR bundles and indexes;
- navigation and review-scope indexes;
- pseudo-Rust and executable reference artifacts;
- verification reports and evidence index;
- SVD, raw PAC, bindings index, and restricted API output.

Generated outputs are disposable and reproducible. They must preserve source
artifact identity/provenance and must not contain proprietary payloads or full
disassembly dumps. A generated file cannot replace its reviewed input.

Human output is not an automation API. Scripts use `--format json` and check
the reported schema.
