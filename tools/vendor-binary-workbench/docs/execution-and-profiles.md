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
cargo vendor-binary-workbench advanced execute run \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
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

Profiles may also declare mechanism-neutral stateful FIFO services separately
for vendor and Rust executions. A reviewed binding maps a concrete function
symbol and RV32 ABI to `enqueue`, `dequeue` or `len`; the generic executor does
not know RTOS names. Queue contents persist through `ExecutionSession`, and
the report retains initial/final items plus ordered enqueue, dequeue, full,
empty and length lifecycle evidence. Invalid widths/capacities, duplicate
IDs/handles, missing services, wrong handles and a symbol shared with a
scripted call response fail before execution. Semantic annotation alone still
does not authorize this executable model.

## Replay an asynchronous lifecycle

`execute run` is intentionally one call. Long-lived task routes use a replay
manifest so writable ELF state and scenario-owned services survive across
ordered calls while each private stack remains fresh:

```console
cargo vendor-binary-workbench \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  --details advanced execute replay \
  --source libpp-replay \
  --manifest verification/vendor/targets/esp32s31/replays/pp-signal-25.toml \
  --output verification/vendor/targets/esp32s31/generated/findings/replays/pp-signal-25.json
```

The checked-in example executes the real `pp_post(0x19)`, records the FIFO
enqueue and transition-only wake, then executes `ppTask` until its reviewed
goal reaches `wdevProcessRxSucDataAll`. The same session proves the matching
dequeue and `pp_sig_cnt[0x19]` increment/decrement lifecycle. A goal bounds a
long-lived task; it is not a function return and therefore cannot be used with
return-value comparison.

Replay manifests use schema 2. Each phase may declare `observe-memory` ranges
by linked symbol and fail-closed `expectations` for memory transitions, FIFO
events, call counts/argument zero, and the absence of delay effects. Published
evidence retains the resolved address and exact write PC; a reviewed event
route may then bind the same observation to the generic `counted-latch` model.

`--output` persists strict evidence rather than a presentation dump. It binds
the replay to the exact manifest and linked ELF digests. The reviewed event
route names its producer and consumer phases, so a later `inspect flow
--event-route rx-success-to-pp-task` can promote queue delivery from “modeled”
to “executed”. Any changed or missing input makes that claim incomplete.

The ordinary `libpp` linked ELF remains the lossless inventory/navigation
view. The separate `libpp-replay` link unit defines storage for runtime-owned
external globals such as `g_osi_funcs_p` and `xphyQueue`; scenarios still own
their values. This distinction is required because allowing an unresolved
data symbol through the linker can relax its access into `lw ..., 0(zero)`,
which is not repairable by execution-time seeding.

Runtime table slots may target a linked function with `kind = "symbol"` or an
external scenario function with `kind = "modeled-symbol"`. A modeled symbol
is assigned a synthetic function-pointer value and must have an executable
FIFO binding or explicit call-response model in that phase. Merely naming an
RTOS semantic is insufficient. `pointer-cell-symbols = ["g_osi_funcs_p"]`
resolves a table pointer cell from the exact linked ELF and avoids unstable
numeric addresses.

## Compare vendor and Rust executions

Compare linked vendor and Rust implementations under the same scenarios:

