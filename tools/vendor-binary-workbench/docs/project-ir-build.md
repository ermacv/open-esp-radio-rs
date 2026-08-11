# Project linked-IR builds

`ir build` turns repeated linked-IR investigations into reproducible project
outputs. The shareable project manifest owns analysis selection and output
paths; a local run spec continues to own private ELF and archive paths.

This is orchestration over the same generic best-effort analysis as
`ir export`. Profiles do not contain RTOS, NVS, logging, delay, register or
vendor-specific semantics.

## Profile format

Declare one or more profiles in `vendor-project.toml`:

```toml
[[analysis.ir]]
id = "phy"
sources = ["rom", "archive"]
roots = "symbol-prefix"
symbol-prefix = "phy_"
include-reachable = true
entry-contract = "none"
output = "generated/findings/phy.ir"

[[analysis.ir]]
id = "all-rom"
sources = ["rom"]
roots = "all"
output = "generated/findings/rom.ir"
```

`id` and `output` are required. Relative outputs are resolved from the project
manifest. Output paths must be unique across linked-IR bundles.

`sources` selects IDs from `source-artifact:ID` run-spec roles. Omitting it
selects every source artifact. An explicitly present empty array is rejected,
because it would silently analyze nothing. When present, its order controls
the stable artifact order in generated reports; when omitted, run-spec order
is retained.

`roots` is required: `"all"` selects every named code symbol, while
`"symbol-prefix"` requires a non-empty `symbol-prefix`. This keeps full-file
analysis explicit instead of encoding it as an empty prefix. `include-reachable`
defaults to `true`. `entry-contract` defaults to
`none` and is validated against the selected generic or platform harness.
The schema-v47 random-access bundle stores pseudo-Rust with each function.
Project-wide concatenated pseudo files were removed: use `inspect function`,
the TUI, or a focused `ir export --symbol-prefix ... --pseudo-rust ...`.

## Local artifact bindings

Private paths remain in an untracked run spec:

```toml
schema = 1

[[inputs]]
role = "source-artifact:rom"
path = "/private/esp32s31-rom.elf"

[[inputs]]
role = "source-artifact:archive"
path = "/private/linked-libphy.elf"

[[inputs]]
role = "source-companion:rom"
path = "/private/linked-libphy.elf"

[[inputs]]
role = "source-companion:archive"
path = "/private/esp32s31-rom.elf"
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
cargo vendor-binary-workbench ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml
```

Select one or more profiles by stable ID:

```console
cargo vendor-binary-workbench ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --profile phy
```

Profiles are processed sequentially. Each bundle member is streamed into a
private sibling staging directory with a 512-MiB safety limit; no serialized
copy of the complete bundle is retained in memory. Write mode replaces the
complete output directory only after all members succeed. Check mode compares
the staged result in fixed-size buffers and never reads a large generated file
into a `String`.

Before catalogs are loaded or analysis starts, every selected artifact,
inventory and companion is checked as a regular file. A missing generated ELF
therefore fails with the exact profile, run-spec role and path instead of a
late anonymous object-reader `ENOENT`.

```console
cargo vendor-binary-workbench ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --check
```

Missing or different documents make `--check` fail and name every stale path.
Artifact identities and digests remain embedded in the schema-v47 report, so a
successful check also binds the generated view to the supplied local inputs.

## Command result formats

The default human view summarizes each selected profile, its function,
register and field-candidate counts, and the generated paths. `--format json`
and `--format jsonl` emit the typed `ir-build`
report directly, with schema, mode, status, ordered profiles and document count. The
generated linked-IR bundle remains a separate schema-v47 project artifact; the
command result only describes the build operation.

## Register-review integration

An IR profile does not automatically become hardware truth or a register-model
input. Link selected generated bundle directories explicitly from the consumer:

```toml
[registers.review]
output = "generated/reports/register-review.md"
linked-ir = ["generated/findings/phy.ir"]
```

`project doctor` reports whether each profile has usable source bindings and a
valid schema-v47 output. It also reports whether the bundle is linked into
register review. The register report still treats functions, predicates and semantic
operations as navigation evidence; clean SVD and PAC generation reads only the
reviewed register model.

Use direct [`ir export`](linked-ir.md) for one-off experiments and project
profiles for outputs that should be named, repeated and checked as part of a
workspace.
