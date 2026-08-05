# Platform, CLI, and configuration boundaries

## Platform harness

- executable adapters for reviewed external callback contracts;
- global-state regions and typed projections;
- semantic scenario adapters and production Rust-driver bindings;
- target-specific reviewed summaries that are enabled only when the caller
  explicitly selects this harness.

The ESP32-S31 harness is allowed to say `esp32s31` and `phy_param`. Chip table
anchors such as `g_osi_funcs_p`, versions, slot names, and ABI signatures now
belong in a project interface pack rather than variants compiled into the
validator engine. A harness may consume validated bindings when it implements
executable effect contracts, but it must not rediscover or silently override
their layout.

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
versions, slot names, and ABI types. A separate reusable semantic catalog owns
operations and effects such as RTOS event delivery. This keeps
RTOS/NVS/logging/delay vocabularies reusable while leaving chip-specific
addresses and table versions outside the generic engine.

`interfaces discover` is the generic implementation of the first layer. Its
JSON is generated evidence and explicitly claims neither table layout nor
linker resolution nor semantic completeness. See
[`interface-discovery.md`](../../tools/vendor-code-validator/docs/interface-discovery.md).
The implemented review and semantic boundary is documented in
[`interface-packs.md`](../../tools/vendor-code-validator/docs/interface-packs.md).

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
harness esp32s31-radio-v1
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

No expected digest belongs in validator source code, the target pack, or a
local path-binding manifest. A reviewed interface pack may deliberately pin a
layout to a discovery artifact SHA-256; that is a compatibility guard against
applying one table version to another, not authentication of the vendor or
provenance of the file. CI remains responsible for authenticating inputs
before invoking the validator.

Reviewed semantic bindings inherit that trust boundary. Core resolves only an
explicitly selected project pack whose selector and guards match current
facts. A platform harness may then implement stronger effect contracts after
the caller has authenticated the complete input artifact.

The concrete project and memory-map schemas, precedence rules, and command
capabilities are documented in
[`project-workspace.md`](../../tools/vendor-code-validator/docs/project-workspace.md).
