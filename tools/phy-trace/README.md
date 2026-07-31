# Compiled PHY parity verifier

`open-esp-radio-phy-trace` derives MMIO behavior from RISC-V ELF files and
archive members and resolves every physical register through the radio SVD. It
parses ELF/ar containers with the Rust `object` reader and decodes RV32IMAC
instructions directly from symbol bytes; it does not invoke binutils or scan
source text for addresses or required function names.

Build the Rust comparison probes first:

```console
cargo build --manifest-path hil/esp32s31/Cargo.toml \
  -p open-esp-radio-hil-esp32s31-trace-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

`libphy.a` is not directly executable because its calls and data references
are relocatable. Build the isolated whole-archive oracle ELF as well:

```console
cargo build --manifest-path hil/vendor-oracle/esp32s31/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release
```

The linked ELF retains relocations, all 161 archive functions, unresolved
external call identities, and an embedded SHA-256 provenance note for the
pinned source archive. The verifier rejects a linked ELF without that exact
note. The original `libphy.a` remains the authoritative function inventory;
the linked ELF supplies executable code, while the ROM ELF supplies callable
ROM code as its companion. ROM data symbols used by relocations are assigned
their real ECO0 addresses by the linker script. An unresolved alloc-section
data/GOT/HI20/LO12 relocation is rejected instead of being executed as zero.

Execute a linked RV32 ELF with concrete arguments and a deterministic MMIO
scenario:

```console
cargo phy-trace execute --svd svd/esp32s31-radio.svd \
  --artifact _oracles/esp32s31_rev0_rom.elf \
  --symbol phy_freq_band_reg_set --arg 1 \
  --mmio 0x20107030=0xffffffff --mmio 0x20107ce4=0
```

The executor follows branches and direct/tail calls, intercepts SVD MMIO,
records ordered bus reads/writes and returns covered branch/call sites.
Repeated `--read ADDRESS=VALUE` options provide response sequences for polling
scenarios. `--ram ADDRESS=VALUE` seeds and observes one little-endian word;
`--observe ADDRESS=LENGTH` adds a byte range whose final mutations are compared
without treating compiler-private stack traffic as behavior. This is the
mechanism for `phy_param` and other caller-owned state. Delay stubs and RISC-V
`FENCE` instructions are emitted as ordered trace events.

Memory is fail-closed. ELF file bytes, zero-filled ELF BSS, the execution
stack, and explicitly seeded RAM/MMIO are known regions; an unseeded RAM or
MMIO read makes the scenario `INCOMPLETE`. `--mmio` declares a stable read
value and `--read` declares an ordered response stream; a bus write never
changes either one. Storage, W1C, FIFO and self-clearing behavior require an
explicit peripheral model. Scripted MMIO responses must be consumed exactly.
This prevents an unresolved table pointer, data relocation, polling
expectation or invented write-readback from silently becoming zero. At most
eight integer arguments are accepted until stack-argument ABI support is
implemented.

Compare linked vendor and Rust implementations under the same scenarios:

```console
cargo phy-trace execute-compare --svd svd/esp32s31-radio.svd \
  --vendor-artifact _oracles/esp32s31_rev0_rom.elf \
  --vendor-symbol phy_freq_band_reg_set \
  --rust-artifact \
    hil/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-hil-esp32s31-trace-probes-elf \
  --rust-symbol open_phy_trace_freq_band_reg_set \
  --case disabled --arg 0 \
    --mmio 0x20107030=0xffffffff --mmio 0x20107ce4=0xffffffff \
  --case enabled --arg 1 \
    --mmio 0x20107030=0xffffffff --mmio 0x20107ce4=0
```

`execute-compare` compares ordered MMIO reads/writes, write values (including
same-value writes), fences, delays, observed final RAM mutations and optional
return values. It statically inventories reachable
conditional branches in each ELF, aggregates the outcomes exercised by every
`--case`, and prints each
missing true/false outcome as `UNCOVERED-BRANCH`. An unresolved indirect edge
is printed as `UNCOVERED-CONTROL-FLOW`, and a physical MMIO access without an
SVD register is printed as `UNCOVERED-MMIO`. Any of these conditions makes the
result `INCOMPLETE`, even when all observed events match.

The static coverage pass propagates instruction-level constants through direct
and tail calls. Consequently, a child branch made unreachable by a fixed call
argument is not falsely required, while an input-dependent branch retains both
required outcomes. If constant propagation loses an indirect target, the edge
remains `UNCOVERED-CONTROL-FLOW`; a profile may resolve it through a symbolic
RAM word, after which the exact child arguments determine its feasible branch
inventory. Regression tests cover both the fixed-argument and unknown-argument
cases.

Every symbolic MMIO read has a separate ordered token, including repeated
reads of the same address. A later write derived from the first observation
therefore cannot compare equal to one derived from the second observation
unless the compiled data flow is actually the same.

Checked-in profiles make those cases part of the validator rather than prose.
The ESP32-S31 profile file contains both ROM and archive entries, so it is
executed by the source-aware `verify-all` command below. `verify-profiles`
remains available for a focused profile file that targets one vendor artifact.

The profile format has `profile`, required `vendor-source` (`rom` or
`archive`), `vendor-symbol`, `rust-symbol`, optional `contract` (`scenario` or
`state`), optional `compare-return`, and one or more `case` sections. Case
directives are `arg`,
`mmio`, `read`, `ram`, `vendor-ram-symbol`, `rust-ram-symbol`, `observe`, and
`max-steps`. Source-specific `vendor-observe`/`rust-observe` ranges and
`vendor-observe-symbol`/`rust-observe-symbol` ranges normalize corresponding
state to the same comparison offsets; the symbolic form is
`SYMBOL[+OFFSET]=LENGTH`. `vendor-ram` and `rust-ram` seed one source-specific
little-endian word without implicitly observing its physical address. Numeric
values accept the same decimal or hexadecimal
notation as the CLI. A symbolic RAM word resolves to the named symbol
independently in the selected ELF, which models function tables without
pinning unstable linked addresses. Dynamically resolved indirect calls are
reported as `COVERED-CONTROL-FLOW`; their child branch inventory is included
in coverage.
Profiles are executable coverage input; they are not a parallel function
ledger.

`contract state` means the vendor bytes are decoded at the binary boundary
while the Rust probe publishes a stable canonical projection through typed
getters. The observed Rust address is the trace protocol output, not the
private layout of `PhyColdState`. The current schema covers dot11p, current
power level, BT power tracking, BLE channel base, initialization mode,
temperature tracking and slow TX-power tracking.

Summarize all vendor functions and every construct not covered by the direct
trace engine:

```console
cargo phy-trace analyze --svd svd/esp32s31-radio.svd \
  --artifact _oracles/esp32s31_rev0_rom.elf

