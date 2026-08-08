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
target.spec                 generic RV32/ILP32 target selection
platform.toml               reviewed platform composition, initially neutral
memory.toml                 editable address spaces and MMIO regions
registers/device.toml       schema-2 editable register-model root
registers/peripherals/      one fragment per declared MMIO window
run.spec.example            template for caller-local artifact bindings
README.md                   project-local bootstrap commands
.gitignore                  local.run and generated outputs
```

The files have deliberately different owners:

- `memory.toml` classifies addresses. It is authoritative even before any
  register name is known.
- `platform.toml` selects compatible reusable semantic catalogs and optionally
  a compiled harness. The generated pack starts empty.
- `registers/**/*.toml` is reviewed hardware knowledge. Users edit names,
  offsets, fields, access rules and reset metadata there.
- `local.run` binds source IDs to licensed local ELF/archive paths and remains
  untracked.
- `generated/` contains reproducible findings, reading views, SVD, PAC and
  bindings. It is never the review database.
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

Copy the generated template and replace each path:

```console
cp verification/vendor/targets/example-radio/run.spec.example \
  verification/vendor/targets/example-radio/local.run

cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/example-radio/vendor-project.toml \
  --run-spec verification/vendor/targets/example-radio/local.run
```

Each generated IR profile consumes `source-artifact:ID`. For an archive plus a
fully linked ELF, extend the local run spec with the corresponding
`source-inventory:ID` and `source-companion:ID` roles. The archive supplies
candidate members; the linked image remains the authority for final placement
and symbol resolution.

Then perform the first discovery and initialize reviewed packs:

```console
cargo vendor-binary-workbench symbols inventory --project PATH/vendor-project.toml --run-spec PATH/local.run
cargo vendor-binary-workbench mmio discover --project PATH/vendor-project.toml --run-spec PATH/local.run
cargo vendor-binary-workbench interfaces discover --project PATH/vendor-project.toml --run-spec PATH/local.run
cargo vendor-binary-workbench ir build --project PATH/vendor-project.toml --run-spec PATH/local.run
cargo vendor-binary-workbench registers review --project PATH/vendor-project.toml
cargo vendor-binary-workbench interfaces init-pack --project PATH/vendor-project.toml
cargo vendor-binary-workbench functions init-pack --project PATH/vendor-project.toml
```

After manual review, `project analyze` refreshes generated analysis evidence,
`project analyze --check` verifies it without writes, and `project publish`
derives the clean SVD, Rust PAC and binding manifest from reviewed register
data.

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

The initializer currently emits `architecture riscv32`, `riscv-ilp32`, little
endian, 32-bit pointers, and defaults the Rust target to
`riscv32imac-unknown-none-elf`. `--rust-target` may select a compatible Rust
triple and `--pac-crate-name` may override the generated PAC import name. A
future architecture initializer should be a separate template selected by an
explicit option; it should not make the generic project silently infer an ABI
from vendor filenames.
