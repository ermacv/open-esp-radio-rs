# Execution and profiles

## Build executable probes and oracle images

Build the Rust comparison probes first:

```console
CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

`libphy.a` is not directly executable because its calls and data references
are relocatable. Build the isolated whole-archive oracle ELF as well:

```console
cargo build --manifest-path verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release
```

The linked ELF retains relocations, all 161 archive functions, unresolved
external call identities, and an embedded provenance note for its source
archive. The note and content identities printed by the workbench are
informational. The caller must authenticate the ROM, archive, linked image and
companions before invocation; the workbench has no built-in artifact
allow-list. The original `libphy.a` remains the authoritative function
inventory;
the linked ELF supplies executable code, while the ROM ELF supplies callable
ROM code as its companion. ROM data symbols used by relocations are assigned
their real ECO0 addresses by the linker script. An unresolved alloc-section
data/GOT/HI20/LO12 relocation is rejected instead of being executed as zero.

## Execute one function

Execute a linked RV32 ELF with concrete arguments and a deterministic MMIO
scenario:

```console
cargo vendor-binary-workbench execute run \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --artifact "$ESP32S31_ROM_ELF" \
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
`FENCE` instructions are emitted as ordered trace events. Ordinary RV32A
`AMO.W` operations are executed on aligned RAM so optimized Rust ownership
code using atomics can participate in composition probes; atomics against
MMIO remain rejected without an explicit peripheral model.

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
implemented. Optimized Rust may copy otherwise-uninitialized struct/enum
padding. `--stack-fill BYTE` explicitly supplies those private bytes for a
compiled probe while the default remains poison. A verification using this
escape hatch must repeat the scenario with distinct fills and require the same
observable MMIO, delays and result.

Persistent execution has explicit reset and RAM-ownership semantics. A normal
call retains writable CPU-owned ELF state, `ColdBoot` discards the overlay and
reloads `.data`/`.bss` from the ELF image, and `WarmReset` retains only ranges
explicitly declared persistent/no-init. Contract footprints classify ranges as
CPU-owned, MMIO-derived, interrupt-owned, DMA-owned, shared/unknown or
immutable. Interrupt/DMA/shared ranges are invalidated at every call boundary:
the next scenario must seed them again before a read. This prevents an old
`phy_param` byte from being treated as stable merely because the previous call
observed it.

## Compare vendor and Rust executions

Compare linked vendor and Rust implementations under the same scenarios:

```console
cargo vendor-binary-workbench execute compare \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
  --vendor-symbol phy_freq_band_reg_set \
  --rust-artifact \
    target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-verification-esp32s31-probes-elf \
  --rust-symbol open_phy_trace_freq_band_reg_set \
  --case 'disabled;arg=0;mmio=0x20107030=0xffffffff;mmio=0x20107ce4=0xffffffff' \
  --case 'enabled;arg=1;mmio=0x20107030=0xffffffff;mmio=0x20107ce4=0'
```

`execute compare` compares ordered MMIO reads/writes, write values (including
same-value writes), fences, delays, observed final RAM mutations and optional
return values. It statically inventories reachable
conditional branches in each ELF, aggregates the outcomes exercised by every
`--case`, and prints each
missing true/false outcome as `UNCOVERED-BRANCH`. An unresolved indirect edge
is printed as `UNCOVERED-CONTROL-FLOW`, and a physical MMIO access without an
SVD register is printed as `UNCOVERED-MMIO`. Each `--case` is a complete,
self-contained scenario in the form `NAME[;KEY=VALUE...]`; this keeps repeated
scenario groups typed and unambiguous at the CLI boundary. Any of these
conditions makes the
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

## Checked-in profiles

Checked-in profiles make those cases part of the verifier rather than prose.
The ESP32-S31 profile file contains both ROM and archive entries, so it is
executed by the source-aware `verify inventory` command below. `verify profiles`
is available for a focused profile file that targets one vendor artifact.

The profile format has `profile`, required `vendor-source`, `vendor-symbol`,
`rust-symbol`, optional `contract` (`scenario` or `state`), optional
`compare-return`, optional profile-level `arg-range INDEX MIN MAX`, and one or
more `case` sections. Case directives are `arg`,
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

`arg-range` is a closed ABI precondition, not a hint inferred from the listed
cases. The loader requires an executed case for every value combination in
the declared finite domain (currently at most 4096 combinations). Static
reachability is then computed separately for every admissible combination,
so an out-of-domain Rust safety panic does not create a false coverage hole,
while any in-domain branch, child call, or unresolved edge remains required.
Arguments without a declared range remain unknown.

## State and semantic contracts

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
a write to a read-only input range, fails verification. The Bluetooth TXDC
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
cargo vendor-binary-workbench verify contract channel \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench verify contract rf-init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"
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