cargo phy-trace analyze --svd svd/esp32s31-radio.svd \
  --artifact _oracles/libphy.a --symbol-prefix ''
```

Verify every ROM function against a conventionally named Rust probe and report
missing probes as uncovered work:

```console
cargo phy-trace verify --svd svd/esp32s31-radio.svd \
  --vendor-artifact _oracles/esp32s31_rev0_rom.elf \
  --rust-artifact \
    hil/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-hil-esp32s31-trace-probes-elf
```

Generate the authoritative combined report for both vendor sources:

```console
cargo phy-trace verify-all --svd svd/esp32s31-radio.svd \
  --rom-artifact _oracles/esp32s31_rev0_rom.elf \
  --archive-artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --archive-inventory _oracles/libphy.a \
  --rom-companion \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --archive-companion _oracles/esp32s31_rev0_rom.elf \
  --rust-artifact \
    hil/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-hil-esp32s31-trace-probes-elf \
  --profiles tools/phy-trace/profiles/esp32s31.profile \
  --gate regression --match-floor 96
```

`verify-all` treats `(vendor source, symbol)` as function identity. This is
necessary because `phy_fe_reg_update` exists in both ROM and `libphy.a`; the
two implementations remain separate rows and the inventory total is therefore
305 + 161 = 466, not the 465 unique spellings. It emits per-source summaries
and one `TOTAL-SUMMARY`. Rust probes are checked against the combined
inventory, so a probe belonging to one source is not falsely reported as an
orphan while the other source is being processed.

The two verification gates answer different questions:

- `--gate regression --match-floor 96` passes when there are no mismatches,
  incomplete comparisons, or orphan probes and at least 96 functions retain
  evidence. Missing probes outside that established set remain in the report.
- `--gate completion` (the default) additionally requires every vendor
  function in the selected inventory to have a matching Rust probe.

The explicit floor is mandatory for the regression gate so the total amount
of established evidence cannot silently decrease. The current ESP32-S31
inventory has 466 source-qualified functions; 96 have evidence, while 370
remain missing.

For ROM, `verify` and `verify-all` map `phy_NAME` to
`open_phy_trace_NAME`. Archive symbols use their full name, so archive
`phy_NAME` maps to `open_phy_trace_phy_NAME`; this keeps identically named ROM
and archive functions distinct. A function with an
observable return uses `open_phy_trace_ret_NAME`; the verifier then compares
the symbolic RISC-V `a0` result in addition to MMIO. Its per-function outcomes
are:

- `MATCH`: the selected comparison method completed and agreed;
- `MISMATCH`: both traces are complete but differ;
- `INCOMPLETE`: a present pair cannot yet be proved;
- `UNCOVERED`: no Rust comparison probe exists.

Each `MATCH` row reports `evidence=symbolic`, `evidence=scenario`, or
`evidence=state`. Symbolic equality proves the normalized straight-line trace.
Scenario equality proves only the explicitly declared inputs plus complete
branch-outcome coverage. State evidence additionally compares the declared
canonical pre/post projection without depending on Rust object layout. Neither
concrete contract claims exhaustive equality over an undeclared input domain.

When a paired function cannot be closed by the straight-line symbolic engine,
`verify` uses its named concrete profile. It promotes the result to `MATCH`
only when every case matches and both ELF branch inventories have no uncovered
outcome or unresolved edge. The final `SUMMARY` therefore combines both proof
methods while keeping their evidence counters separate; it does not leave a
profile-confirmed function as `INCOMPLETE`.

The engine fails closed on control flow, calls and tail jumps, unresolved MMIO
write values, and MMIO registers absent from the SVD. Every such site is
printed as an `UNCOVERED` row followed by aggregate `SUMMARY` counters. The
current direct engine does not claim path coverage for loops, input-dependent
branches, indirect calls or table-derived addresses.

Function-by-function behavior descriptions do not belong in `docs/phy` once
the compiled comparison proves them. The executable scenarios, coverage
summary and pinned oracle identity are the audit record; documentation is
reserved for tool operation and exceptional rules that cannot be encoded in
the verifier.

`verify` also rejects a vendor artifact whose SHA-256 is not one of the pinned
ESP32-S31 rev0 ROM or `libphy.a` oracle images. Adding another chip or ROM
revision requires an explicit reviewed digest in the verifier.

`extract` and `compare` remain available for focused investigation of one
symbol. Run the command without arguments for their complete syntax.

No parity exceptions are currently accepted. A future exception belongs in
the verifier as a typed rule with exact artifact, symbol and behavior scope,
plus tests; it must never turn an unrelated incomplete trace into `MATCH`.
