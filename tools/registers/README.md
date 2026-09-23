# Reviewed register publication

`oer-register-tool` validates source-owned hardware descriptions and generates
SVD, raw PAC, restricted PAC API and the binding index. It does not load a Blobray
project, vendor binary, device model or global provider registry.

```console
cargo registers validate --manifest registers/esp32s31/publication/registers.toml
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Omit `--check` to publish an intentional reviewed source change. `--check` is
read-only and fails if any selected output is absent or differs. All four outputs
are prepared before replacement; each file is replaced atomically, but a failure
between replacements reports an incomplete batch. Rerun generation to finish;
there is no claim of atomicity across four directories.

The schema-1 manifest selects model, sparse reviewed assertions, applicability,
memory map, ownership policy, API/lint packs, evidence catalogs and output paths.
Paths are relative to that manifest. No configuration is inferred from a vendor
project. Output parents must exist; aliased destinations and input replacement
are rejected. The source-only context never invents authenticated artifact hashes:
artifact-scoped facts require a different, explicitly authenticated consumer.

[Contracts](contracts/README.md) owns semantic identities, evidence classification
and applicability without execution dependencies. [Review](review/README.md) owns
reviewed-pack validation and conflict selection. [Model](model/README.md) owns
geometry, hardware semantics and PAC policies. The tool loads one model and policy
set, validates their evidence/ownership and renders all outputs from those values.
Reading several source documents is not an atomic filesystem snapshot; the loaded
values, rather than reopened paths, supply generation.

The host owns temporary files and the `rustfmt` process group. Formatting has a
60-second timeout and cancellation reaps the child group. Generated Rust is limited
to 128 MiB before reading it back. These trusted source-generation inputs and
third-party SVD rendering do not claim allocation-controlled binary analysis.
Blobray's separate supervisor remains the owner of untrusted binary investigations.

Publication checks reference validity, selected applicability, reviewed semantics,
memory ranges, API safety and generated source consistency. They do not qualify
hardware or prove equivalence to vendor behavior. Review policy, source provenance
and required recovered tables remain with `registers/`.
