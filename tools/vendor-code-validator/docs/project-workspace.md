# Project workspace

A validator project is the stable entry point for repeated analysis. It
composes reusable target knowledge, caller-owned artifact bindings, a memory
map, and optional SVD catalogs without copying those concerns into every
command line.

## Project manifest

`vendor-validator.toml` is shareable and resolves every relative path from its
own directory:

```toml
schema = 1
id = "esp32s31-radio-rev0"
target-spec = "target.spec"
memory-map = "memory.toml"
svd = ["registers/vendor.svd", "registers/reviewed.svd"]

[registers]
facts = "generated/findings/mmio.json"
model = "registers/device.toml"

[registers.svd]
output = "generated/svd/device.svd"

[registers.pac]
output = "generated/pac/src/lib.rs"
target = "none"
edition = "2024"

[interfaces]
facts = "generated/findings/interfaces.json"
pack = "interfaces/reviewed.toml"
semantic-catalogs = [
  "../../tools/vendor-code-validator/catalogs/embedded-semantics.toml",
]
```

`target-spec` selects the architecture and ABI. A target may omit `harness`
for generic artifact and MMIO discovery; commands that use reviewed platform
semantics reject that target with a capability error.

A target spec may provide a `memory-map` fallback for direct `--target-spec`
invocations. A project-level `memory-map` takes precedence. This keeps MMIO
classification independent from clean SVD register names in both invocation
forms.

`run-spec` may be included when the project itself is private. Public projects
normally omit it and use an untracked override:

```console
cargo vendor-code-validator mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run
```

Explicit `--run-spec` and `--svd` arguments override project defaults. The old
`--target-spec` invocation remains supported, but it cannot be combined with
`--project` because that would create two configuration roots.

If the project omits the top-level `svd` key, target-spec SVD catalogs remain
the fallback. An explicit `svd = []` disables that fallback. This is useful
when the schema-2 register model is the complete catalog; non-empty entries
are additional read-only catalogs merged with that model.

When neither option is present, the validator searches the current directory
and its parents for the nearest `vendor-validator.toml`. An explicit
`--project` remains preferable in CI because it makes the configuration root
visible in the command itself.

The optional `[registers]` table establishes a generated/reviewed register
workspace. Its `facts` path becomes the default JSON destination of
`mmio discover`; `model` is a versioned multi-file hardware description.
`registers init-model` bootstraps it from discovery ranges and
`registers import-svd` migrates an existing catalog. Configured SVD and PAC
outputs let export/check commands run without repeating paths. See
[register workspace](register-workspace.md) for the model schema, provenance
boundary and generation workflow.

The optional `[interfaces]` table names the generated structural report from
`interfaces discover`, an optional reviewed `pack`, and zero or more reusable
`semantic-catalogs`. Its `facts` path becomes the default JSON destination.
Interface facts are regenerated; reviewed table names, versions, slot
signatures and semantics do not belong in that file. See
[interface packs](interface-packs.md).

## Project diagnostics

Run the doctor before a long analysis:

```console
cargo vendor-code-validator project doctor \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run
```

It reports backend and harness availability, memory/address-space statistics,
the number of SVD registers, and every local input binding. Existing inputs
are parsed through the selected architecture backend. The doctor counts all
named symbol facts, code definitions, exported definitions, undefined symbols,
object members, and skipped non-object archive members. Missing or incompatible
artifacts make the command fail; an omitted run spec, omitted SVD, omitted
harness, or an artifact with no named symbols is reported as a warning or an
unavailable optional capability.

If `[registers]` is configured, the doctor also distinguishes facts that have
not been generated, a model that has not been initialized, an invalid model,
legacy schema 1 overlays, and a ready schema 2 workspace. Coverage reports
reviewed, ignored, manual and unreviewed registers plus reviewed fields and
configured SVD/PAC outputs.

If `[interfaces]` is configured, the doctor distinguishes missing facts, a
pack that has not been initialized, invalid or stale review, and a ready
workspace. Coverage includes reviewed/ignored/unreviewed anchors and slots,
semantic links, and loaded semantic operations.

From inside a project tree, the short form is sufficient:

```console
cargo vendor-code-validator project doctor
```

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
candidates. An ordinary CMSIS-SVD without the historical
`openEspRadioAddressWindows` extension is accepted; the project memory map is
the primary source of address classification.

```console
cargo vendor-code-validator mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
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
| `image audit-targets` | yes | no | no |
| `symbols inventory` | yes | no | no |
| `interfaces discover` | yes | no | no |
| `interfaces init-pack` / `validate` | no | no | no |
| `registers init-model` / `import-svd` / `validate` / `export-svd` / `generate-pac` | no | no | no |
| `mmio discover` | yes | explicit/project ranges | no |
| `ir export` | yes | optional | optional enrichment |
| execute/compare | yes | yes | no |
| reference/driver/semantic verification | yes | yes | yes |

Without a harness, `ir export` uses a neutral empty contract. It still resolves
ordinary symbols, relocations, direct calls, control flow, memory, and MMIO,
but deliberately assigns no platform semantics to external tables or helper
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
They remain project files rather than knowledge added to the generic backend.
