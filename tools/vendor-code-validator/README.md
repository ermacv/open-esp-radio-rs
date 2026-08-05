# Vendor code validator

`vendor-code-validator` inspects, executes and compares compiled vendor and
Rust code, generates fail-closed Rust reference models and enforces final-image
call policies. The current backend parses RISC-V ELF/ar containers with the
Rust `object` reader and decodes RV32IMAC instructions directly from symbol
bytes; it does not invoke binutils or scan source text for addresses or
required function names.

Every command requires an explicit `--target-spec`. The ESP32-S31 target pack
at `validation/esp32s31/target.spec` selects `riscv32` + `riscv-ilp32`, the
platform harness, Rust recompilation target, composed SVD catalog, profiles,
dispositions and evidence baseline. It contains no vendor artifact paths or
accepted digests. Callers authenticate their inputs and pass paths directly or
through a separate `--run-spec`; computed content identities in reports are
descriptive provenance only.

The tool also provides an SVD-independent final-image policy check:

```console
cargo vendor-code-validator image audit-targets \
  --target-spec validation/esp32s31/target.spec \
  --artifact target/path/to/runtime-elf \
  --forbid 'radio-api=0x2f800bf0..0x2f8016bc' \
  --forbid 'radio-body=0x2f823c12..0x2f83e6d0'
```

It scans every executable section rather than named functions, resolves
constant `JAL`/`JALR` targets formed by ordinary RV32 immediate sequences and
fails when a target is inside a forbidden half-open range. Absolute linker
symbols do not fail by mere presence. Calls through runtime-loaded function
pointers are intentionally outside this binary check and must be governed by
the platform/effect contract. `tools/audit-source-only.sh` applies the pinned
ESP32-S31 ECO0 radio ranges to the final normal HIL ELF.

## MMIO discovery

`mmio discover` is a best-effort, artifact-wide inventory for reverse
engineering register blocks. It accepts multiple ELF/ar inputs and explicit
half-open address ranges independently of whether every address already has an
SVD register name:

```console
cargo vendor-code-validator mmio discover \
  --target-spec validation/esp32s31/target.spec \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --range phy=0x20100000..0x20110000 \
  --json-report /tmp/esp32s31-phy-mmio.json
```

The report groups statically addressed 8/16/32-bit reads and writes by
address, names known SVD registers, assigns stable `RANGE.REG_ADDRESS`
candidate names to unknown addresses, and lists every artifact/member/function
that used each register. For writes it reports output-bit provenance as
preserved, inverted, forced zero, forced one, derived from a register read, or
dynamic. `modified_mask`, `candidate_bit_ranges` and `field_candidates` are
mechanical data-flow facts; they do not claim field names, reset values, W1C
semantics or any other peripheral behavior. Field candidates combine partial
write masks, poll masks and MMIO-backed branch predicates, and link the
resulting bit ranges to access functions and guarded semantic actions for
manual analysis.

Discovery deliberately retains events recovered before unsupported control
flow and emits per-function diagnostics without failing the run. Its JSON says
`"analysis_mode": "best-effort"` and `"completeness_claim": false`. Use the
existing reference/verification workflows when a fail-closed completeness
claim is required. The initial discovery slice covers statically resolved
addresses; indexed and pointer-derived range recovery remains part of the
reference analyzer rather than this inventory.

Input-dependent conditional branches are explored in both directions with
explicit bounds of 127 symbolic states and 12 decisions per path. Artifact
summaries report explored states, terminal paths and distinct branch sites;
exhausting either bound produces an `exploration` diagnostic. Access counts use
the maximum multiplicity of an observable shape on any explored path, rather
than summing paths and double-counting their common prefix. The JSON records
this as `"access_count_mode": "maximum-per-path"`.

## Linked function IR

`ir export` produces a separate best-effort representation for manual code
reading. It uses the reference resolver to link direct ELF targets, archive
`R_RISCV_CALL`/`R_RISCV_CALL_PLT` relocations, structured conditional flows,
and harness-known external function-table calls:

```console
cargo vendor-code-validator ir export \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_LIBPHY_ARCHIVE" \
  --symbol-prefix phy_ \
  --include-reachable \
  --pseudo-rust /tmp/libphy.pseudo.rs \
  --json-report /tmp/libphy.ir.json
```

