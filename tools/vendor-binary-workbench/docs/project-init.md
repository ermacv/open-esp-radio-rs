# Creating a workbench project

`project init` creates a self-contained, generic RV32 workspace for repeated
vendor-code analysis. It is the entry point when no target pack or register
catalog exists yet.

```console
cargo vendor-binary-workbench project init \
  --directory verification/vendor/targets/example-radio \
  --id example-radio-rev0 \
  --mmio radio=0x20000000..0x20010000 \
  --source rom \
  --source archive
```

The MMIO intervals are half-open. Repeat `--mmio` for disjoint peripheral
windows and `--source` for independently selectable ELF/archive inputs. With
no `--source`, the initializer creates one profile named `vendor`.

The output directory must not exist. The initializer writes into a sibling
staging directory, loads and cross-validates the generated target, memory map,
project manifest and register model, and only then renames it into place. A
failure leaves no partial project at the requested path.

## Generated ownership

The new directory contains:

```text
vendor-project.toml       shareable analysis and publication configuration
target.toml                 generic RV32/ILP32 target selection
platform.toml               reviewed platform composition, initially neutral
memory.toml                 editable address spaces and MMIO regions
registers/device.toml       schema-2 editable register-model root
registers/peripherals/      one fragment per declared MMIO window
registers/api.toml          schema-2 transactions and public domains (initially empty)
generated/pac/src/generated.rs  derived closed-PAC domains after publication
local.example.toml            template for caller-local artifact bindings
README.md                   project-local bootstrap commands
.gitignore                  local.toml and generated outputs
```

The files have deliberately different owners:

- `memory.toml` classifies addresses. It is authoritative even before any
  register name is known.
- `platform.toml` selects compatible reusable semantic catalogs and optionally
  a compiled harness. The generated pack starts empty.
- `registers/**/*.toml` is reviewed hardware knowledge. Users edit names,
  offsets, fields, access rules and reset metadata there.
- `local.toml` binds source IDs to licensed local ELF/archive paths and remains
  untracked.
- `generated/` contains reproducible findings, reading views, SVD, the raw
  svd2rust backend and bindings. It is never the review database.
- `generated/findings/navigation.json` associates the generated symbol, IR and
  interface facts for manual browsing; it is regenerated, not edited.
- interface and function packs are initialized after their first discovery
  pass. They hold reviewed table ABI, reusable semantic bindings, function
  roles and context layouts.

The initializer does not guess an RTOS, NVS service, logger, delay primitive,
trampoline-table ABI or chip-specific harness. Those are reviewed platform
knowledge, not properties of generic ELF/RISC-V analysis. Add a harness to
the platform pack only when executable comparison or target-specific semantic
enrichment needs it. Attach a reusable pack with `project configure`; see
[platform packs](platform-packs.md).

## Local inputs and first analysis

Create the untracked input manifest through the typed project command:

```console
cargo vendor-binary-workbench project inputs init \
  --project verification/vendor/targets/example-radio/vendor-project.toml \
  --bind source-artifact:rom=/path/to/rom.elf \
  --bind source-artifact:archive=/path/to/linked-archive.elf \
  --bind source-inventory:archive=/path/to/vendor.a \
  --bind source-companion:rom=/path/to/linked-archive.elf \
  --bind source-companion:archive=/path/to/rom.elf

cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/example-radio/vendor-project.toml
```

The command derives required source IDs from `[[analysis.ir]]`, rejects missing
or unknown source bindings, checks each path, verifies that artifact roles are
ELF32 and inventory roles are archives, and atomically writes sibling
`local.toml`. It refuses an existing file unless `--force` is explicit;
`--check` verifies the exact generated content without writing. The generated
`local.example.toml` remains a role reference for manual setups.

Each generated IR profile consumes `source-artifact:ID`. For an archive plus a
fully linked ELF, add the corresponding
`source-inventory:ID` and `source-companion:ID` roles. The archive supplies
candidate members; the linked image remains the authority for final placement
and symbol resolution.

For project commands the resolver selects an explicit `--run-spec` first, then
a manifest `run-spec`, then an existing sibling `local.toml`. Therefore the
normal project-local workflow does not repeat `--run-spec`. Perform the first
discovery and initialize reviewed packs:

```console
cargo vendor-binary-workbench advanced symbols inventory --project PATH/vendor-project.toml
cargo vendor-binary-workbench advanced mmio discover --project PATH/vendor-project.toml
cargo vendor-binary-workbench advanced interfaces discover --project PATH/vendor-project.toml
cargo vendor-binary-workbench advanced ir build --project PATH/vendor-project.toml
cargo vendor-binary-workbench registers review --project PATH/vendor-project.toml
cargo vendor-binary-workbench advanced interfaces init-pack --project PATH/vendor-project.toml
cargo vendor-binary-workbench advanced functions init-pack --project PATH/vendor-project.toml
```

After manual review, `project analyze` refreshes generated analysis evidence,
`project analyze --check` verifies it without writes, and `project publish`
derives the clean SVD, internal raw PAC and binding manifest from reviewed
register data, and emits the configured closed-PAC domain module. The
application-facing PAC is a separate closed crate which includes that module;
typed transaction bridges expect its private raw dependency to be named
`svd` inside the crate. See
[Closed PAC workflow](closed-pac.md).

## Starting with an existing SVD

Use `--import-svd` to make an existing catalog the initial editable model:

```console
cargo vendor-binary-workbench project init \
  --directory verification/vendor/targets/example-radio \
  --id example-radio-rev0 \
  --mmio radio=0x20000000..0x20010000 \
  --import-svd vendor-radio.svd
```

Import is performed in the staging project. Every imported register, including
its complete width, must lie in a declared MMIO region or initialization
fails. The resulting TOML is the reviewed source of truth; later SVD output is
generated from it and excludes discovery paths, function names and review
annotations.

## Current architecture scope

The initializer currently emits `architecture = "riscv32"`,
`calling-convention = "riscv-ilp32"`, `endianness = "little"`, 32-bit
pointers, and defaults the Rust target to
`riscv32imac-unknown-none-elf`. `--rust-target` may select a compatible Rust
triple and `--pac-raw-crate-name` may override the generated raw-PAC import name. A
future architecture initializer should be a separate template selected by an
explicit option; it should not make the generic project silently infer an ABI
from vendor filenames.
