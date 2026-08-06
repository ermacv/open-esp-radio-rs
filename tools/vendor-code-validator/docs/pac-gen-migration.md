# Legacy `pac-gen` retirement matrix

The project manifest is the canonical entry point for register validation and
publication. `tools/pac-gen` is no longer allowed to acquire new semantics. It
remains temporarily so removal can be based on explicit parity rather than on
the current equality of generated files alone.

## Responsibility matrix

| Legacy responsibility | Canonical owner and executable gate | State |
| --- | --- | --- |
| Materialize the checked clean radio SVD | schema-2 model plus `registers export-svd --check` | migrated |
| Strict CMSIS-SVD model validation | pinned `svd-rs` strict builder and pinned encoder | migrated |
| Register names, field ranges and write constraints | `register-model::model_validation`, exercised by `registers validate` | migrated |
| Expanded address alignment and physical overlap | `RegisterModel::register_identities`, exercised by `registers validate` | migrated; aliases fail closed |
| Register/MMIO window containment | project/target memory map plus evidence validation | migrated |
| Provenance source and confidence vocabulary | functional catalogs under `registers/evidence/` | migrated |
| Evidence ranges and their source references | `RegisterEvidenceSet` plus project MMIO regions | migrated |
| Safe compound transactions and ownership helpers | reviewed `registers/api.toml`, cross-validated against the release SVD | migrated |
| PAC source generation | `registers generate-pac --check` | migrated; byte-for-byte parity |
| Address-to-PAC binding index | `registers generate-bindings --check` | migrated; byte-for-byte parity |
| Platform-owned dependency SVD parsing | project SVD catalog loaded by `registers validate` | migrated |
| XML dimension-child ordering | deterministic output of the pinned encoder; the clean generated SVD is the checked artifact | redundant serialization assertion |
| Reject field names containing `PRESERVED` | optional `registers/lints.toml` project policy | migrated |

The generic model deliberately rejects overlapping register views, including
otherwise valid CMSIS-SVD aliases, until an explicit alias model is designed.
That is stricter than silently accepting an unmarked alias and is sufficient
for the current ESP32-S31 catalog, which contains no register aliases.

## Deletion criteria

The legacy crate and `pac-addon.xml` can be removed together when all of these
conditions hold:

1. Keep these canonical gates green:

   ```console
   cargo vendor-code-validator registers validate \
     --project verification/vendor/targets/esp32s31/vendor-validator.toml \
     --deny-unreviewed
   cargo vendor-code-validator registers export-svd \
     --project verification/vendor/targets/esp32s31/vendor-validator.toml \
     --check --deny-unreviewed
   cargo vendor-code-validator registers generate-pac \
     --project verification/vendor/targets/esp32s31/vendor-validator.toml \
     --check --deny-unreviewed
   cargo vendor-code-validator registers generate-bindings \
     --project verification/vendor/targets/esp32s31/vendor-validator.toml \
     --check --deny-unreviewed
   ```

2. In one removal change, delete `tools/pac-gen`, `pac-addon.xml`, the Cargo
   workspace member and alias, and the legacy invocation in
   `tools/audit-source-only.sh`.
3. Run the register-model/validator test suites and the complete source-only
   audit after deletion.

Target-specific RTOS, NVS, logging and delay semantics are unrelated to this
migration. They remain interface/function semantic catalogs and must not move
into the generic register backend or PAC API pack.
