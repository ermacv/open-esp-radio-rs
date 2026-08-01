# Compiled PHY parity verifier

`open-esp-radio-phy-trace` derives MMIO behavior from RISC-V ELF files and
archive members and resolves every physical register through composed SVD
catalogs. It
parses ELF/ar containers with the Rust `object` reader and decodes RV32IMAC
instructions directly from symbol bytes; it does not invoke binutils or scan
source text for addresses or required function names.

The authoritative ESP32-S31 run passes both `svd/esp32s31-radio.svd` and
`svd/esp32s31-platform-radio-deps.svd`. The latter mirrors only the official
PAC registers reached by the trace and is never used to generate a runtime
peripheral owner.

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
pinned source archive. The note is informational; the verifier pins the
SHA-256 of the complete linked ELF and rejects a modified ELF even when it
copies the expected note. It independently pins every vendor companion and
archive inventory used by an integrated run. The original `libphy.a` remains
the authoritative function inventory;
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

Pass `--timeline` to `execute` to print one unified ordered stream containing
calls (with all eight register arguments), conditional-branch outcomes,
RAM reads/writes, MMIO/delay events and fences. The ordinary call/branch sets
remain compact coverage summaries; the timeline retains intermediate RAM
values, multiplicity, loop iterations and relative order for semantic
normalizers.

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

Persistent execution has explicit reset and RAM-ownership semantics. A normal
call retains writable CPU-owned ELF state, `ColdBoot` discards the overlay and
reloads `.data`/`.bss` from the ELF image, and `WarmReset` retains only ranges
explicitly declared persistent/no-init. Contract footprints classify ranges as
CPU-owned, MMIO-derived, interrupt-owned, DMA-owned, shared/unknown or
immutable. Interrupt/DMA/shared ranges are invalidated at every call boundary:
the next scenario must seed them again before a read. This prevents an old
`phy_param` byte from being treated as stable merely because the previous call
observed it.

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

Architectural replacements use named semantic contracts from the disposition
manifest. The ESP32-S31 channel contract executes the complete pinned vendor
root and normalizes its ordered calls/MMIO, call-time TX-gain payload and final
`phy_param` state into the same action vocabulary produced by
`PhyChipChannelTransition`. The RF-init contract similarly compares the direct
child phases of `phy_rf_init` with `PhyRfColdInit`; delay, enable, clock-select,
PHY-I2C address/mask and retained RC-prestate parameters are part of those
events instead of being discarded. It then compares a canonical typed
projection of RC calibration, BBPLL, parameter 0x18e, crystal-duty and
channel-frequency state. Both root contracts declare a reviewed directional
`phy_param` footprint. Any read or write outside those named ranges, including
a write to a read-only input range, fails qualification. The Bluetooth TXDC
contract executes the archive orchestrator and its ROM calibration child,
comparing all ordered PBus, TX-clock, tone and delay events plus the three BT
DCO rows and completion flag. The Bluetooth TX-power contract covers
`phy_bt_tx_pwctrl_init` and its shared mode-one calibration child. It compares
saved/restored PHY-I2C fields, complete shared debug/work-mode transitions, BT
PBus/DCO setup, RFPLL channel and TX-cap selection, every tone/SAR delay and
the typed BT power curve. Its directional footprint also checks the shared
PBus and DCO state consumed during cleanup.
Vendor baselines come from the linked ELF rather than the Rust object
representation. This compares external actions and typed final state without
requiring Rust to reproduce the vendor stack, function boundaries or polling
loop:

```console
cargo phy-trace qualify-esp32s31-channel \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --vendor-artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion _oracles/esp32s31_rev0_rom.elf

cargo phy-trace qualify-esp32s31-rf-init \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --vendor-artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion _oracles/esp32s31_rev0_rom.elf
```

The current matrix covers channel numbers 1–13 for both zero/nonzero CBW
branches, every equivalent 2.4-GHz frequency input, and representative
off-grid frequencies whose second NRX update uses the normalized channel
frequency. The 45 reviewed edge cases are followed by 32 deterministic
generated frequency/CBW cases with replayable seeds. All 77 calls run through
one persistent vendor/Rust state sequence rather than resetting `phy_param`
for every case. A success is labeled
`STATE-SCENARIO-MATCH`, not symbolic or domain-exhaustive equality. Any poison
read, unmapped MMIO or event/state divergence fails closed and prints the first
complete normalized diff. Each case reports the number of state bytes read and
written under the reviewed footprint. RF init runs twice through one persistent execution
session: first from the linked ELF image, then from the RAM state produced by
the first call. Its `STATE-SEQUENCE-MATCH` therefore also checks retained
`.data`/`.bss`; MMIO responses and the private stack are fresh for each call.

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
cargo phy-trace verify-all \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
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
  --dispositions tools/phy-trace/dispositions/esp32s31.disposition \
  --gate regression --match-floor 103 \
  --evidence-baseline tools/phy-trace/baselines/esp32s31.evidence \
  --json-report oracle-regression.json