By default the prefix selects only report roots. `--include-reachable` also
exports the transitive internal callees recovered from those roots within the
same primary artifact. Each function is marked `symbol-prefix-root` or
`reachable-internal`, and schema v27 records the selection mode plus root and
included-callee counts. This is an opt-in analysis-size tradeoff: only exactly
resolved internal edges enqueue a callee, exploration limits remain visible as
blockers, and companion or independently named primary definitions are not
silently imported into the closure.

A project inventory can aggregate several independently linked or relocatable
inputs in one report. Multiple inputs must have stable source names:

```console
cargo vendor-code-validator ir export \
  --target-spec validation/esp32s31/target.spec \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --artifact libpp="$ESP32S31_LIBPP_ARCHIVE" \
  --pseudo-rust /tmp/vendor-project.pseudo.rs \
  --json-report /tmp/vendor-project.ir.json
```

Project identities are namespaced, for example `rom::ets_delay_us` and
`libphy::phy_init`. Semantic boundaries and all summary counts are aggregated
across sources. Each named primary is analyzed in its own address space;
schema v27 records `"linkage_mode": "independent-artifacts"` and does not claim
that separate inputs share an address space or were fully linked. Use one
linked ELF primary plus `--companion` inputs when cross-image addresses and
relocations belong to one executable address space.

Project mode does perform a conservative symbol-level call association. An
unresolved call relocation becomes `project-linked` only when exactly one
exported definition with that symbol exists across all inputs. Multiple weak or
global definitions remain ambiguous, and local definitions are never selected.
`project_call_linkage` records this policy. The edge is useful for navigation,
but arguments, return propagation and addresses are not substituted, so the
original reference blocker and incomplete function status remain intact.

Call records are compacted by stable call identity: kind, target, recovered
static site when available, ABI/semantic contract and typed signature.
Distinct symbolic argument forms for that identity are reported as
`argument_shapes` instead of duplicating the entire call record, while the
report summary retains their total as
`call_argument_shapes`. An argument value that differs is rendered as
`varies-across-N-shapes`. An affine argument binding survives compaction only
when the exact same binding is present in every shape; otherwise it is omitted
and downstream context projection fails closed if the callee needs it. Shape
counts are distinct recovered IR forms, not runtime call counts or loop
iteration counts. The JSON records this policy as
`call_compaction_mode: "stable-identity-universal-affine-bindings"`.

Exploratory blocker messages can contain thousands of repeated exact clauses
when branch recovery reaches the same unsupported call or jump through many
symbolic states. Schema v27 records
`diagnostic_compaction_mode: "exact-semicolon-fragment-inventory"`. Each
function's structured `diagnostics` keeps the original fragment count, every
unique exact fragment, its number of occurrences and its first ordinal. The
legacy blocker arrays and pseudo-source use the compact `rendered` form with an
explicit `[repeated N times]` suffix. This is mechanical report compaction, not
semantic parsing: later duplicate ordering is not retained, counts are not
runtime occurrence counts, and backend completeness remains fail-closed.

Every function also has an `effect_summary` reachable inventory. It follows
resolved internal and unique project edges to a fixed point, including through
recursive components, and groups MMIO access shapes, delay shapes and semantic
operations by their originating functions. `call_graph_closed` is true only
when every reached function body and traversed edge is complete; otherwise
`blockers` names the incomplete bodies, unresolved edges or omitted callees.
This is deliberately not an effect-equivalence proof: counts describe recovered
IR shapes and mutually exclusive paths may coexist. Top-level
`effect_summary_mode: "reachable-inventory-origin-preserving"` makes that
boundary machine-readable.

Internal call records retain proven affine pointer bindings such as
`callee arg0 = caller arg2 + 0x20`. The effect summary composes those bindings
along simple call paths and projects callee context fields back into the root
function's argument/offset coordinates. Projected fields retain access counts,
write masks, values, complete call paths and originating functions. Dynamic or
missing bindings are never guessed; they produce `context_projection_blockers`
when the reached callee actually accesses that argument. Recursive paths stop
before revisiting a function, and projection is capped at 4096 path states to
avoid unbounded affine offsets or combinatorial output. The top-level
`context_projection_mode: "affine-simple-call-paths"` and per-function
`context_projection_complete` expose these limits. Exact access shapes seen in
both an already-composed caller flow and the separately analyzed callee are
counted once while retaining both provenance paths.

