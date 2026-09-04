# Source-only radio register publication

This project publishes the checked ESP32-S31 chip register model, its reviewed
register semantics, and the target PAC ownership/API pack. It explicitly selects
the chip provider. It does not select the binary investigation provider from
`../vendor-project.toml`, whose exact memory-access facts require authenticated
vendor inputs.

From the repository root, the publication can be checked without private
artifacts or a run spec:

```console
cargo blobray registers validate --project verification/vendor/targets/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers export-svd --check --project verification/vendor/targets/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers generate-pac-raw --check --project verification/vendor/targets/esp32s31/publication/vendor-project.toml --format json
cargo blobray registers generate-bindings --check --project verification/vendor/targets/esp32s31/publication/vendor-project.toml --format json
```

Omit `--check` to publish an intentionally reviewed change. The outputs are the
same checked-in SVD, raw PAC and bindings used by the full investigation project.
The model, reviewed facts, and API pack remain in their existing ownership
locations; this manifest does not duplicate those facts. Its named ownership
ranges preserve the target project's publication scope.

An absent MMIO findings file means model-only publication. Such a check proves
that generated source matches the reviewed model; it does not qualify hardware
behavior or certify vendor comparison. Binary investigation remains explicit:

```console
cargo blobray registers validate --project verification/vendor/targets/esp32s31/vendor-project.toml --run-spec /absolute/path/to/local.toml --format json
```

That context continues to authenticate exact artifact constraints. Selecting a
publication context cannot authorize exact binary facts from another provider.
