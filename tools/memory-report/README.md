# ELF memory report

`oer-memory-report` is a target-neutral, read-only ELF analyzer. It
keeps two kinds of evidence separate:

- the ELF is authoritative for addresses, section sizes and symbol sizes;
- a project-owned TOML policy is authoritative for ownership, placement
  requirements, reasons and optimization notes.

It never infers that an allocation is DMA-safe merely from a familiar name.
Required policy rules fail closed when their symbol disappears or moves to the
wrong region.

```console
cargo memory report \
  --elf /absolute/path/to/runtime.elf \
  --policy hil/targets/esp32s31/memory/tcp.toml

cargo memory audit --elf ELF --policy POLICY
cargo memory code --elf ELF
cargo memory code-diff --before OLD.ELF --after NEW.ELF
cargo memory mono --input CRATE.mono_items.json
cargo memory diff \
  --before OLD.ELF --after NEW.ELF --policy POLICY
```

Select the retained `runtime.elf` from the completed HIL image or standalone
firmware bundle reported by its builder. Image/network selections and build IDs
change output paths; an old Cargo cache path does not identify the selected
firmware. The policy must match that image's composition.

Every command supports `--format human|json`. `stdout` contains only the
selected report; errors are written to `stderr`.

Stack bounds are not this tool's: the image builds' stack gate
([firmware tooling](../firmware/README.md)) bounds every stack from the image's
machine code with [`oer-riscv-stack`](../riscv/stack/README.md).

The analyzer reports linker-region capacity, allocated sections, explicit
reservations, genuinely unassigned address space, policy-attributed consumers
and the largest unclassified symbols. `diff` compares both region totals and
semantic consumers, which makes buffer changes visible even when mangled Rust
symbols change between builds.

`code` inventories surviving text-symbol ranges in the supplied linked image.
Aliases and overlapping ranges count once; padding and unsized/stripped code
remain unattributed. `code-diff` compares exact text-section totals and retains
before/after address ranges for each complete alias-set identity. Duplicate
names retain all ranges; changed alias sets appear as removed/added rather than
being heuristically merged. Symbol-range deltas are not additive when symbols
overlap, and hash/name changes can prevent matching across builds. Section
deltas, not these diagnostic identities, establish the total image change.
`mono` reads one compiler-generated JSON file produced with
`-Zdump-mono-stats=DIR -Zdump-mono-stats-format=json` on the pinned toolchain.
Its counts and estimates are not linked bytes. Keep build provenance with the
compiler output; neither command rebuilds firmware or adds a mandatory gate.
The pinned compiler can emit a zero-byte file for crates without mono items.
Reports preserve this as `compiler_output_empty`, distinct from an empty JSON
array; malformed nonempty JSON remains an error. Capture retains every file
identity and requires at least one reported definition overall. This is not a
proof of compiler metadata coverage.