Every function has structured `return_provenance` in addition to the canonical
`return_value` string. Constant-zero, constant-one and unknown output bits are
separate masks. Dynamic bits are grouped into exact contiguous mappings from
output ranges to argument, MMIO-read, indexed-MMIO, memory, private-stack or
call-result source ranges. Each mapping retains source/output masks, bit
positions, width, inversion, read token, resolved call target and concrete SVD
register identity when available. `exact` means that all 32 bits of the
recovered return value have known symbolic provenance; it does not imply that
the function body or call graph is complete. The top-level
`return_provenance_mode: "exact-bit-ranges-with-constant-and-unknown-masks"`
records this distinction.

The same path walk emits `semantic_actions`: one record for every recovered
semantic call on every explored simple call path. Each action retains its
origin, static call site, target, typed argument values, replacement hint and
any affine projection back to root arguments. It also carries the exact
contract source, stable ID and evidence rule that justified the semantic name.
The `site_path` array records the lexical call-site chain from the report root
to the action, and actions are stably ordered by that chain.

Direct call records additionally expose recovered `cfg_guard_paths` in
disjunctive normal form: paths are alternatives and the decisions inside one
path are conjunctive. During semantic projection these are retained as
`cfg_guard_scopes`, with an AND between function scopes and an OR between the
paths inside one scope. Keeping the formula factorized avoids an artificial
cross product across nested calls and preserves the function in which each
decision was made. Complementary alternatives and absorbed supersets are
minimized mechanically; conditions themselves remain low-level symbolic
expressions and call results receive stable descriptive identifiers rather
than guessed domain names. Aligned bit provenance from one value is rendered
losslessly as a normal mask expression such as `(result & 0x0000000f)`;
mixed, shifted or otherwise non-uniform provenance remains an explicit
`symbolic(...)` expression. `cfg_guard_expression_mode` records this policy.
Each structured guard atom also carries `result_sources`: the call-result kind,
trace-local token, resolved producer target, operand, compared-value bits and
exact source-bit mask recovered from symbolic bit provenance. Equality and
inequality against a constant additionally retain the visible comparison value
and map it into producer-result coordinates. The target is the same stable
identity used by `functions[].identity`, so consumers can join a guard operand
to the producer's return value and MMIO inventory without parsing the rendered
expression.
Missing producer resolution remains `null`, and the
`cfg_guard_result_source_mode` value
`"bit-provenance-with-operand-comparison-mapping-and-producer-targets"` makes
the join contract explicit. When the selected producer returns bits directly
from a concrete MMIO read, each result source also contains `mmio_sources`
with the intersection between the tested result mask and the producer's return
mapping.
It records both result-bit and register-bit masks, comparison values in both
coordinate systems, address, SVD name and composed inversion. This is
deliberately direct rather than transitive through another returned call
result, and an absent or non-MMIO mapping stays an empty array.
The top-level `cfg_guard_mmio_linkage_mode` value
`"two-stage-exact-bit-projection-with-comparison-values"` identifies that
boundary.

The top-level
`semantic_action_mode: "lexical-site-paths-factorized-cfg-guards-affine-root-bindings"`
and `cfg_guard_mode` describe this representation. A `null` guard field means
that CFG evidence was unavailable; an empty path means unconditional with
respect to the recovered decisions. `cfg_guard_completeness_claim: false` is
intentional: the guards are evidence from explored forced branch decisions,
not a claim that bounded symbolic exploration enumerated every feasible path.
Likewise, a `null` site means the backend recovered a composed boundary but not
a standalone instruction address. This remains a path-qualified
manual-analysis inventory, not a total runtime order. Mutually exclusive paths
can both contribute actions, loops are represented by recovered shapes rather
than iteration counts, and recursive revisits stop at the projection boundary.

The pseudo-Rust intentionally uses `u32` argument placeholders and is not
compilable output. It renders recovered MMIO/RAM effects, delays, polls,
branches, internal calls, diagnostic calls, scratch buffers, and named
external ABI calls. External call records include the table version, slot,
argument count and reviewed return model. Unsupported instructions and
incomplete control flow remain adjacent `DIRECT-BLOCKER` or
`REFERENCE-BLOCKER` comments instead of being guessed.