```

`verify-all` treats `(vendor source, symbol)` as function identity. This is
necessary because `phy_fe_reg_update` exists in both ROM and `libphy.a`; the
two implementations remain separate rows and the inventory total is therefore
305 + 161 = 466, not the 465 unique spellings. It emits per-source summaries
and one `TOTAL-SUMMARY`. Rust probes are checked against the combined
inventory, so a probe belonging to one source is not falsely reported as an
orphan while the other source is being processed.

The disposition manifest classifies every exact implemented replacement and
uses fail-closed defaults for the rest. A `semantic-contract` names executable
validator logic; a Rust component without one remains
`IMPLEMENTED-UNQUALIFIED` and does not count as evidence. Executable root
contracts are reported separately as `composition-match`: they compare their
declared action/state projection but do not imply an independent proof for
every transitive leaf. Such an entry can use
one or more `blocked-by SOURCE SYMBOL` directives. The verifier rejects a
missing blocker target and prints the source-qualified blockers in the report,
so an architectural root cannot hide an unported child behind prose. Protocol
classification is independent, so shared PHY/RF, Wi-Fi, Bluetooth, BLE, Coex
and 802.15.4 scope are not inferred from completion status.

The two verification gates answer different questions:

- `--gate regression --match-floor 103 --evidence-baseline PATH` passes when
  there are no mismatches, incomplete comparisons, or orphan probes, at least
  103 functions retain evidence, and every source-qualified baseline function
  retains the same evidence kind. A lost state proof cannot be hidden by a new
  scenario match elsewhere. New evidence is reported as `EVIDENCE-ADDITION`
  and does not require weakening the existing baseline. Profile evidence also
  contains a hash of the parsed scenario contract, so narrowing inputs,
  observations or scripted responses requires a reviewed baseline change.
  Composition evidence contains a SHA-256 over the contract label, scenario
  wiring, semantic normalizer/footprints and execution engine sources. Editing
  the validator itself therefore also requires an explicit baseline review.
- `--gate completion` (the default) additionally requires every vendor
  function in the selected inventory to have a matching Rust probe.

The explicit floor is mandatory for the regression gate so the total amount
of established evidence cannot silently decrease. The current ESP32-S31
inventory has 466 source-qualified functions; 103 have evidence. Of the
remaining 363, two are implemented architectural roots that still need
semantic contracts and 361 are classified `not-yet-ported`.

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

Each `MATCH` row reports `evidence=symbolic`, `evidence=scenario`,
`evidence=state`, or `evidence=composition-state-scenario`. Symbolic equality proves the
normalized straight-line trace. Scenario equality proves only the explicitly
declared inputs plus complete branch-outcome coverage. State evidence
additionally compares the declared canonical pre/post projection without
depending on Rust object layout. Composition-state-scenario evidence compares
normalized root actions and final state for a declared transition matrix
without claiming independent proof of all transitive children. None of the
concrete contracts claims exhaustive equality over an undeclared input domain.
`--json-report PATH` writes a versioned machine-readable `verify-all` result
with the summary, evidence identities and SHA-256 of every input artifact and
policy file.

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

`verify` also rejects any vendor artifact, linked executable, inventory or
companion whose complete SHA-256 is not one of the pinned ESP32-S31 rev0
oracle images. Adding another chip, linked layout or ROM revision requires an
explicit reviewed digest in the verifier and an evidence-baseline update.

`extract` and `compare` remain available for focused investigation of one
symbol. Run the command without arguments for their complete syntax.

`cargo test --workspace --locked` does not require the ignored private oracle
directory. Decoder, memory and policy behavior use synthetic fixtures; the two
inventory-count checks report a skip when the pinned ROM/archive fixtures are
not installed. The explicit qualification commands above remain the required
private-oracle integration checks. The repository CI runs formatting,
workspace tests, strict validator-only clippy, PAC generation checks and the
source-only audit. A separate
`Private oracle regression` workflow runs only on protected `main` or manual
dispatch using a dedicated self-hosted runner and approved
`oracle-regression` environment; it uploads both text and JSON reports and
never executes pull-request code with proprietary oracle access.

No parity exceptions are currently accepted. A future exception belongs in
the verifier as a typed rule with exact artifact, symbol and behavior scope,
plus tests; it must never turn an unrelated incomplete trace into `MATCH`.
