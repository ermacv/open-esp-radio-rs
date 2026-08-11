# Archived: retired `pac-gen` responsibility matrix

This document records a completed migration and is not part of the current
workflow contract.

The project manifest is the canonical entry point for register validation and
publication. The legacy `tools/pac-gen` crate and `pac-addon.xml` were removed
after every responsibility below had an executable project-owned replacement.

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
| Raw PAC source generation | `registers generate-pac-raw --check` | migrated; byte-for-byte parity |
| Address-to-PAC binding index | `registers generate-bindings --check` | migrated; byte-for-byte parity |
| Platform-owned dependency SVD parsing | project SVD catalog loaded by `registers validate` | migrated |
| XML dimension-child ordering | deterministic output of the pinned encoder; the clean generated SVD is the checked artifact | redundant serialization assertion |
| Reject field names containing `PRESERVED` | optional `registers/lints.toml` project policy | migrated |

The generic model deliberately rejects overlapping register views, including
otherwise valid CMSIS-SVD aliases, until an explicit alias model is designed.
That is stricter than silently accepting an unmarked alias and is sufficient
for the current ESP32-S31 catalog, which contains no register aliases.

## Current publication gate

Keep the canonical project gate green:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

This one command performs strict register/API/evidence validation and verifies
all configured SVD, PAC and binding outputs. The individual `registers`
commands remain the narrow debugging and override interface.

Target-specific RTOS, NVS, logging and delay semantics are unrelated to this
migration. They remain interface/function semantic catalogs and must not move
into the generic register backend or PAC API pack.
