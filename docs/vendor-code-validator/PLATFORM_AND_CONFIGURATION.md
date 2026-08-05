# Platform, CLI, and configuration boundaries

## Platform harness

- external callback-table descriptions;
- mutable pointer-cell and function-table entry contracts;
- global-state regions and typed projections;
- semantic scenario adapters and production Rust-driver bindings;
- target-specific reviewed summaries that are enabled only when the caller
  explicitly selects this harness.

The ESP32-S31 harness is allowed to say `esp32s31`, `phy_param` and
`g_osi_funcs_p`. Those names are an error in core or the RISC-V backend.
Versions and layouts are data owned by the harness, not variants compiled into
the validator engine.

The harness does not own the project memory map or decide which artifacts are
loaded. An architecture-only target may omit the harness for generic MMIO and
IR discovery. In that mode no external-table or RTOS semantics are invented.

## Target and project

The reusable target pack selects the architecture, calling convention,
endianness, pointer width, and optional semantic harness. The project composes
that target with a caller-owned memory map, SVD register catalogs, and local
artifact bindings. This keeps physical address classification usable before a
chip harness or complete SVD exists.

Artifact symbol tables are also platform-neutral. `symbols inventory` retains
binding, visibility, type, section, definition state, archive member, and
cross-input candidates without invoking harness semantics. A unique candidate
in another input is a navigation association, not a claim that the linker
selected it. Exact resolution belongs to the fully linked ELF.

Trampoline recognition has a two-layer contract. Core may recover an aligned
pointer table, a constant slot offset, an indirect call, and argument value
flow. A selected semantic pack or platform harness owns table anchors, layout
versions, slot names, ABI types, and effects such as RTOS event delivery. This
keeps RTOS/NVS/logging/delay vocabularies reusable while leaving chip-specific
addresses and table versions outside the generic engine.

`interfaces discover` is the generic implementation of the first layer. Its
JSON is generated evidence and explicitly claims neither table layout nor
linker resolution nor semantic completeness. See
[`interface-discovery.md`](../../tools/vendor-code-validator/docs/interface-discovery.md).

## CLI/orchestrator

- loads a project manifest as the stable composition root;
- loads a checked target pack and an optional local run manifest;
- loads address spaces and memory regions independently of SVD;
- binds input roles to paths supplied by the caller;
- selects an architecture backend and validates its ABI;
- invokes one workflow and renders text/JSON reports.

The CLI does not derive `_oracles` from `CARGO_MANIFEST_DIR`. Proprietary input
paths are supplied as ordinary command options, environment variables in an
external script, or a local untracked run manifest. Checked target packs
contain no proprietary paths.

## Configuration boundaries

A checked target pack contains public validation knowledge:

```text
schema 1
target esp32s31-rev0
harness esp32s31-phy-v1
architecture riscv32
calling-convention riscv-ilp32
endianness little
pointer-width 32
rust-target riscv32imafc-unknown-none-elf
svd esp32s31-radio.svd
svd esp32s31-platform-radio-deps.svd
```

The public target pack does not bind artifacts. A local run spec does:

```text
schema 1
input rom-artifact /absolute/path/to/rom.elf
input archive-artifact /absolute/path/to/linked-vendor.elf
input archive-inventory /absolute/path/to/libphy.a
input rust-artifact /absolute/path/to/rust-probes.elf
```

No expected digest belongs in either validator source code or a binding
manifest. A CI job that requires an exact vendor revision authenticates those
files before invoking the validator. Content digests in generated reports are
descriptive evidence, not an acceptance policy.

Reviewed semantic summaries inherit that trust boundary. Core never chooses a
summary from a vendor digest. The explicitly selected platform harness chooses
its summary by target, symbol and structural identity after the caller has
authenticated the complete input artifact.

The concrete project and memory-map schemas, precedence rules, and command
capabilities are documented in
[`project-workspace.md`](../../tools/vendor-code-validator/docs/project-workspace.md).
