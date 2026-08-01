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

## Internal architecture

The binary entry point only translates the library result into an exit code.
The implementation is split by responsibility:

- `artifact` reads relocatable ELF objects and archives; `execution::image`
  separately loads linked executable images;
- `mmio` builds the register index from one or more SVD files;
- `ir` owns symbolic values, observable traces, indexed-MMIO proofs and the
  resolved reference-program types;
- `analysis` contains the structural tracer and artifact-level call resolver;
- `execution` contains scenarios, persistent memory ownership, coverage and
  the RV32 machine;
- `codegen` renders only a `ResolvedReferenceProgram`;
- `verification` owns profiles, dispositions, evidence and comparisons;
- `qualification` owns ESP32-S31-specific state/action normalization;
- `cli` parses a typed top-level command and dispatches it to those services.

Reference generation has an explicit fail-closed phase boundary:

```text
FunctionAnalysis -> resolution/composition -> ResolvedReferenceProgram -> Rust codegen
```

The resolved program type has no variants for unresolved direct/tail calls or
temporary branch decisions and carries no blockers. Consequently incomplete
analysis cannot reach code generation by accident. Composition evidence hashes
the concrete qualification, comparison and execution source modules involved
in each contract, not only their facade modules.

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

cargo phy-trace analyze --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --companion _oracles/esp32s31_rev0_rom.elf \
  --entry-contract esp32s31-phy-registered \
  --json-report /tmp/esp32s31-libphy-analysis.json
```

Each row reports `reference-codegen=eligible|blocked`, the number of composed
dependencies and `indexed-mmio=N`. The summary counts the stricter generation
subset separately from `direct_trace_exact`. The two are intentionally
different: a trace can be exact for MMIO comparison while an unmodeled RAM/ELF
load or store still makes generation unsafe.

Every exact reference-generation failure is also printed as a
`REFERENCE-BLOCKED` row. When call composition reaches an ineligible callee,
the row retains the complete nested cause chain instead of stopping at the
callee name. `--json-report PATH` writes the same information as a versioned
machine-readable blocker graph. It separates local and transitive reference
blockers per function, ranks ineligible callees by the number of affected root
functions, groups blocker kinds by both occurrence and affected-function
count, lists every exact unmapped MMIO address from linear and structured
branch paths, and pins the primary and companion artifacts by SHA-256. Event
validation failures name the exact event, operand role and unavailable value
class instead of collapsing to a generic eligibility error. The impact counts
overlap deliberately: a function can depend on more than one root blocker.

Use the linked vendor-oracle ELF plus ROM companion for semantic analysis of
`libphy`. The raw archive remains the authoritative inventory and relocation
source, but unresolved archive calls do not represent the final relaxed link
and companions are therefore rejected for a relocatable primary artifact.

Generate a safe, executable Rust reference for a supported symbol:

```console
cargo phy-trace generate-reference \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --artifact _oracles/esp32s31_rev0_rom.elf \
  --symbol phy_disable_agc \
  --output /tmp/phy_disable_agc_reference.rs

rustc --edition 2024 --crate-type lib \
  /tmp/phy_disable_agc_reference.rs
