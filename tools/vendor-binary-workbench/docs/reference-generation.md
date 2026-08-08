# Reference generation

## Fail-closed phase boundary

Reference generation has an explicit fail-closed phase boundary:

```text
FunctionAnalysis -> resolution/composition -> ResolvedReferenceProgram -> Rust codegen
```

The resolved program type has no variants for unresolved direct/tail calls or
temporary branch decisions and carries no blockers. Consequently incomplete
analysis cannot reach code generation by accident. Composition evidence hashes
the concrete verification, comparison and execution source modules involved
in each contract, not only their facade modules.

## Inventory and generation

Summarize all vendor functions and every construct not covered by the direct
trace engine:

```console
cargo vendor-binary-workbench inspect analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_ROM_ELF"

cargo vendor-binary-workbench inspect analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_LIBPHY_ARCHIVE" --symbol-prefix ''

cargo vendor-binary-workbench inspect analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact \
    verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --companion "$ESP32S31_ROM_ELF" \
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
branch paths, and records content identities for the primary and companion
artifacts. Event
validation failures name the exact event, operand role and unavailable value
class instead of collapsing to a generic eligibility error. The impact counts
overlap deliberately: a function can depend on more than one root blocker.

Use the linked vendor-oracle ELF plus ROM companion for semantic analysis of
`libphy`. The raw archive remains the authoritative inventory and relocation
source, but unresolved archive calls do not represent the final relaxed link
and companions are therefore rejected for a relocatable primary artifact.

Generate a safe, executable Rust reference for a supported symbol:

```console
cargo vendor-binary-workbench reference generate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_ROM_ELF" \
  --symbol phy_disable_agc \
  --output /tmp/phy_disable_agc_reference.rs

rustc --edition 2024 --crate-type lib \
  /tmp/phy_disable_agc_reference.rs
```

For an archive symbol, add `--member phy_init.o` when the symbol owner must be
selected explicitly. Without `--output`, the generated source is written to
stdout so it can be inspected or consumed by another tool.

## Generated program contract

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
unrolled, with a hard limit of 1,024 visits to any instruction; the resulting
ordered effects are accepted only if the loop actually terminates within that
bound. Symbolic loops, excessive iteration, path explosion, unsupported
instructions, unresolved write values and unmapped MMIO registers remain
fail-closed. After complete unrolling, two proof-driven CPU-RAM forms may be
rendered back into compact Rust loops: repeated 32-bit word reads followed by
little-endian byte writes, and calls to a pure four-byte little-endian loader
followed by 32-bit word writes. These forms retain the vendor access widths and
ordering; they are not replaced with `memcpy`. Any MMIO event, pattern
mismatch, non-contiguous range, or read/call token used after the candidate
loop disables compaction and leaves the ordered events explicit.
A backward branch
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
expected target identity, symbol and load address. Authenticating the selected
ROM image is a caller precondition. `memcpy`
snapshots every source byte before publishing destination writes; `memset`
retains the low byte of its value argument; both preserve the C return value.
The generated reference records the resulting byte-level `ReferenceMemory`
effects, while private-stack-only bytes remain internal. A dynamic or excessive
length, an unproven pointer, MMIO, or a read-only destination remains a named
blocker. This bounded lowering avoids treating a libc implementation loop as
vendor behavior and is deliberately not a general license to dereference
arbitrary pointers.

The ESP32-S31 rev0 ROM `__divdi3` body has a separate reviewed RV32
summary. It reconstructs each signed 64-bit operand from `a1:a0` and `a3:a2`,
preserves both quotient result words in `a1:a0`, and emits one ordered helper
call even when later code consumes both words. The helper uses wrapping signed
division and asserts the recovered nonzero-divisor precondition. The summary is
enabled only for the explicitly selected target plus the exact symbol name,
ROM load address and 926-byte body. This is an ABI-specific intrinsic, not a
general assumption that an arbitrary call's `a1` contains a second return
word. The caller must authenticate the complete ROM before selecting this
platform harness.

Mutable global pointer cells require an explicit lifecycle contract. The
default `--entry-contract none` makes no claim about their runtime contents.
`esp32s31-phy-cold` models `rom_phyFuns` as the reviewed rev0 ROM function table.
`esp32s31-phy-registered` additionally models `g_phyFuns` after
`phy_get_romfunc_addr` and `phy_param_rom` after it has been redirected to the
linked `phy_param` object; table replacements are resolved by symbol in the
exact linked ELF. Merely finding those symbols in the ELF never activates the
contract. Analysis reports and batch manifests record the selected contract,
so cold-start and post-registration results cannot be mixed silently.


## Resolver and call composition

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
every companion path and computed content identity is recorded in the output. The resolver also
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
`g_osi_funcs_p -> fixed slot load -> JALR`. The platform harness declares
version `9`, magic `0xDEADBEAF`, the 512-byte size and slot offsets. Generated
references expose modeled callbacks through
the target-neutral `ReferencePlatform::external_call(table, function, args)`
boundary, assert the version/magic/size precondition by table ID, and retain
nondeterministic callback results as symbolic values. The generated trait does
not acquire ESP32-S31 method names. `_env_is_chip`, `_rand`,
`_random` and `_slowclk_cal_get` are modeled; `_coex_pti_get` is identified by
name and modeled only for a one-byte output pointer into the current
function's private stack. Generated references obtain that byte from
the same opaque platform boundary; the callback's integer status remains unresolved, so any
status-dependent behavior fails closed. A non-stack output pointer is also
rejected. This represents the real callback contract without accepting a
no-COEX stub which returns without initializing its output byte.
Unknown table pointers, offsets and callback effects are never guessed.

For example, the vendor `hal_random` tail call is now a compilable reference
over the `_rand` callback at offset `0xbc`:

```console
cargo vendor-binary-workbench reference generate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_LIBPP_ARCHIVE" \
  --member hal_mac.o \
  --symbol hal_random \
  --output /tmp/hal_random_reference.rs