Linked IR connects code and register analysis directly. Every function has
`mmio_accesses` records containing the exact or candidate address, SVD name,
width, read/write/poll kind, branch/call path, symbolic address and value, and
write-bit provenance masks. Their `ordinal` preserves recovered flow traversal
order, including repeated accesses; mutually exclusive paths remain identified
by `path`. The top-level `mmio_registers` index groups these
facts by `(address, width)` and lists all using functions. Counts are explicitly
access *shapes*, not runtime execution counts. An indexed candidate records one
possible register selected by a proven bounded address expression; it is not a
claim that every candidate is touched in one invocation.

For writes, the register index retains every distinct `write_mask`, counts
whole-register and read-modify-write shapes, and splits modified masks into
contiguous `candidate_bit_ranges`. Each range lists the functions that produced
it. This write-only inventory remains available for compatibility.

Schema v27 exposes `field_candidates`. It merges equal contiguous subregister
ranges recovered from four independent evidence classes: write masks, poll
predicates, direct local MMIO branch conditions, and guard-result links to a
producer function's MMIO-backed return bits. Every candidate keeps separate
shape counts, the functions that accessed or tested it, and semantic operations
and report roots whose recovered call paths are guarded by it. A semantic
operation is therefore a navigation link, not a proposed register-field name
or behavior.

Every function's `direct_mmio_predicates` records branch site, normalized
comparison operation and exact bit provenance for an MMIO operand. The source
retains both bits in the compared value and their original register positions,
so shifted fields remain traceable. If the other operand is constant and the
operation is equality or inequality, `register_comparison_value` maps that
constant back into register bit positions, accounting for bit inversion. A
non-constant operand or relational comparison remains explicit with a null
register value rather than being guessed. The same structured evidence appears
under field candidates as `predicate_evidence`, together with poll and
producer-return evidence. Guarded producer-return evidence also records
`taken` and `effective_operation`; a false branch complements the supported
comparison operator instead of hiding path polarity. `semantic_evidence`
identifies the concrete semantic action by target, origin, call path, call site
and full lexical site path. Scope/path indices form stable coordinates into the
action's factorized `cfg_guard_scopes`; the evidence also retains the selected
DNF alternative, guard position and `residual_path_expression` after removing
the MMIO literal. JSON and tabular indices are zero-based; pseudo-source labels
are one-based. Thus opposite bit polarities on different action sites or under
different remaining conditions stay distinguishable without duplicating the
entire action guard for every field. The compact operation/root sets remain
available as an index. The `semantic_field_guard_mode` value is
`"action-identity-and-path-coordinate-preserving"`.

Zero and whole-register masks never create field candidates; whole-register
writes, predicates and polls have separate counters. Discontiguous masks become
separate contiguous candidates, so adjacent hardware fields can still be merged
and one logical field can still be split. Guard evidence is indexed only when
the address has one unambiguous observed access width. Direct predicates require
recoverable per-bit provenance; producer predicates follow the exact direct
producer-return linkage described below. Neither path guesses through unknown
arithmetic or transitive calls. No access policy such as W1C, reset value,
field name or peripheral semantics is inferred. The JSON records this scope
with `direct_mmio_predicate_mode`,
`direct_mmio_predicate_completeness_claim: false`,
`mmio_field_candidate_mode` and `mmio_field_semantics_claim: false`.

External ABI slots carry a harness-owned semantic overlay: an opaque operation
name, typed/named input/output arguments, return type and optional replacement
hint. The ESP32-S31 Wi-Fi OSI v9 contract currently identifies ISR queue
notifications, task delays, event posting, microsecond timers, NVS
open/commit/blob operations, logging, randomness, clock calibration and
coexistence PTI queries. The table also records critical-section enter/exit
slots even though their interrupt-state effects remain deliberately unmodeled.
Pseudo-Rust renders these as calls such as
`semantic.rtos_queue_send_from_isr(...)` while retaining table version and
slot. Slots whose meaning is known but whose complete memory/scheduler effects
are not modeled emit `unmodeled-external-semantics`; their opaque return data
flow is preserved for reading, but the function remains incomplete for
validation and reference generation.

The report also builds a top-level `semantic_boundaries` index. It groups each
operation across artifacts by calling functions, concrete ABI targets and
replacement hints, so RTOS, timer, NVS and logging dependencies can be audited
without first reading every recovered function body.

