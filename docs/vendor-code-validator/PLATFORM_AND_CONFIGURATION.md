# Platform, CLI, and configuration boundaries

## Platform harness

- target identity and memory map;
- SVD composition;
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

## CLI/orchestrator

- loads a checked target pack and an optional local run manifest;
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
