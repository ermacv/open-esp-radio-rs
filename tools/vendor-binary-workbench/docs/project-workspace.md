# Project workspace

A workbench project is the stable entry point for repeated analysis. It
composes reusable target knowledge, caller-owned artifact bindings, a memory
map, an optional platform pack, and optional SVD catalogs without copying
those concerns into every command line.

## Creating a project

For a new RV32 target, create the manifest, target, memory map and schema-2
register workspace together:

```console
cargo vendor-binary-workbench project init \
  --directory verification/vendor/targets/example-radio \
  --id example-radio-rev0 \
  --mmio radio=0x20000000..0x20010000 \
  --source rom --source archive
```

Initialization is non-overwriting and validates the complete scaffold before
publishing the directory. See [creating a workbench project](project-init.md)
for the generated tree, SVD import, local-input bootstrap and generic/platform
semantic boundary.

## Project manifest

`vendor-project.toml` is shareable and resolves every relative path from its
own directory:

```toml
schema = 1
id = "esp32s31-radio-rev0"
target-spec = "target.spec"
platform-pack = "platform.toml"
memory-map = "memory.toml"
svd = ["registers/vendor.svd", "registers/reviewed.svd"]

[[analysis.ir]]
id = "vendor"
sources = ["rom", "archive"]
symbol-prefix = "phy_"
include-reachable = true
output = "generated/findings/vendor.ir.json"
pseudo-rust = "generated/reports/vendor.pseudo.rs"

[registers]
facts = "generated/findings/mmio.json"
model = "registers/device.toml"

[registers.review]
output = "generated/reports/register-review.md"
linked-ir = ["generated/findings/vendor.ir.json"]

[registers.svd]
output = "generated/svd/device.svd"

[registers.pac]
output = "generated/pac/src/lib.rs"
target = "none"
edition = "2024"

[registers.bindings]
output = "generated/svd/device.bindings"
crate-name = "device_pac"

[registers.api]
pack = "registers/api.toml"

[registers.evidence]
catalogs = ["registers/evidence.toml"]

[interfaces]
facts = "generated/findings/interfaces.json"
pack = "interfaces/reviewed.toml"

[functions]
pack = "functions/reviewed.toml"
profiles = ["vendor"]

[functions.review]
output = "generated/reports/function-review.md"
```

`target-spec` selects the architecture and ABI. It does not select a platform
harness; commands that use reviewed platform semantics require a project with
a platform pack.

The optional `platform-pack` composes an ABI-matched compiled harness and
reusable semantic catalogs above that generic target. Its catalog paths are
relative to the pack so the same pack can serve several projects. Use
`project configure` to attach, verify or detach it safely. Interface catalogs
and harness selection belong only to this layer; `[interfaces]` owns generated
facts and reviewed bindings. See [platform packs](platform-packs.md).

A target spec may provide a `memory-map` fallback for direct `--target-spec`
invocations. A project-level `memory-map` takes precedence. This keeps MMIO
classification independent from clean SVD register names in both invocation
forms.

`run-spec` may be included when the project itself is private. Public projects
normally omit it and use an untracked override:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

Explicit `--run-spec` and `--svd` arguments override project defaults. The old
`--target-spec` invocation remains supported, but it cannot be combined with
`--project` because that would create two configuration roots.

Optional `[[analysis.ir]]` profiles give generated linked IR stable IDs,
source selection and project-relative destinations. `ir build` generates all
profiles from `source-artifact:ID` bindings in the local run spec; `--check`
verifies that existing JSON and pseudo-Rust documents match. See
[project linked-IR builds](project-ir-build.md) for the schema and companion
rules.

If the project omits the top-level `svd` key, target-spec SVD catalogs remain
the fallback. An explicit `svd = []` disables that fallback. This is useful
when the schema-2 register model is the complete catalog; non-empty entries
are additional read-only catalogs merged with that model.