Semantic boundaries are not limited to callback-table slots. A platform
harness may assign a typed operation to a direct internal vendor function only
after an exact reviewed body-and-relocation fingerprint matches. For the
ESP32-S31 `libpp.a` contract this recognizes `pp_post(signal)` as
`wifi.internal-signal.post`; a changed body, member, symbol layout or relocation
schema stays an ordinary internal call. This lets callers such as
`wDev_ProcessFiq` expose the recovered signal argument without pretending that
the helper is an RTOS event API or weakening its ordinary body analysis.

Known function-table calls additionally carry a structured `trampoline`
record. It includes the table pointer and backing symbols, version, magic and
size contract, exact slot, C name, argument count, stable return model, typed
semantic arguments and replacement hint. The top-level `trampoline_slots`
index groups all observed calls by that ABI identity and lists their calling
functions. This inventory is deliberately restricted to loads proven to start
from a harness-registered table pointer and to select an exact slot in the
registered version; arbitrary indirect calls are not assigned a trampoline
identity.

Reachable effect summaries also contain `trampoline_calls`. Their complete call
path and origin are retained, and pointer arguments with proven affine
provenance are projected into the root function as `ctxN + offset`. Pointer
types are recognized conservatively from the reviewed external signature;
dynamic pointers remain `not-affine-caller-context`, scalar arguments remain
`non-pointer`, and failed affine composition closes
`context_projection_complete` with an explicit blocker. These bindings are a
manual-analysis aid, not a claim about pointee layout, lifetime, scheduler or
storage effects. In particular, an `Unmodeled` ABI return/effect remains a
validation blocker even though its name and signature are known.

Pointer-relative RAM is rendered as an inferred context view. For example, an
access rooted at ABI argument 0 becomes `ctx0.read16(+0x8)` or
`ctx0.write32(+0x4, value)`. Each function also receives `context_fields` and
`context_accesses` JSON records. A field groups `(argument, byte offset,
width)`, counts reads/writes and unions the observed write mask; individual
accesses preserve their branch/call path, symbolic value and, for recognized
read-modify-write values, preserved/forced-zero/forced-one masks. These are
layout and data-flow facts only: the tool does not infer a C type name, field
name, ownership, validity invariant or concurrency semantics.

The JSON is the machine-readable linked view: each function contains artifact
identity, `global-or-weak` or `local` binding, address or relocatable object
offset, flow quality, dependencies, direct call edges, symbolic arguments,
blockers and the same pseudo body. `ir export` deliberately loads sized local
text symbols as well as exported global/weak functions, while validation and
qualification commands retain their narrower exported-symbol inventory. A
local call resolved through an archive `R_RISCV_CALL` relocation is therefore
linked to its private callee. Repeated local names in a linked ELF are given an
`@0x...` address suffix so identities and call targets remain deterministic.

This is still symbol-guided recovery, not a proof that every executable byte
has a function boundary: stripped functions, zero-sized labels, hand-written
code without `STT_FUNC` metadata, jump tables, and undiscovered indirect calls
can be absent. As with MMIO discovery, the report declares
`"completeness_claim": false`.

## Internal architecture

The binary entry point only translates the library result into an exit code.
The implementation is split by responsibility:

- `crates/core` is a zero-dependency, architecture-neutral contract model;
- `crates/model` owns architecture-neutral symbolic/reference IR, indexed-MMIO
  proofs and the SVD-derived register catalog;
- `crates/semantic` owns architecture-neutral effect-policy, qualification
  request/result and evidence-source interfaces;
- `crates/backend-riscv` owns ELF decoding, RISC-V relocations, reference CFG
  analysis, code generation, concrete RV32 execution and image auditing;
- `crates/harness-esp32s31` owns the external ABI versions and lifecycle
  fixture data and depends only on core;
- `crates/harness-esp32s31-semantic` owns reviewed summaries, typed
  qualification and the only validator-side dependency on the production PHY;
- `analysis` contains the thin architecture-facing artifact service;
- `orchestration` owns cross-layer workflows such as compiling and
  independently re-extracting a generated reference;
- `verification` owns profiles, dispositions, evidence and comparisons;
- `harnesses::esp32s31` is a thin registry facade over the two ESP32-S31
  harness crates;
- `validation/esp32s31` owns the checked target/profile/disposition data;
- `cli` parses a typed top-level command and dispatches it to those services.

The backend depends only on the neutral core/model crates. Chip-specific
secondary-return recognition and reviewed summaries are supplied through the
typed `RiscvHarnessSpec`; the backend contains no platform registry. The
facade does not depend directly on the production PHY: that dependency ends at
the ESP32-S31 semantic harness boundary.

