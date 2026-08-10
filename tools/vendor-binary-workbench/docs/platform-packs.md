# Platform packs and the generic boundary

A platform pack is the explicit composition layer between generic binary
analysis and reviewed platform knowledge. It may select one compiled harness
and supplies reusable semantic catalogs to a project, but it never identifies
a vendor trampoline table or assigns an operation to a slot.

```toml
schema = 1
id = "example-radio-platform-v1"
architecture = "riscv32"
calling-convention = "riscv-ilp32"
harness = "example-radio-v1"
semantic-catalogs = [
  "../catalogs/rtos.toml",
  "../catalogs/storage.toml",
]
```

Catalog paths are relative to the pack, not to a consuming project. This makes
one pack reusable from several projects and vendor revisions. Absolute catalog
paths are rejected because a checked-in pack must remain shareable.

## Layer ownership

| Layer | Owns | Must not own |
| --- | --- | --- |
| Generic backend | ELF/archive linkage, relocations, instructions, CFG, memory and MMIO facts | RTOS names, chip table layouts, vendor revisions |
| Target spec | Architecture, calling convention, endianness, pointer width and Rust target | Reviewed table-slot semantics |
| Platform pack | Target compatibility, optional compiled harness, reusable semantic catalogs | Vendor artifact paths, observed anchors, slot offsets |
| Semantic catalog | Operation vocabulary, argument roles, effects and replacement hints | A claim that a concrete vendor slot implements an operation |
| Interface pack | Reviewed anchors, layout/version guards, slot ABI and explicit semantic bindings | Generic decoder behavior or compiled harness code |
| Function pack | Reviewed function roles and context layouts | Hardware register truth |

This separation allows neutral analysis first. A platform pack enriches the
same IR later; it does not replace structural evidence and cannot make an
unknown indirect call valid merely by naming an operation.

## Attaching a pack

Use `project configure` instead of manually calculating a path relative to the
manifest:

```console
cargo vendor-binary-workbench project configure \
  --project verification/vendor/targets/example-radio/vendor-project.toml \
  --platform-pack tools/vendor-binary-workbench/platform-packs/embedded-riscv.toml
```

The CLI path is resolved from the current working directory. The command
validates the pack, all of its semantic catalogs, target ABI compatibility and
compiled-harness availability before atomically replacing the manifest. It
stores a project-relative path. Repeating the command is idempotent.

Verify the selected configuration without writing:

```console
cargo vendor-binary-workbench project configure \
  --project verification/vendor/targets/example-radio/vendor-project.toml \
  --platform-pack tools/vendor-binary-workbench/platform-packs/embedded-riscv.toml \
  --check
```

`project configure --check` with no selection validates the currently attached
pack. Detach it with `--no-platform-pack`; a project without a platform pack
has no platform harness or reusable semantic operations.

The shipped `embedded-riscv` pack adds the RTOS, NVS, logging and delay
operation vocabulary but deliberately has no compiled chip harness. A project
still has to review its interface facts and bind exact slots to those
operations. Chip-specific packs may add a harness when concrete execution,
entry contracts or proprietary table-shape enrichment requires one.

## Compiled addon registry

Compiled harnesses are entries in one static `HarnessDescriptor` registry.
The normal build has an empty compiled-harness registry. The explicit
`esp32s31-harness` feature alone pulls in the ESP32-S31 ABI fixture, semantic
harness and production PHY/MAC dependencies. Build and test the generic binary
with:

```console
cargo build --manifest-path tools/vendor-binary-workbench/Cargo.toml \
  --no-default-features
cargo test --manifest-path tools/vendor-binary-workbench/Cargo.toml \
  --no-default-features --lib
```

Enable the compiled ESP32-S31 addon explicitly:

```console
cargo build -p open-radio-vendor-binary-workbench \
  --features esp32s31-harness
cargo vendor-binary-workbench-esp32s31 project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Generic project, artifact, MMIO and IR operations remain available; selecting
a platform pack whose harness was not compiled in fails during resolution,
before expensive analysis stages run. Adding a compiled addon
therefore requires a feature and one registry descriptor, not changes to the
generic backend or a dynamic ABI plugin protocol.

## Composition rules

Pack architecture and calling convention must exactly match the target. A
harness is selected only by the platform pack; `target.toml` is generic and a
`harness` key there is rejected. Commands that require target-specific
execution or verification therefore require a project with an appropriate
platform pack. Generic artifact and IR workflows may still use an explicit
target spec for backend development.

Multiple semantic catalogs may be composed. Duplicate paths and duplicate
operation IDs are rejected. A pack does not pin artifact hashes: revision and
layout guards belong to the project-specific interface pack because they are
claims about observed vendor bytes, not reusable platform vocabulary.