When neither option is present, the workbench searches the current directory
and its parents for the nearest `vendor-project.toml`. An explicit
`--project` remains preferable in CI because it makes the configuration root
visible in the command itself.

The optional `[registers]` table establishes a generated/reviewed register
workspace. Its `facts` path becomes the default JSON destination of
`mmio discover`; `model` is a versioned multi-file hardware description.
`registers init-model` bootstraps it from discovery ranges,
`registers import-svd` migrates an existing catalog, and `registers review`
joins generated functions/write patterns to reviewed model identities without
copying them into the model. Optional `linked-ir` reports add poll, predicate
and guarded semantic-action evidence to that generated review view. Configured
review, SVD and PAC outputs let generation/check commands run without repeating
paths. See
[register workspace](register-workspace.md) for the model schema, provenance
boundary and generation workflow.

The optional `[interfaces]` table names the generated structural report from
`interfaces discover`, an optional reviewed `pack`, and zero or more reusable
`semantic-catalogs`. Its `facts` path becomes the default JSON destination.
Interface facts are regenerated; reviewed table names, versions, slot
signatures and semantics do not belong in that file. See
[interface packs](interface-packs.md).

The optional `[functions]` table selects linked-IR profiles and names a
human-edited pack for function roles and observed context layouts. Optional
`[functions.review].output` is a generated Markdown reading view that combines
those reviewed names with pseudo-code and linked evidence. If a validated
interface pack is present, function review also joins its bindings as
explicitly association-only navigation links. Register semantics
remain in the register model, while trampoline ABI and RTOS/NVS/logging/delay
semantics remain in interface and semantic packs. See
[function and context packs](function-packs.md).

## Project-wide generation

Once the manifest and local run spec are ready, all configured generated
evidence can be refreshed with one command:

```console
cargo vendor-binary-workbench project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

`project analyze --check` repeats the analyses without writing and verifies
byte-stable MMIO facts, interface facts, linked IR, pseudo-Rust, register
review, and function review. Both modes then validate the reviewed
register/interface/function workspaces read-only.
They intentionally exclude SVD and PAC publication. Publish reviewed register
outputs separately, without a private run spec:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

See the separate [project analysis pipeline](project-pipeline.md) and
[project publication pipeline](project-publication.md) for their dependency
and failure models.

## Project diagnostics

Run the doctor before a long analysis:

```console
cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

It reports backend and harness availability, memory/address-space statistics,
the number of SVD registers, configured IR-profile readiness, and every local
input binding. Existing inputs
are parsed through the selected architecture backend. The doctor counts all
named symbol facts, code definitions, exported definitions, undefined symbols,
object members, and skipped non-object archive members. Missing or incompatible
artifacts make the command fail; an omitted run spec, omitted SVD, omitted
harness, or an artifact with no named symbols is reported as a warning or an
unavailable optional capability.

If `[registers]` is configured, the doctor also distinguishes facts that have
not been generated, a model that has not been initialized, an invalid model,
and a ready schema-2 workspace. Coverage reports
reviewed, ignored, manual and unreviewed registers plus reviewed fields and
configured review/SVD/PAC outputs. Configured linked-IR review inputs are parsed
as schema-v32 reports and their register/field-candidate counts are reported;
missing outputs owned by `[[analysis.ir]]` are reported as not generated,
while missing external inputs or incompatible existing reports are errors
rather than silently disabling enrichment.

If `[interfaces]` is configured, the doctor distinguishes missing facts, a
pack that has not been initialized, invalid or stale review, and a ready
workspace. Coverage includes reviewed/ignored/unreviewed anchors and slots,
semantic links, and loaded semantic operations.

If `[functions]` is configured, the doctor checks selected IR outputs, strict
schema-v32 facts, artifact provenance guards, the pack lifecycle, review
coverage for root functions/contexts/fields, explicitly accepted incomplete
evidence, and the configured generated report destination.

From inside a project tree, the short form is sufficient:

```console
cargo vendor-binary-workbench project doctor
```