```

For an archive symbol, add `--member phy_init.o` when the symbol owner must be
selected explicitly. Without `--output`, the generated source is written to
stdout so it can be inspected or consumed by another tool.

The generated function is an executable specification over `ReferenceIo`, a
separate `ReferenceMemory` state port and a typed `ReferencePlatform` callback
boundary, not a guessed PAC/HAL
implementation. It retains ordered MMIO and classified ELF/RAM reads and
writes, distinct read identities, delays, fences, exact wrapping bit
expressions and source provenance. Mixed-source operations such as
`MMIO_read | argument` remain explicit expression trees instead of degrading
to unknown bits. RV32 `slt`, `sltu`, `slti` and `sltiu` retain their signed or
unsigned zero-or-one result. Variable `sll`, `srl` and `sra` mask the shift
count to five bits, and arithmetic right shift retains the signed RV32 result. The
generated `Rv32ReferenceArguments` separates the eight `a0`-`a7` register
arguments from eight modeled argument words passed on the entry stack. This is
an explicit machine-ABI boundary for the oracle, not a guessed C prototype;
production Rust APIs remain free to expose typed parameters instead. The
memory implementation must seed `.data`,
`.bss` and read-only ELF bytes from the same pinned image, resolve archive-local
and global symbol identities against that exact link, then retain the writes
required by the validation scenario. `ReferenceOutcome::exit_a0` records the machine
value in the ABI `a0` register without inferring whether the unavailable C
prototype declared a return value. An unresolved `a0` is represented by
`None`.

Generation explores resolved input/MMIO/RAM/callback-dependent branches in a
bounded acyclic CFG and emits structured Rust `if`/`else` expressions. Per
resolved function, the limit is 64 complete paths and 12 symbolic branch
decisions per path. Every
path must terminate, preserve a renderable condition and contain only modeled
effects. A loop whose branch operands become concrete during tracing is fully
unrolled, with a hard limit of 256 visits to any instruction; the resulting
ordered effects are accepted only if the loop actually terminates within that
bound. Symbolic loops, excessive iteration, path explosion, unsupported
instructions, unresolved write values and unmapped MMIO registers remain
fail-closed. A backward branch
is automatically reduced to `PollMmio` only when its complete loop body has
exactly one SVD-mapped MMIO read, pure scalar operations, no calls, stores,
fences, RAM accesses or stack mutation, and an exit predicate reducible to
`value & mask == expected`. The value of the final poll read is deliberately
invalidated after the loop; later observable use therefore fails closed rather
than escaping an unmodeled token. More complex reviewed polling code may use an
exact-body summary, but only when the symbol name, load address, complete
instruction bytes, SVD register family and any mutable-table entry contract all
match. A changed binary falls back to structural analysis rather than
inheriting the summary.
A reviewed bounded poll may repeat a complete composed reference flow rather
than only one direct MMIO read. Its body must have a modeled scalar result, the
attempt count must be nonzero and fixed, and exhaustion may currently retain
only a named diagnostic call. The pinned rev0 `phy_wait_rfpll_cal_end` summary
uses this form for exactly 100 iterations of `delay(20 us)` plus
`phy_i2c_readReg_Mask(0x62, 1, 7, 1, 1)`, exits on a nonzero result, and retains
the final `ets_printf` timeout event. Name, address, 86-byte size and complete
body digest are all required; this does not make arbitrary repeated call sites
eligible.
A reviewed live poll may also repeat a complete branching flow without
inventing a bound or a peripheral state transition. Each iteration performs
fresh MMIO reads and persistent `ReferenceMemory` effects, then returns a
scalar used only by the loop exit predicate. The pinned rev0
`phy_iq_est_enable` summary uses this form: it observes the estimator-done bit,
reads activity status only while not done, and increments the real 16-bit
`phy_param + 0x1ac` counter only for active iterations. It requires the
registered `phy_param` entry contract, exact SVD names for all four estimator
registers, address `0x2f8289d4`, 180-byte size and complete body digest
`0f2ae45a5762be934b704a677f4d650dcb84ee291a6ca0e840e11c64751bde60`.
The reference can therefore reproduce a supplied hardware response sequence,
but it makes no claim that the loop terminates for every possible peripheral
behavior.
A second reviewed loop form models a bounded symmetric calibration search as
four independently composed flows: initial read, setup, candidate write and
sample. The IR requires fixed flows to consume no outer arguments, the writer
to consume only its local candidate, both reads to have modeled scalar results,
and a nonzero per-direction attempt bound. The pinned 192-byte
`phy_rfpll_cap_init_cal` body uses two ten-step directions around the initial
`u16` cap, exact wrapping accumulation, the recovered sample mask, signed RV32
division of the nonnegative accumulated values, and the final write/delay.
Executable generated-reference tests cover all-accepted, none-accepted and
early-window termination paths. As with every reviewed summary, a name,
address, size or digest mismatch disables this lowering.
A statically addressed
MMIO access must name an exact SVD register; membership in a broad SVD address
window alone is not enough. Input-indexed MMIO is generated in two bounded
forms. If the address depends on at most eight input bits, the extractor
enumerates every combination and requires all resulting addresses to belong to
one exact SVD register family. Otherwise the expression must be affine in one
ABI argument and indices starting at zero must form a contiguous SVD register
bank; generation is capped at 32 registers and emits an explicit maximum-index
assertion. Both forms also emit a runtime address allowlist assertion and keep
indexed reads as distinct symbolic identities for later RMWs and return values.
An arbitrary pointer, a gap in the bank, a second input, an unrelated register
family or a merely window-mapped address remains fail-closed. These assertions
are recovered reference preconditions, not proof that a production Rust API
should accept an untyped `u32` index.

A statically addressed RAM access is generated only when its complete width
belongs to a real alloc section of a linked ELF. The extractor also preserves
loads and stores whose address is an affine `argument + constant` expression as
caller-owned ABI RAM.
Those accesses retain the runtime address in the generated Rust instead of
guessing the layout of `phy_param` or another C context. The `ReferenceMemory`
implementation must bound them to explicitly declared CPU-owned ranges and
reject MMIO or undeclared memory. A pointer loaded from RAM/MMIO, returned by an
unmodeled call or produced by non-affine arithmetic does not inherit that
provenance and remains fail-closed. Relocatable archives preserve matched
`R_RISCV_HI20` plus `R_RISCV_LO12_I`/`R_RISCV_LO12_S` data addresses as
`archive member + symbol + high/low addends`. Generated references ask
`ReferenceMemory::symbol_address` for the address in the exact linked scenario
and reproduce the RV32 HI20/LO12 rounding formula, including pairs whose high
and low addends differ. The memory adapter must reject an absent/ambiguous
symbol and a write to a read-only resolved section. A missing or mismatched
pair, unexpected encoded low addend, and unsupported GOT/PC-relative/TLS
relocation remain blockers. The symbolic extractor treats compiler-private stack
slots as internal temporary storage, so register spills do not leak into the
generated `ReferenceMemory` contract. For straight-line composed calls it also
models a private stack object explicitly: a callee may read or write an exact
affine address derived from the caller's stack pointer, and the resolver replays
those effects before eliminating every private-stack event. This supports
caller-allocated output slots without exposing them as C-style public memory.
The slot must be definitely initialized before every read, and no stack-derived
value may survive into generated code. Branch-dependent callee memory effects,
non-affine stack addresses, an uninitialized read, or any escaping stack pointer
remain fail-closed. Loads from the first eight aligned words at or above the
entry stack pointer are instead modeled as RV32 arguments 9 through 16, and
outgoing values in those slots are substituted across direct calls. Access
beyond that explicit bound remains fail-closed. A pointer reloaded from a stack
slot after a linear call may defer its RAM access until call composition, but
the effect is retained only if the reconstructed address is still an affine
caller-owned argument address. A callback/MMIO value, constant device address,
or any other lost provenance remains fail-closed. Use the linked vendor-oracle
ELF instead of `libphy.a` when generating state accessors.

A reviewed terminal wrapper may bind one callee argument to a bounded private
scratch object instead of exposing that pointer through `ReferenceMemory`.
Generated scratch is at most 256 bytes, delegates only wholly disjoint accesses
to the outer memory adapter, rejects partial overlap, and tracks definite byte
initialization before every 8/16/32-bit little-endian read. The pinned
16-byte `phy_set_rf_freq_offset` wrapper uses five bytes for the SDM values
written and consumed inside `phy_set_rfpll_freq`; its exact name, address, size
and digest gate the lowering. This scoped form does not publish a C struct or
permit a scratch pointer to escape the composed call.

Unresolved archive relocations named exactly `memcpy` or `memset`, and pinned
ROM bodies with the same exact names, receive a standard-library summary only
when the call has the ordinary RV32 return-link shape and its byte count is a
proven constant no larger than 256. A ROM summary additionally requires the
expected load address and complete body digest, so a changed implementation
falls back to ordinary analysis. `memcpy`
snapshots every source byte before publishing destination writes; `memset`
retains the low byte of its value argument; both preserve the C return value.
The generated reference records the resulting byte-level `ReferenceMemory`
effects, while private-stack-only bytes remain internal. A dynamic or excessive
length, an unproven pointer, MMIO, or a read-only destination remains a named
blocker. This bounded lowering avoids treating a libc implementation loop as
vendor behavior and is deliberately not a general license to dereference
arbitrary pointers.

The pinned ESP32-S31 rev0 ROM `__divdi3` body has a separate reviewed RV32
summary. It reconstructs each signed 64-bit operand from `a1:a0` and `a3:a2`,
preserves both quotient result words in `a1:a0`, and emits one ordered helper
call even when later code consumes both words. The helper uses wrapping signed
division and asserts the recovered nonzero-divisor precondition. The summary is
enabled only for the exact symbol name, ROM load address, 926-byte body and
complete SHA-256 digest; any changed implementation falls back to structural
analysis. This is an ABI-specific intrinsic, not a general assumption that an
arbitrary call's `a1` contains a second return word.

Mutable global pointer cells require an explicit lifecycle contract. The
default `--entry-contract none` makes no claim about their runtime contents.
`esp32s31-phy-cold` models `rom_phyFuns` as the pinned rev0 ROM function table.
`esp32s31-phy-registered` additionally models `g_phyFuns` after
`phy_get_romfunc_addr` and `phy_param_rom` after it has been redirected to the
linked `phy_param` object; table replacements are resolved by symbol in the
exact linked ELF. Merely finding those symbols in the ELF never activates the
contract. Analysis reports and batch manifests record the selected contract,
so cold-start and post-registration results cannot be mixed silently.

The reference resolver composes returning direct calls and terminal direct
tail-calls when every target is a known eligible symbol in the primary or a
companion linked image. Straight-line callees are flattened with argument,
return-value and MMIO/RAM token remapping. A callee with symbolic control flow
is instead emitted as a nested call-flow block with its own read/callback token
scope. Only the ABI arguments actually consumed by that callee are captured
before entering the block, and its modeled `a0` can feed later caller
arithmetic, writes or branches. An unmodeled callee `a0` is allowed when it is
discarded or only becomes the unresolved top-level exit value, but fails closed
if caller-visible behavior depends on it. Every composed symbol is recorded in
the generated provenance header. This turns small call graphs into one
executable reference without reproducing the vendor C function boundaries.
Repeated `--companion PATH` options resolve
`R_RISCV_CALL`/`R_RISCV_CALL_PLT` targets by symbol name across ELF images;
every companion path and SHA-256 is recorded in the output. The resolver also
models `ets_delay_us` as an ordered `ReferenceIo::delay_micros` action and
follows exact local unconditional jumps. It recognizes the side-effect-free
single-read MMIO polling form above while rejecting other loops. Constant
conditions follow only their feasible edge; resolved symbolic conditions are
explored on both edges and rebuilt as structured reference flow. Constant
arguments from a particular call site specialize the child for that call
without changing the generated ABI expressions. Direct and tail calls may
appear before a branch, inside either arm, or in a branch condition through a
modeled callee result. Recursion, unresolved targets, stack-pointer arguments
and unbounded control flow remain fail-closed, except for calls proven to come
from a registered external ABI table.

Relocatable archives retain `R_RISCV_CALL` and `R_RISCV_CALL_PLT` with the
owning member, function and instruction site. A target is composed only when
its definition is unique (preferring an exact same-member definition); an
absent, ambiguous or nonzero-addend target stays unresolved. Registered
diagnostic `wifi_log` calls remain explicit `ReferencePlatform` events instead
of being silently discarded.

The first registered table is the ESP32-S31 Wi-Fi OS adapter v9. For both
relocatable archives and linked ELFs, the resolver recognizes the exact chain
`g_osi_funcs_p -> fixed slot load -> JALR`. The table description pins the
target header commit and SHA-256, version `9`, magic `0xDEADBEAF`, 512-byte
size and slot offsets. Generated references expose modeled callbacks through
`ReferencePlatform`, assert the version/magic/size precondition, and retain
nondeterministic callback results as symbolic values. `_env_is_chip`, `_rand`,
`_random` and `_slowclk_cal_get` are modeled; `_coex_pti_get` is identified by
name and modeled only for a one-byte output pointer into the current
function's private stack. Generated references obtain that byte from
`ReferencePlatform`; the callback's integer status remains unresolved, so any
status-dependent behavior fails closed. A non-stack output pointer is also
rejected. This represents the real callback contract without accepting the
pinned no-COEX stub which returns without initializing its output byte.
Unknown table pointers, offsets and callback effects are never guessed.

For example, the vendor `hal_random` tail call is now a compilable reference
over the `_rand` callback at offset `0xbc`:

```console
cargo phy-trace generate-reference \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --artifact _oracles/libpp.a \
  --member hal_mac.o \
  --symbol hal_random \
  --output /tmp/hal_random_reference.rs