```console
cargo vendor-binary-workbench advanced execute compare \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
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
`--case`, and reports each missing true/false outcome as an uncovered branch.
An unresolved indirect edge is reported as uncovered control flow, and a
physical MMIO access is validation-grade whenever its complete byte range is
inside a declared MMIO region and the region permits the requested read or
write. SVD and reviewed register names are optional enrichment: an unnamed
access is reported as `UNNAMED-MMIO`, but does not make the comparison
incomplete.
Each `--case` is a complete,
self-contained scenario in the form `NAME[;KEY=VALUE...]`; this keeps repeated
scenario groups typed and unambiguous at the CLI boundary. An access outside
the declared map, one crossing a region boundary, a permission violation, an
unseeded read, or incomplete control-flow coverage makes the result
`INCOMPLETE`, even when the remaining observed events match.

Concrete comparison has exactly three verdicts: `MATCH`, `DIFF`, and
`INCOMPLETE`. `DIFF` means both executions completed but their requested
observables differ. `INCOMPLETE` means the workbench cannot make that claim
because execution, branch/control-flow coverage, MMIO classification or a
required model is missing.

Schema 9 reports a typed `TraceDiffReport` instead of dumping both complete
outcomes for every difference. It identifies the first differing event, RAM
change or return value, keeps up to three aligned items before and after it,
and records the ordered branch/call paths for both sides. Coverage gaps use the
same typed report with `kind = "coverage"` and remain an `INCOMPLETE` result.
Every case also retains its source-specific runtime table instances and slot
targets as typed scenario-environment provenance. This is the shared
presentation model for JSON, human CLI and the TUI/application layer.
Every observable event in a diff also retains its producer PC and, when the
linked image exposes a containing symbol, the symbol plus relative offset.
This provenance is collected by the concrete executor and is not inferred by
a renderer. Source file/line enrichment remains a later optional DWARF layer.

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

Profile files are strict TOML with `schema = 2`, one or more `[[profiles]]`
tables, and one or more nested `[[profiles.cases]]` tables. A profile requires
`name`, `vendor-source`, `vendor-symbol`, and `rust-symbol`; `contract`
(`"scenario"` or `"state"`) and `compare-return` are optional. Closed ABI
Continuous domains use `[[profiles.argument-ranges]]` with `index`, `min`,
and `max`. Sparse selector domains use `[[profiles.argument-values]]` with
`index` and a non-empty, duplicate-free `values` array. A profile may not
constrain the same argument through both forms.

Cases store arguments in `arguments`, stable MMIO seeds in
`[[profiles.cases.mmio-initial]]`, ordered reads in
`[[profiles.cases.mmio-reads]]`, and RAM words in the corresponding `ram`,
`vendor-ram`, or `rust-ram` arrays of tables. Symbol-backed words,
source-specific observations, runtime tables, memory objects, device models,
and `max-steps` are structured TOML fields rather than positional strings.
Numeric values use normal TOML decimal or hexadecimal integers. A symbolic
RAM word resolves to the named symbol independently in the selected ELF,
which models function tables without pinning unstable linked addresses.
Dynamically resolved indirect calls are reported as
`COVERED-CONTROL-FLOW`; their child branch inventory is included in coverage.
Profiles are executable coverage input; they are not a parallel function
ledger.

Schema 2 models reviewed external-call responses independently for the vendor
and Rust images. Each response is consumed in call order for its exact symbol,
may provide at most the RV32 `a0`/`a1` return words, and may write reviewed
outputs through private-stack pointer arguments. A reviewed zeroing allocator
instead consumes one explicit fresh arena and derives the live prefix from its
size argument:

```toml
[[profiles.cases.vendor-calls]]
symbol = "queue_send_from_isr"
return-words = [1]
outputs = [{ kind = "private-stack", pointer-argument = 2, width = 8, value = 1 }]

[[profiles.cases.rust-calls]]
symbol = "wake_receiver"
return-words = [0]

[[profiles.cases.vendor-calls]]
symbol = "wifi_zalloc"
allocation = { address = 0x3fce0000, size-argument = 0, capacity = 0x100 }
```

The two sides need not use the same external symbol or ABI adapter. Unknown
fields, more than two return words, duplicate output arguments, non-`a0..a7`
arguments, an invalid private-stack pointer, or an unused response make the
profile invalid or the execution incomplete. There is no schema-1 compatibility
path. An allocation cannot also declare `return-words`; its arena must be
non-empty, 32-bit aligned, bounded to 1 MiB, large enough for the runtime
request, and disjoint from ELF memory, MMIO, the private stack and initial RAM.
Only the requested prefix is addressable and zero-initialized. Allocation
lifecycle evidence is emitted independently for vendor and Rust runs; arena
addresses describe their modeled environments and are not themselves compared
as observable driver effects.

### Runtime table instances

A reviewed interface pack describes a stable table layout and ABI. A scenario
describes one concrete runtime instance: where the table and optional pointer
cell live, and which linked function is installed in each slot. These are
separate claims. Use source-specific directives so vendor and Rust images may
have different addresses and symbols:

```toml
[[profiles.cases.vendor-tables]]
layout-id = "esp32s31-radio-rev0::wifi-osi-v9"
base-address = 0x3fff1000
layout-size = 0x40
pointer-cells = [0x3fff0030]
slots = [{ offset = 0x10, target = { kind = "symbol", value = "vendor_queue_send" } }]