For automation, `project status` expresses the same project as five stable
readiness phases and can write deterministic schema-1 JSON. It distinguishes a
valid but incomplete analysis workspace from invalid configuration and from a
publication-ready register workspace. See
[project status and lifecycle readiness](project-status.md).

## Memory map

The memory map is independent of CMSIS-SVD. It declares address spaces and the
classification of regions even when no register names are known:

```toml
schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[regions]]
name = "radio"
address-space = "cpu"
kind = "mmio"
start = 0x20100000
end-exclusive = 0x20200000
permissions = "rw"
```

Supported region kinds are `code`, `rodata`, `ram`, `mmio`, `device`, and
`unknown`. Ranges are half-open. MMIO regions default to `volatile = true`;
other regions default to false. `permissions` contains each of `r`, `w`, and
`x` at most once.

Overlaps are rejected. An intentional exact alias must be declared instead of
being accepted silently:

```toml
[[regions]]
name = "radio-alias"
address-space = "cpu"
kind = "mmio"
start = 0x20100000
end-exclusive = 0x20200000
permissions = "rw"
alias-of = "radio"
```

The current RISC-V backend consumes MMIO regions from the default address
space as 32-bit windows. Keeping the source model address-space-aware avoids
encoding that backend restriction into the project format.

## Discovery defaults

For `mmio discover`, project MMIO regions become `--range` defaults when no
explicit range is present. SVD remains optional: without it, findings use
`UNMAPPED` names while retaining addresses, widths, users, and write-pattern
candidates. CMSIS-SVD contains hardware description only; the project memory
map is the source of address classification.

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run \
  --json-report generated/findings/mmio.json
```

Supplying any explicit `--range` suppresses the project range defaults for
that invocation. This makes a narrow investigation reproducible and prevents
an implicit scan of unrelated regions.

## Capability boundary

Commands now request the knowledge they actually consume:

| Command family | Backend | MMIO map | Platform harness |
| --- | --- | --- | --- |
| `project init` | no existing backend | creates and validates it | no |
| `project configure` | validates target compatibility | no | optional pack selection |
| `project status` | optional artifact inspection | reads and reports it | optional enrichment |
| `image audit-targets` | yes | no | no |
| `symbols inventory` | yes | no | no |
| `interfaces discover` | yes | no | no |
| `interfaces init-pack` / `validate` | no | no | no |
| `functions init-pack` / `validate` / `review` | no | no | no |
| `registers init-model` / `import-svd` / `review` / `export-svd` / `generate-pac` / `generate-bindings` | no | no | no |
| `registers validate` | no | optional containment/evidence validation | no |
| `mmio discover` | yes | explicit/project ranges | no |
| `ir export` | yes | optional | optional enrichment |
| `ir build` | yes | optional | optional enrichment |
| `project analyze [--check]` | yes | required by MMIO stage | optional IR enrichment |
| `project publish` | no | validation only | no |
| execute/compare | yes | yes | no |
| reference/driver/semantic verification | yes | yes | yes |

Without a harness, `ir export` and `ir build` use a neutral empty contract. They resolve
ordinary symbols, relocations, direct calls, control flow, memory, and MMIO,
but deliberately assign no platform semantics to external tables or helper
functions. A configured harness enriches the same lower-level IR.

## Current and future ownership

The project separates generated artifact/register facts from local bindings
and reviewed metadata.
See [artifact and symbol inventory](symbol-inventory.md) for the exact boundary
between archive candidates, fully linked ELF truth, and semantic packs. The
[register workspace](register-workspace.md) implements the equivalent boundary
for MMIO facts, reviewed fields, and SVD. Generated interface facts are
described in [interface discovery](interface-discovery.md); reviewed layouts
and reusable semantics are described in [interface packs](interface-packs.md).
Reviewed function roles and context names are described in
[function and context packs](function-packs.md).
They remain project files rather than knowledge added to the generic backend.
