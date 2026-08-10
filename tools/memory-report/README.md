# ELF memory report

`open-esp-radio-memory-report` is a target-neutral, read-only ELF analyzer. It
keeps two kinds of evidence separate:

- the ELF is authoritative for addresses, section sizes and symbol sizes;
- a project-owned TOML policy is authoritative for ownership, placement
  requirements, reasons and optimization notes.

It never infers that an allocation is DMA-safe merely from a familiar name.
Required policy rules fail closed when their symbol disappears or moves to the
wrong region.

```console
cargo memory report \
  --elf target/hil/esp32s31/psram-code-psram-data-open-radio-tcp/cargo/runtime/riscv32imafc-unknown-none-elf/release/open-esp-radio-hil-esp32s31-runtime \
  --policy hil/targets/esp32s31/memory/tcp.toml

cargo memory audit --elf ELF --policy POLICY
cargo memory stack --elf ELF --policy hil/targets/esp32s31/stack.toml
cargo memory diff \
  --before OLD.ELF --after NEW.ELF --policy POLICY
```

Every command supports `--format human|json`. `stdout` contains only the
selected report; errors are written to `stderr`.

`stack` requires compiler-emitted `.stack_sizes` metadata. Target policy owns a
review threshold, a hard per-frame limit, the compiler move limit and runtime
headroom. Generated async `poll` functions are included. Local frames do not
prove indirect call chains, so HIL stack painting remains independently
mandatory.

Every frame above the review threshold must match a `reviewed_frames` policy
entry and stay below its individual ceiling. A new large frame or growth of a
reviewed frame therefore fails the build instead of producing an unactioned
warning.

The analyzer reports linker-region capacity, allocated sections, explicit
reservations, genuinely unassigned address space, policy-attributed consumers
and the largest unclassified symbols. `diff` compares both region totals and
semantic consumers, which makes buffer changes visible even when mangled Rust
symbols change between builds.