```

A generated reference should be compiled as a probe and fed back through
`execute-compare`; successful generation by itself is not qualification
evidence and does not make the file production driver code.

Generate every currently eligible reference in one pass and retain the blocked
inventory as a machine-readable work queue:

```console
cargo phy-trace generate-reference-batch \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --artifact _oracles/esp32s31_rev0_rom.elf \
  --companion hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --symbol-prefix phy_ \
  --source-name rom \
  --entry-contract esp32s31-phy-registered \
  --output-dir /tmp/esp32s31-rom-references
```

The output directory contains one self-contained, warning-clean Rust reference
per eligible function and `manifest.json`. The manifest pins the artifact and
companion digests, records the proposed verifier probe symbol, dependencies and
return-value status for generated candidates, and preserves the complete
failure reasons for every blocked function. Existing output is not overwritten
unless `--force` is passed. The generated files remain behavioral reference
models: a human-owned typed adapter is still required before a function becomes
production driver code or qualification evidence.

For example, the complete two-path `hal_timer_update_by_rtc` reference is now
generated directly from the archive. Its disabled arm clears the RTC update
bit; its enabled arm sets that bit and publishes the low 18 calibration bits:

```console
cargo phy-trace generate-reference \
  --svd svd/esp32s31-radio.svd \
  --svd svd/esp32s31-platform-radio-deps.svd \
  --artifact _oracles/libpp.a \
  --member hal_tsf.o \
  --symbol hal_timer_update_by_rtc \
  --output /tmp/hal_timer_update_by_rtc_reference.rs

rustc --edition 2024 --crate-type lib -D warnings \
  /tmp/hal_timer_update_by_rtc_reference.rs
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