The hierarchical workflows are `inspect`, `mmio`, `ir`, `reference`, `execute`,
`verify`, and `image`. Legacy flat command spellings remain accepted during
the migration. The remaining orchestration and additional-backend work is
tracked in
[`docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md`](../../docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md).

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
external call identities, and an embedded provenance note for its source
archive. The note and content identities printed by the validator are
informational. The caller must authenticate the ROM, archive, linked image and
companions before invocation; the validator has no built-in artifact
allow-list. The original `libphy.a` remains the authoritative function
inventory;
the linked ELF supplies executable code, while the ROM ELF supplies callable
ROM code as its companion. ROM data symbols used by relocations are assigned
their real ECO0 addresses by the linker script. An unresolved alloc-section
data/GOT/HI20/LO12 relocation is rejected instead of being executed as zero.

Execute a linked RV32 ELF with concrete arguments and a deterministic MMIO
scenario:

```console
cargo vendor-code-validator execute run \
  --target-spec validation/esp32s31/target.spec \
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
compiled probe while the default remains poison. A qualification using this
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

Compare linked vendor and Rust implementations under the same scenarios:

```console
cargo vendor-code-validator execute compare \
  --target-spec validation/esp32s31/target.spec \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
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
cargo vendor-code-validator verify contract channel \
  --target-spec validation/esp32s31/target.spec \
  --vendor-artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --vendor-companion "$ESP32S31_ROM_ELF"

cargo vendor-code-validator verify contract rf-init \
  --target-spec validation/esp32s31/target.spec \
  --vendor-artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
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

Summarize all vendor functions and every construct not covered by the direct
trace engine:

```console
cargo vendor-code-validator inspect analyze \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_ROM_ELF"

cargo vendor-code-validator inspect analyze \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_LIBPHY_ARCHIVE" --symbol-prefix ''

cargo vendor-code-validator inspect analyze \
  --target-spec validation/esp32s31/target.spec \
  --artifact \
    hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
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
cargo vendor-code-validator reference generate \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_ROM_ELF" \
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
target harness.

Mutable global pointer cells require an explicit lifecycle contract. The
default `--entry-contract none` makes no claim about their runtime contents.
`esp32s31-phy-cold` models `rom_phyFuns` as the reviewed rev0 ROM function table.
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
`g_osi_funcs_p -> fixed slot load -> JALR`. The target harness declares
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
cargo vendor-code-validator reference generate \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_LIBPP_ARCHIVE" \
  --member hal_mac.o \
  --symbol hal_random \
  --output /tmp/hal_random_reference.rs
```

A generated reference must be compiled as a probe and fed back through the
validator; successful generation by itself is not qualification evidence and
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
cargo vendor-code-validator reference generate-batch \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_ROM_ELF" \
  --companion hil/vendor-oracle/esp32s31/target/riscv32imafc-unknown-none-elf/release/open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --symbol-prefix phy_ \
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
production driver code or qualification evidence.

For example, the complete two-path `hal_timer_update_by_rtc` reference is now
generated directly from the archive. Its disabled arm clears the RTC update
bit; its enabled arm sets that bit and publishes the low 18 calibration bits:

```console
cargo vendor-code-validator reference generate \
  --target-spec validation/esp32s31/target.spec \
  --artifact "$ESP32S31_LIBPP_ARCHIVE" \
  --member hal_tsf.o \
  --symbol hal_timer_update_by_rtc \
  --output /tmp/hal_timer_update_by_rtc_reference.rs

rustc --edition 2024 --crate-type lib -D warnings \
  /tmp/hal_timer_update_by_rtc_reference.rs
```

Verify every ROM function against a conventionally named Rust probe and report
missing probes as uncovered work:

```console
cargo vendor-code-validator verify source \
  --target-spec validation/esp32s31/target.spec \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
  --rust-artifact \
    hil/esp32s31/target/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-hil-esp32s31-trace-probes-elf
```

Generate the authoritative combined report for both vendor sources:

```console
# First copy validation/esp32s31/run.spec.example to an untracked local file
# and replace its placeholder paths with authenticated inputs.
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --run-spec /path/to/local-esp32s31.run \
  --gate regression --match-floor 104 \
  --json-report oracle-regression.json
```

