# Source-only radio register publication

This composition publishes the [reviewed register model](../README.md), its
reviewed hardware assertions and the production PAC ownership/API pack. It
selects the reusable chip provider, independently of the
[vendor investigation](../../../verification/vendor/projects/esp32s31/vendor-project.toml)
and its artifact-specific models and memory-access facts.

Run from the repository root without private artifacts or a run spec:

```console
cargo blobray registers validate --project registers/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers export-svd --check --project registers/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers generate-pac-raw --check --project registers/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers generate-pac-api --check --project registers/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers generate-bindings --check --project registers/esp32s31/publication/vendor-project.toml --format json
```

The four generation commands check SVD, raw PAC, closed PAC API and binding
index respectively. `project publish` retains its separate requirement for
structural investigation review; it does not fall back to this model-only flow.

Omit `--check` to publish an intentionally reviewed change. The generated SVD,
binding index and both Rust PAC files are the same outputs selected by the
full investigation. Common MMIO scope lives in
[`../policy/ownership.toml`](../policy/ownership.toml); model, API, reviewed facts
and provider selection remain explicit manifest references.

An absent MMIO findings file means model-only publication. Such a check proves
that generated source matches the reviewed model; it does not qualify hardware
behavior or certify vendor comparison. This manifest explicitly selects the
same shared lint pack and nine reviewed evidence catalogs as the investigation.
Validation checks the lint policy, catalog schemas, provenance references from
the model and PAC API, and evidence ranges against the memory map. Missing,
invalid or inconsistent selected inputs fail validation.

These are source consistency checks: descriptions of vendor artifacts in the
catalogs do not cause those artifacts to be opened or authenticated. The
explicit references preserve source policy without inheriting investigation
models or weakening their separate artifact-authentication requirements.

Binary investigation remains explicit:

```console
cargo blobray registers validate --project verification/vendor/projects/esp32s31/vendor-project.toml --run-spec /absolute/path/to/local.toml --format json
```

That context authenticates exact artifact constraints. Selecting this
publication context cannot authorize binary facts from another provider.