```

## Verification loop and batch generation

A generated reference must be compiled as a probe and fed back through the
workbench; successful generation by itself is not verification evidence and
does not make the file production driver code. Binding v1 automates this loop
for exact MMIO-only leaves: the verifier generates a concrete no-std harness,
compiles it for `riscv32imafc-unknown-none-elf`, extracts the resulting machine
code and first proves `generated reference == vendor ELF`. Only then does it
compare both traces with the bound production Rust probe. RAM, delays, polling
and platform callbacks are deliberately rejected by this first harness rather
than receiving placeholder implementations.

Generate every currently eligible reference in one pass and retain the blocked
inventory as a machine-readable work queue:

```console
cargo vendor-binary-workbench reference generate-batch \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_ROM_ELF" \
  --companion verification/vendor/targets/esp32s31/oracle-firmware/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --symbol-prefix phy_ \
  --probe-prefix open_phy_trace_ \
  --source-name rom \
  --entry-contract esp32s31-phy-registered \
  --output-dir /tmp/esp32s31-rom-references
```

The output directory contains one self-contained, warning-clean Rust reference
per eligible function and `manifest.json`. The manifest records artifact and
companion content identities, the proposed verifier probe symbol, dependencies and
return-value status for generated candidates, and preserves the complete
failure reasons for every blocked function. Existing output is not overwritten
unless `--force` is passed. The generated files remain behavioral reference
models: a human-owned typed adapter is still required before a function becomes
production driver code or verification evidence.

For example, the complete two-path `hal_timer_update_by_rtc` reference is now
generated directly from the archive. Its disabled arm clears the RTC update
bit; its enabled arm sets that bit and publishes the low 18 calibration bits:

```console
cargo vendor-binary-workbench reference generate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --artifact "$ESP32S31_LIBPP_ARCHIVE" \
  --member hal_tsf.o \
  --symbol hal_timer_update_by_rtc \
  --output /tmp/hal_timer_update_by_rtc_reference.rs

rustc --edition 2024 --crate-type lib -D warnings \
  /tmp/hal_timer_update_by_rtc_reference.rs
```