Run-spec paths are resolved relative to that file. Explicit command options
override the same role from the run spec, and repeated `input companion` lines
remain available for workflows that accept multiple companions. Authentication
is intentionally outside the validator; the protected oracle workflow checks
caller-configured SHA-256 values before it creates its run spec.

`verify-all` treats `(vendor source, symbol)` as function identity. This is
necessary because `phy_fe_reg_update` exists in both ROM and `libphy.a`; the
two implementations remain separate rows and the inventory total is therefore
305 + 161 = 466, not the 465 unique spellings. It emits per-source summaries
and one `TOTAL-SUMMARY`. Rust probes are checked against the combined
inventory, so a probe belonging to one source is not falsely reported as an
orphan while the other source is being processed.

The disposition manifest classifies every exact implemented replacement and
uses fail-closed defaults for the rest. A `semantic-contract` names executable
validator logic; an `effect-contract` names the canonical effect comparator.
A Rust component without either remains
`IMPLEMENTED-UNQUALIFIED` and does not count as evidence. Executable root
contracts are reported separately as `composition-match`: they compare their
declared action/state projection but do not imply an independent proof for
every transitive leaf. Such an entry can use
one or more `blocked-by SOURCE SYMBOL` directives. The verifier rejects a
missing blocker target and prints the source-qualified blockers in the report,
so an architectural root cannot hide an unported child behind prose. Protocol
classification is independent, so shared PHY/RF, Wi-Fi, Bluetooth, BLE, Coex
and 802.15.4 scope are not inferred from completion status.

Effect Contract v1 is the common boundary between a vendor implementation and
a Rust implementation. Its closed effect vocabulary is MMIO read/write,
projected state read/write, delay, await-ready, typed platform call and four
named semantic-boundary events. Each vendor effect must resolve to one exact
manifest rule with one of these closed dispositions: `required`,
`replaced-by-async`, `platform-provided-input`, `platform-provided-service`,
`published-event`, `initialization-prerequisite`, `platform-owned`,
`forbidden`, or `allowed-omission` with one of `debug-diagnostic`,
`nvs-calibration-cache`, `rtos-scheduling-adapter`, and
`unused-instrumentation`. The four semantic replacements require the compiled
Rust trace to publish the exact named boundary; declaring one is not permission
to silently omit the vendor effect. This is how a constructor-supplied MAC
address, an Embassy wake service, a typed driver event or a separately proven
MAC-clock prerequisite replaces vendor-owned eFuse/RTOS/global-init behavior.
Unknown effect kinds, platform operations, omission reasons, unclassified
vendor effects and extra Rust effects are errors. An async replacement also
fixes one named condition and one non-zero `attempts=COUNT` or
`deadline-us=COUNT`; an arbitrary await or a changed deadline cannot satisfy
the rule.

The first direct vertical slice is intentionally small and exact:

```text
function rom phy_disable_agc
disposition direct
rust-component open_esp_radio_esp32s31_hal::phy_agc::set_enabled
binding v1
rust-probe open_phy_trace_disable_agc
effect-contract exact-effects-v1
effect mmio-read 32 0x20107030 required
effect mmio-write 32 0x20107030 required
```

The verifier derives those two effects independently from the caller-supplied ROM ELF,
the recompiled generated reference and the compiled production Rust probe.
Binding v1 verifies that the exact `rust-probe` symbol exists in the supplied
Rust ELF. Input revision and authenticity are deliberately caller-owned. The
verifier selects that probe from the binding instead of falling back to the
naming convention. `compare-return true` additionally binds the observable
ABI return register; without it the contract compares effects only. The flag
is deliberately opt-in because a machine value in `a0` is not evidence that
an unavailable C prototype declared a return value.
The effect evidence digest covers the canonical binding, policy, comparator,
binding validator, generator, generated harness, normalized generated source,
re-extracted effects and exact Rust compiler identity, so weakening or changing
any part of the proof requires a reviewed baseline change. Local artifact path
spellings are excluded from the source identity, while computed content
identities remain included as descriptive provenance.

