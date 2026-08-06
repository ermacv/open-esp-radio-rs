# Project linked-IR builds

`ir build` turns repeated linked-IR investigations into reproducible project
outputs. The shareable project manifest owns analysis selection and output
paths; a local run spec continues to own private ELF and archive paths.

This is orchestration over the same generic best-effort analysis as
`ir export`. Profiles do not contain RTOS, NVS, logging, delay, register or
vendor-specific semantics.

## Profile format

Declare one or more profiles in `vendor-validator.toml`:

```toml
[[analysis.ir]]
id = "phy"
sources = ["rom", "archive"]
symbol-prefix = "phy_"
include-reachable = true
entry-contract = "none"
output = "generated/findings/phy.ir.json"
pseudo-rust = "generated/reports/phy.pseudo.rs"

[[analysis.ir]]
id = "all-rom"
sources = ["rom"]
output = "generated/findings/rom.ir.json"
```

`id` and `output` are required. Relative outputs are resolved from the project
manifest. Output paths must be unique across every JSON and pseudo-Rust
document.

`sources` selects IDs from `source-artifact:ID` run-spec roles. Omitting it
selects every source artifact. An explicitly present empty array is rejected,
because it would silently analyze nothing. When present, its order controls
the stable artifact order in generated reports; when omitted, run-spec order
is retained.

`symbol-prefix` defaults to the empty prefix, which selects every named code
symbol. `include-reachable` defaults to `true`. `entry-contract` defaults to
`none` and is validated against the selected generic or platform harness.
`pseudo-rust` is optional; schema-v30 JSON is always generated.

## Local artifact bindings

Private paths remain in an untracked run spec:

```text
schema 1
input source-artifact:rom /private/esp32s31-rom.elf
input source-artifact:archive /private/linked-libphy.elf
input source-companion:rom /private/linked-libphy.elf
input source-companion:archive /private/esp32s31-rom.elf
```

When a profile selects one source, its matching `source-companion:ID` and any
global `companion` bindings are supplied to the resolver. A profile selecting
multiple sources analyzes them as independent address spaces and links only
unique exported-symbol edges, matching `ir export` multi-artifact behavior.
Source-specific companions are then unnecessary; a global companion is
rejected because its ownership would be ambiguous.

## Generate and check

Build every configured profile:

```console
cargo vendor-code-validator ir build \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run
```

Select one or more profiles by stable ID:

```console
cargo vendor-code-validator ir build \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run \
  --profile phy
```

All selected profiles are analyzed and rendered in memory before any output is
written. Parent directories are created as needed. CI or a local review can
verify existing outputs without modifying them:

```console
cargo vendor-code-validator ir build \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run \
  --check
```

Missing or different documents make `--check` fail and name every stale path.
Artifact identities and digests remain embedded in the schema-v30 report, so a
successful check also binds the generated view to the supplied local inputs.

## Register-review integration

An IR profile does not automatically become hardware truth or a register-model
input. Link selected generated JSON documents explicitly from the consumer:

```toml
[registers.review]
output = "generated/reports/register-review.md"
linked-ir = ["generated/findings/phy.ir.json"]
```

`project doctor` reports whether each profile has usable source bindings, a
valid entry contract, generated schema-v30 output and an optional pseudo-Rust
document. It also reports whether the JSON output is linked into register
review. The register report still treats functions, predicates and semantic
operations as navigation evidence; clean SVD and PAC generation reads only the
reviewed register model.

Use direct [`ir export`](linked-ir.md) for one-off experiments and project
profiles for outputs that should be named, repeated and checked as part of a
workspace.
