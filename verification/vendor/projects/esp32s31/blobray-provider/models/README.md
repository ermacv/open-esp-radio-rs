# ESP32-S31 executable reconstruction provider

This crate owns temporary handwritten control flow, intrinsic interpretation,
body applicability checks, and composition with the reusable chip runtime
model provider. Declarative semantic meaning, RAM classifications and ABI
contracts remain in sibling knowledge/contracts crates; dependency direction
is models → knowledge, with no dependency in reverse.

`PROVIDER.kind = ManualReconstruction` makes this implementation's nature
explicit to the host and `project doctor`. Its applicability/evidence fields
are provenance metadata, not accepted facts or an equivalence verdict. Hooks
must independently check the exact body and required context before returning
a reconstruction. A mismatch preserves structural analysis fallback.

RFPLL polling/search, IQ polling, I2C operation sequences, scratch lifetime and
assertion branching remain temporary executable models. Moving them here does
not make them generated analysis. Replacing them with generic instruction
reconstruction is a separate backend task; their current exact-body guards,
entry checks and regression tests are retained during that transition.

The host registers this provider separately from declarative knowledge.
Its revision participates in project cache identity, and registry validation
requires the same model ID/revision in the function-analysis harness domain.
The base model provider is included in both composition identities.

Run source-only regressions with:

```console
cargo test -p open-radio-vendor-models-esp32s31 --lib
```

The optional authenticated-ROM mutation test additionally requires a
caller-owned `BLOBRAY_REVIEWED_ROM` path and checks the reviewed whole-image
SHA before reading any body. See [`../OWNERSHIP.md`](../OWNERSHIP.md) for
applicability and the reason no DTM caller-domain bound is supplied.