[[profiles.cases.rust-tables]]
layout-id = "esp32s31-radio-rev0::wifi-osi-v9"
base-address = 0x3ffe1000
layout-size = 0x40
pointer-cells = []
slots = [{ offset = 0x10, target = { kind = "symbol", value = "open_queue_send" } }]
```

Slot targets use `{ kind = "symbol", value = "..." }`,
`{ kind = "modeled-symbol", value = "..." }`,
`{ kind = "address", value = 0x... }`, or `{ kind = "null" }`. The executor
resolves linked symbols against the exact side's ELF, allocates collision-
checked synthetic addresses for modeled external functions, and then
materializes 32-bit little-endian pointer cells and slots. It rejects duplicate
instances/slots, unaligned locations, out-of-layout offsets, missing target
symbols, modeled targets without executable behavior and conflicts with
explicit RAM seeds. Thus a profile no longer needs
to encode unstable linked callback addresses as raw words.

At the backend boundary `LAYOUT-ID` is retained provenance, not proof that the
runtime bytes satisfy a reviewed interface pack. Project-level layout/guard
validation remains the owner of that assertion; concrete execution proves only
the behavior of the explicitly materialized instance.

Execution also retains a fail-closed lifecycle for each instance: initial slot
contents, pointer-cell installation, CPU writes into the table, and indirect
calls resolved back to a unique `(layout, slot)` pair. If an indirect target
matches no slot or several slots in configured instances, lifecycle coverage is
incomplete and `execute compare` returns `INCOMPLETE` even if the remaining bus
events match. This separates the stable reviewed `TableLayout` claim from the
scenario-specific `TableInstance` contents and makes callback replacement or
uninstallation visible in reports.

### Runtime memory-object instances

Linked IR identifies caller-visible objects independently of their runtime
placement: argument pointees, globals, pointers loaded from globals, and
absolute objects. A comparison case can bind several such observations to one
logical runtime instance:

```toml
[[profiles.cases.vendor-memory-instances]]
id = "phy-state"
base-address = 0x3fff2000
length = 0x240
bindings = [
  { kind = "argument", index = 0 },
  { kind = "dereferenced-global", symbol = "g_phy_state", pointer_offset = 0 },
]

[[profiles.cases.rust-memory-instances]]
id = "phy-state"
base-address = 0x3ffe2000
length = 0x240
bindings = [
  { kind = "argument", index = 0 },
  { kind = "global", symbol = "OPEN_PHY_STATE" },
]
```

Binding kinds are `argument`, `global`, `dereferenced-global`, and `absolute`.
Materialization checks placement, symbol
addresses and pointer cells instead of inferring nominal type identity from
matching offsets. The shared instance ID is the explicit reviewer claim that
the source-specific objects represent the same logical state.

### Peripheral execution models

`mmio-initial` and `mmio-reads` remain the simplest deterministic oracle: they
provide stable or ordered read responses and never infer device state from
writes. A case that needs real register behavior can instead own an explicit
model:

```toml
[[profiles.cases.device-models]]
kind = "w1c"
id = "irq-status"
address = 0x60008020
width = 32
initial_value = 0x0000000f
clear_mask = 0x00000003
read_clear_mask = 0x0000000c

[[profiles.cases.device-models]]
kind = "sequence-read"
id = "ready"
address = 0x60008024
width = 32
values = [0, 0, 1]