The blocking-to-async slice is now executable for ROM
`phy_iq_est_enable`. Its closed `esp32s31-iq-est-enable-v1` driver adapter
compares three things independently: concrete ROM execution, the release/LTO
probe compiled from the production HAL/PAC leaves, and the public actions of
`PhyDcIqEstimateTransition`. Three scenarios cover immediate ready, inactive
then ready, and active/inactive/ready; together they cover all four ROM branch
outcomes. The vendor `phy_param+0x1ac` halfword is projected onto the typed
`readiness_activity_edges` state field. The one-microsecond delay must become
`timer-1us deadline-us=1`, each live ready sample must become
`iq-estimator-ready attempts=10000`, and the typed timeout must traverse the
complete disable tail. The evidence also binds the generated reference source,
the selected vendor and release-probe code closures, scenario inputs, adapter,
transition, target-port, execution engine and comparator sources. Whole
artifact digests remain reported as caller-owned provenance, but unrelated
linked functions do not enter this adapter baseline.

`PROTOCOL-INVENTORY` reports `executable-bindings` separately from exact
disposition entries. This keeps migration honest: legacy semantic contracts
remain visible but are not presented as artifact/probe-bound until they adopt
Binding v1. A Binding v1 function containing an unresolved call cannot reach
effect comparison; the ordinary extractor marks the trace incomplete. Typed
call dispositions are added only when a pilot needs a deliberate composition,
async replacement, platform boundary, or closed omission.

The two verification gates answer different questions:

- `--gate regression --match-floor 104 --evidence-baseline PATH` passes when
  there are no mismatches, incomplete comparisons, or orphan probes, at least
  104 functions retain evidence, and every source-qualified baseline function
  retains the same evidence kind. A lost state proof cannot be hidden by a new
  scenario match elsewhere. New evidence is reported as `EVIDENCE-ADDITION`
  and does not require weakening the existing baseline. Profile evidence also
  contains a hash of the parsed scenario contract, its explicit ABI argument
  domain, and the parser, comparison, reachability and execution-engine
  sources. Narrowing inputs or `arg-range`, changing observations or scripted
  responses, or weakening the verifier therefore requires a reviewed baseline
  change.
  Composition evidence contains a SHA-256 over the contract label, scenario
  wiring, semantic normalizer/footprints and execution engine sources. Editing
  the validator itself therefore also requires an explicit baseline review.
- `--rust-prefix` scopes convention-paired probes and orphan accounting for a
  particular verification run. Exact Binding v1 probes remain selectable by
  their full symbol names even when they use another prefix. A focused pilot
  sharing one Rust probe ELF with other suites must set its own prefix so
  unrelated probes do not weaken or fail its orphan-probe gate.
- `--gate completion` (the default) additionally requires every vendor
  function in the selected inventory to have a matching Rust probe.

The explicit floor is mandatory for the regression gate so the total amount
of established evidence cannot silently decrease. The current ESP32-S31
inventory has 466 source-qualified functions; 104 have evidence. Of the
remaining 362, two are implemented architectural roots that still need
semantic contracts and 360 are classified `not-yet-ported`.

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

Each `MATCH` row reports `evidence=symbolic`, `evidence=effect-contract`,
`evidence=scenario`, `evidence=state`, or
`evidence=composition-state-scenario`. Symbolic equality proves the normalized
straight-line trace. Effect-contract equality additionally proves that every
effect has an explicit closed policy. Scenario equality proves only the
explicitly declared inputs plus complete branch-outcome coverage. State evidence
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
summary and reported input identity are the audit record; documentation is
reserved for tool operation and exceptional rules that cannot be encoded in
the verifier.

`verify` analyzes exactly the paths supplied by the caller. Artifact revision
and authenticity checks belong in the invoking CI job or local harness. A new
chip or ROM revision therefore adds a target/harness pack and an evidence
baseline, not a digest constant in the validator.

`extract` and `compare` remain available for focused investigation of one
symbol. Run the command without arguments for their complete syntax.

`cargo test --workspace --locked` does not require the ignored private oracle
directory. Decoder, memory and policy behavior use synthetic fixtures; the two
inventory-count checks report a skip when the private ROM/archive paths are
not supplied through the documented environment variables. The explicit
qualification commands above remain the required private-oracle integration
checks. The repository CI runs formatting,
workspace tests, strict validator-only clippy, PAC generation checks and the
source-only audit. A separate
`Private oracle regression` workflow runs only on protected `main` or manual
dispatch using a dedicated self-hosted runner and approved
`oracle-regression` environment; it uploads both text and JSON reports and
never executes pull-request code with proprietary oracle access.

No parity exceptions are currently accepted. A future exception belongs in
the verifier as a typed rule with exact artifact, symbol and behavior scope,
plus tests; it must never turn an unrelated incomplete trace into `MATCH`.