[[profiles.cases.device-models]]
kind = "fifo"
id = "rx-fifo"
address = 0x60008028
width = 32
read_values = [0x41, 0x42]
expected_writes = []

[[profiles.cases.device-models]]
kind = "indexed-bank"
id = "rf-bank"
index_address = 0x6000802c
data_address = 0x60008030
width = 32
initial_values = [0x10, 0x20, 0x30]
```

The closed `DeviceModelSpec` vocabulary is `constant-read`,
`sequence-read`, `w1c`, `read-to-clear`, `self-clearing`, `fifo`, and
`indexed-bank`. TOML arrays are ordered expectations. Raw reads and writes
are always retained in the ordered effect trace; model state only determines
the returned value and subsequent peripheral state.

A device model exclusively owns its MMIO range for that case. Overlapping
models and mixing a model with `mmio`/`read` seeds in the same range fail
closed. The generic backend exposes `DeviceModel` and `DeviceModelInstance`
traits so a platform crate can add FIFO or state-machine behavior without
adding chip or RTOS vocabulary to the executor. Every comparison side receives
a fresh instance from the same scenario factory. Every instance separately
reports coverage at the end of execution. Unconsumed sequence values, FIFO
reads, or expected FIFO writes make that side `INCOMPLETE`; they can never be
silently ignored to obtain `MATCH`.

Compiled platform implementations are published through
`DeviceModelRegistry`. Registry IDs are exact foreign keys: duplicate and
unknown IDs fail, and neither an address nor a familiar semantic name selects
a model. This keeps the standard data vocabulary portable while giving a
selected platform harness a controlled extension point for genuine device
state machines.

The default `verify profiles` view contains a profile coverage table and a
scenario table with match, diff or incomplete details. JSON and JSONL
serialize the same typed aggregate report directly for automation.

`[[profiles.argument-ranges]]` and `[[profiles.argument-values]]` are closed
ABI preconditions, not hints inferred from the listed cases. The loader
requires an executed case for every value combination in the declared finite
domain (currently at most 4096 combinations). Static reachability is then
computed separately for every admissible combination, so an out-of-domain
Rust safety panic does not create a false coverage hole, while any in-domain
branch, child call, or unresolved edge remains required. Sparse values are
important for jump-table projections: `values = [6, 8]` does not admit case
7. Arguments without a declared range or value set remain unknown.

## State and semantic contracts

`contract = "state"` means the vendor bytes are decoded at the binary boundary
while the Rust probe publishes a stable canonical projection through typed
getters. The observed Rust address is the trace protocol output, not the
private layout of `PhyState`. The current schema covers dot11p, current
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
PBus and DCO state consumed during cleanup. The Bluetooth TX-gain parent
contract is hierarchical: it first locks the six direct child selections and
their arguments, then compares the actually active RFPLL, TX-cap, TXDC,
TX-power, PWDET and publication phases with
`PhyBluetoothTxGainInitTransition`. Cold and retained calls share one vendor
execution session and one Rust `PhyState`; retained no-op children are not
misreported as effects merely because the vendor entered their wrapper. The
final comparison includes the three DCO rows and the gain-producing power
curve, adjustment, attenuation and calibration state.
Vendor baselines come from the linked ELF rather than the Rust object
representation. This compares external actions and typed final state without
requiring Rust to reproduce the vendor stack, function boundaries or polling
loop:

```console
cargo vendor-binary-workbench advanced verify contract channel \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench advanced verify contract rf-init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench advanced verify contract bluetooth-tx-gain-init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench advanced verify contract baseband-init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --vendor-artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench advanced verify contract register-init \
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
read, invalid MMIO access or event/state divergence fails closed and retains the
first complete normalized diff in the typed qualification report. Each case
reports the number of state bytes read and
written under the reviewed footprint. RF init runs twice through one persistent execution
session: first from the linked ELF image, then from the RAM state produced by
the first call. Its `STATE-SEQUENCE-MATCH` therefore also checks retained
`.data`/`.bss`; MMIO responses and the private stack are fresh for each call.
