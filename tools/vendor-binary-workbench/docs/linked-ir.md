# Linked function IR

`ir export` can run with an architecture-only target. In that mode it uses an empty ABI contract: ordinary ELF linkage and
instruction effects remain visible, while service-table slots and external
semantics remain unresolved. Selecting a platform pack through a project
enriches the same IR and does not change its exploratory completeness claim.

`ir export` produces a separate best-effort representation for manual code
reading. It uses the reference resolver to link direct ELF targets, archive
`R_RISCV_CALL`/`R_RISCV_CALL_PLT` relocations, structured conditional flows,
and harness-known external function-table calls:

```console
cargo vendor-binary-workbench ir export \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --symbol-prefix phy_ \
  --include-reachable \
  --pseudo-rust /tmp/libphy.pseudo.rs \
  --json-report /tmp/libphy.ir
```

## Persistent bundle and random access

Schema v45 is a directory, not one monolithic JSON document. The output path
contains `manifest.json`, `functions.jsonl`, `function-overview.jsonl`,
`function-index.json`, `graph.json`, `register-index.json`,
`data-objects.jsonl`, and `data-object-index.json`. Functions and data objects
are individually encoded records addressed by byte offsets from deterministic
indexes. The manifest contains identity, modes, provenance and summary only.

`function-overview.jsonl` is the strict compact projection used by status,
review validation and the TUI index. It contains identities, completeness,
direct-call/MMIO counts, projected context and memory fields, semantic
operations, event dispatches and decode blockers, but no instruction stream,
pseudo-code, paths or scenario bodies. A selected function detail is loaded by
one indexed seek from `functions.jsonl`; project startup therefore never scans
the lossless stream, which is about 1.1 GiB for the current ESP32-S31 project.

Focused consumers must use the relevant index. `inspect function` reads one
function record plus the graph index; `inspect object` reads one data-object
record; register review reads only `register-index.json`. Whole-project joins
may stream all function records, but do not parse the data-object inventory
unless they consume it. A bundle is valid only when every required member
exists and has schema v45; the removed single-file representation is rejected.

```console
cargo vendor-binary-workbench inspect function libpp:wDev_AppendRxBlocks \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --depth 2 --callers

cargo vendor-binary-workbench inspect object linked:g_wdev_control \
  --project path/to/vendor-project.toml
```

The default function view is semantic and bounded. `--full` additionally
prints the lossless CFG and every instruction, annotated at exact PCs with
call targets, semantic operations, decode blockers and stable blocker IDs.
Each semantic blocker names the reviewed model required to continue. Raw
instructions remain available even when symbolic execution stops.

## Selection and project linking

By default the prefix selects only report roots. `--include-reachable` also
exports the transitive internal callees recovered from those roots within the
same primary artifact. Each function is marked `symbol-prefix-root` or
`reachable-internal`, and schema v45 records the selection mode plus root and
included-callee counts. This is an opt-in analysis-size tradeoff: only exactly
resolved internal edges enqueue a callee, exploration limits remain visible as
blockers, and companion or independently named primary definitions are not
silently imported into the closure.

Schema v37 also inventories unsupported instructions explicitly. Every
function has a typed `decode_blockers` array containing the instruction PC,
width, raw encoding, extension class and whether the base ISA proves linear
continuation. The summary counts both blockers and affected functions. These
records are restricted to instructions reachable from the function entry by
the conservative CFG walk; padding and embedded bytes after a return are not
reported as blockers. F and CSR instructions may therefore preserve useful
later integer/MMIO evidence, but any reached blocker still makes the function
incomplete. Concrete verification uses the strict decoder and never executes
an unsupported instruction.

RV32F word loads and stores are a narrower exception at the structural layer:
the backend decodes `flw`/`fsw` (including compressed forms), tracks their
addresses through the ordinary integer base register and carries loaded bit
provenance through a separate floating-register state. This recovers MMIO,
stack and reviewed memory-object accesses without claiming floating-point
arithmetic semantics. The original F blocker remains, so this extra evidence
cannot make the function verification-eligible.

Bit-preserving RV32F moves (`fmv.w.x`, `fmv.x.w` and the `fsgnj*` family) also
carry exact provenance between integer and floating registers. This includes
the assembler aliases `fmv.s`, `fabs.s` and `fneg.s`. Call boundaries
invalidate caller-saved floating registers while retaining the ABI-defined
callee-saved set. Arithmetic, conversions and comparisons remain blockers.

An all-zero halfword is classified as `zero-fill-or-illegal-trap`, not as a
generic decoder failure. RISC-V makes that encoding illegal, while real
toolchains also place it in unreachable fill after branches and noreturn
calls. The artifact alone cannot prove which intent applies, so the workbench
preserves the exact site, stops that path and does not reject the surrounding
ELF function symbol.

## Bounded function workers

Artifact-wide roots can be analyzed with `ir build --jobs N` or
`ir export --jobs N`. The work unit is a function, not a whole archive: this
also distributes symbols from one large ROM ELF and avoids making a single
large object member the scheduling bottleneck. Roots are greedily balanced by
code size. Each worker produces only function-local decode, CFG, call, MMIO,
context and pseudo facts. The joined function set is then sorted by stable
identity before shared guard linking, SCC/fixed-point effect summaries and
register indexing.

The parallel path is used only when every named symbol is already a root.
Prefix-root `--include-reachable` analysis remains serial because discovered
callee roots change the pending set. `--jobs 0` uses up to four available
workers; explicit parallelism is bounded to eight workers. Serial and
parallel reports are byte-identical and concrete verification semantics are
unchanged.

A project inventory can aggregate several independently linked or relocatable
inputs in one report. Multiple inputs must have stable source names:

```console
cargo vendor-binary-workbench ir export \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --artifact libpp="$ESP32S31_LIBPP_ARCHIVE" \
  --pseudo-rust /tmp/vendor-project.pseudo.rs \
  --json-report /tmp/vendor-project.ir
```

Project identities are namespaced, for example `rom::ets_delay_us` and
`libphy::phy_init`. Semantic boundaries and all summary counts are aggregated
across sources. Each named primary is analyzed in its own address space;
schema v45 records `"linkage_mode": "independent-artifacts"` and does not claim
that separate inputs share an address space or were fully linked. Use one
linked ELF primary plus `--companion` inputs when cross-image addresses and
relocations belong to one executable address space.

When the command is resolved from a project with `[code]`, every primary input
also receives the accepted reviewed ranges authenticated for its source and
artifact SHA-256. Schema v37 records those ranges in each source artifact as
`reviewed_code_boundaries` (member, section, reviewed name and exact offsets),
and the summary records their count. A report therefore never hides whether a
function came from an ELF symbol or from explicit human boundary review.

Project mode does perform a conservative symbol-level call association. An
unresolved call relocation becomes `project-linked` only when exactly one
exported definition with that symbol exists across all inputs. Multiple weak or
global definitions remain ambiguous, and local definitions are never selected.
`project_call_linkage` records this policy. The edge is useful for navigation,
but arguments, return propagation and addresses are not substituted, so the
original reference blocker and incomplete function status remain intact.

When a project also has reviewed interface facts, linked IR can project a
reviewed archive interface call onto the authoritative linked ELF. This is a
separate, fail-closed join. It requires the symbol inventory's sole
`unique-name-and-kind` archive origin, a uniquely associated reviewed table
pointer cell, and the same decoded indirect target shape: container depth,
load offsets and widths, indexed selector, slot offset, call/tail shape, and
`jalr` offset. Instruction addresses are not required to retain the same
function-relative offset because linker relaxation can change instruction
widths and positions. Ambiguous contracts are left unresolved. Successful
calls carry `semantic_contract.source =
"archive-origin-interface-association"` and retain the exact evidence rule;
the semantic name remains descriptive and does not authorize an executable
call model.

An unresolved returning relocation or indirect `jalr ra` is also an opaque ABI
boundary for function-local structural analysis. The analyzer records an
explicit completeness blocker, invalidates caller-saved integer and floating
registers, treats private stack passed to the call as potentially modified and
continues at the return address. It does not invent a target, return value or
side effects. This preserves later accesses through callee-saved context
registers for manual review without making the trace reference-eligible.
Unknown tail jumps and other unresolved indirect control-flow shapes still
terminate the current path.

## Instruction effects

Schema v45 gives every function an `instruction_effects` array. It is the
canonical lossless join between structural semantics and the decoded body:
each directly observed MMIO or RAM access retains its originating instruction
`site`, conservative CFG `block`, access width and kind, typed target, path
inventory and recovered value/mask evidence. A compact structural MMIO poll is
attached to its load instruction rather than the back-edge branch. Bounded
memory intrinsics are attached to the originating call instruction.

These entries describe direct instruction origins only. Effects composed from
callees remain in `effect_summary` with their call-path provenance and are not
misattributed to the caller's call instruction. Function-level
`mmio_accesses` and `memory_accesses` remain path-grouped analysis shapes;
`instruction_effects` is the addressable evidence used by `inspect function`
and the TUI full-body view. Unsupported instructions and semantic blockers do
not remove either the raw instruction stream or effects recovered at other
sites. Top-level `instruction_effect_mode` records this boundary as
`direct-origin-sites-with-basic-blocks`.

## Calls, summaries, and return provenance

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
iteration counts. This compaction is part of the typed linked-IR model rather
than a renderer-specific JSON transformation.

Exploratory blocker messages can contain thousands of repeated exact clauses
when branch recovery reaches the same unsupported call or jump through many
symbolic states. Schema v39 records
`diagnostic_compaction_mode: "exact-semicolon-fragment-inventory"`. Each
function's channel-specific structured diagnostics keep the original fragment count, every
unique exact fragment, its number of occurrences and its first ordinal. The
diagnostic record also carries a classified `kind`, an optional instruction
`site`, and a stable `root_id` used by review queues. Pseudo-source uses the
compact `rendered` form with an explicit `[repeated N times]` suffix. The old
parallel string blocker arrays are not part of schema v45. This is mechanical report compaction, not
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

## Memory objects and reviewed type evidence

Schema v39 generalizes caller-context observations into `memory_accesses` and
`memory_fields`. A recovered affine address has one of these fail-closed roots:

- `argument`, identified by an ABI argument index;
- `global`, identified by the archive member containing a completed
  relocation and its symbol name. The member is relocation provenance, not a
  linker-definition claim; it is absent for a resolved linked image;
- `dereferenced`, identifying a pointer-bearing memory object plus the exact
  offset of the loaded 32-bit pointer cell;
- `absolute`, retaining the address space and address of a known RAM root;
- `indexed`, retaining a base object, selector argument and proven stride.

Schema v42 also inventories named static `data_objects` from every analyzed
ELF and relocatable archive member. A data object retains its source, member,
section, symbol, section-relative offset, optional linked address, size,
writability and uninterpreted initializer bytes. Relocations in the initializer
remain symbolic target/addend records: an archive initializer is not rebased as
if the workbench had reproduced the final link. Compiler-local symbols at the
same exact section offset (for example `.LANCHOR3`) are recorded as aliases, so
function memory accesses can be joined back to the human-facing ELF object.
If a referenced initialized section has only a zero-sized anchor and no named
object, the remaining section bytes are retained as
`synthetic_from_anchor = true`; this is how compiler switch tables remain
visible without inventing a source-level variable name.
Per-object xrefs summarize readers, writers, fixed offsets and recovered
argument/stride selectors. This is binary evidence and does not infer a C/Rust
type, array bound, ownership model or runtime initialization order.

Each access retains signed byte offset, width, read/write kind, path and value
evidence. A field aggregates equal `(object, offset, width)` accesses while
keeping counts and write masks. A resolved address whose value depends on a
known RAM pointer read but whose offset remains dynamic is reported as dynamic
RAM evidence, not promoted to a fixed field. Incomplete HI/LO relocations and
unknown roots remain blockers.

`context_accesses` and `context_fields` remain the argument-only projection
used for affine interprocedural composition. Reachable `effect_summary`
`memory_fields` contains those projected root arguments plus relocation-rooted
fields observed in the reachable closure. It is an origin-preserving
inventory, not a nominal-type or aliasing proof. Function-pack schema 4 is the
separate, reviewed layer that may bind several exact objects to one logical
type.

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

## Semantic actions, event dispatch, and guards

The same path walk emits `semantic_actions`: one record for every recovered
semantic call on every explored simple call path. Each action retains its
origin, static call site, target, typed argument values, replacement hint and
any affine projection back to root arguments. It also carries the exact
contract source, stable ID and evidence rule that justified the semantic name.
The `site_path` array records the lexical call-site chain from the report root
to the action, and actions are stably ordered by that chain.

Schema v39 projects reviewed event-like contracts into
`event_dispatches`. This is a higher-level navigation view over
`semantic_actions`, not a second source of effects. Each record has a
zero-based `semantic_action_index`, a reviewed mechanism and execution context,
and typed arguments assigned to stable roles such as `channel`, `selector`,
`payload`, `payload-size`, `wait` and `wake-output`. These fields are declared
next to the typed semantic ABI by the selected platform harness. The generic
linked analyzer contains no operation-name dispatch table, so a new reviewed
contract can opt in without teaching it vendor or RTOS vocabulary. An operation
name alone, a raw function name or an argument value never creates an event.

`interface_complete` means only that the reviewed contract is present and its
expected named arguments were projected without missing, duplicate or
unexpected fields. It is not a scheduler, memory-effect or delivery proof;
the top-level `event_dispatch_effect_completeness_claim` is therefore false.
The receiving task or callback is not inferred from the sending call:
`event_dispatch_receiver_inference_mode` is `"none"`. `receiver` is populated
only when the reviewed contract explicitly names it and otherwise remains
`null`; `event_dispatch_receiver_source_mode` records this
`"reviewed-contract-or-unknown"` policy. The referenced semantic action remains
authoritative for origin, lexical site path, full call path and factorized CFG
guards. Human and pseudo views render the same relationship, with one-based
action labels only in pseudo-source.

Direct call records additionally expose recovered `cfg_guard_paths` in
disjunctive normal form: paths are alternatives and the decisions inside one
path are conjunctive. During semantic projection these are retained as
`cfg_guard_scopes`, with an AND between function scopes and an OR between the
paths inside one scope. Keeping the formula factorized avoids an artificial
cross product across nested calls and preserves the function in which each
decision was made. Registered external-table calls preserve their original
instruction `site` as well, so reviewed interface evidence can join to these
guards by exact caller and address. Calls synthesized through composition
remain distinguishable from site-bearing direct evidence. Complementary
alternatives and absorbed supersets are
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
the join contract explicit. When selected producer bits map to a concrete MMIO
read, either directly or through exact internal `call-result` return ranges,
each result source also contains `mmio_sources`. Projection is per bit, so
shifted and inverted wrapper returns preserve the intersection between the
tested result mask and the leaf register mapping. Each source records both
result-bit and register-bit masks, comparison values in both coordinate
systems, address, SVD name, composed inversion, `producer_path` and derived
`return_depth`. Traversal follows only resolved identities present in the
selected report and rejects a recursive `(function, output bit)` revisit.
Arguments, unknown arithmetic, external results and unresolved targets stop
projection; an absent mapping stays an empty array.
The top-level `cfg_guard_mmio_linkage_mode` value
`"recursive-exact-bit-projection-with-producer-paths"` identifies that
contract. The summary separates all `guard_mmio_links` from those whose
producer path crosses at least one returned internal call as
`transitive_guard_mmio_links`.

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

## Scenario suggestions

Schema v34 adds `functions[].scenario_suggestions`. This is an advisory bridge
from structural evidence to concrete execution profiles, not a new verifier.
The current bounded rules emit paired candidates for direct argument
equality/inequality branches, equal/not-equal MMIO predicates, and polling
registers (`ready-immediately` plus `one-retry-then-ready`). Each variant keeps
typed ABI argument assignments or an ordered MMIO read sequence together with
the originating condition, site, mask, and expected value.

Suggestions intentionally cover only shapes whose concrete candidate values
can be derived without solving arbitrary machine arithmetic. Their absence is
not evidence that no scenario is needed, and their presence is not path
feasibility or coverage proof. The top-level
`scenario_suggestion_mode` is
`"structural-candidates-require-concrete-replay"` and
`scenario_suggestion_proof_claim` is `false`: a reviewer must select or edit a
candidate, and only a successful fail-closed executor replay contributes to
MATCH/DIFF/INCOMPLETE and coverage.

## Pseudo-Rust view

The pseudo-Rust intentionally uses `u32` argument placeholders and is not
compilable output. It renders recovered MMIO/RAM effects, delays, polls,
branches, internal calls, diagnostic calls, scratch buffers, and named
external ABI calls. External call records include the table version, slot,
argument count and reviewed return model. Unsupported instructions and
incomplete control flow remain adjacent `DIRECT-BLOCKER` or
`REFERENCE-BLOCKER` comments instead of being guessed.

## MMIO register and field index

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
it. This write-pattern inventory remains available alongside the richer field
candidate evidence.

Schema v39 exposes `field_candidates`. It merges equal contiguous subregister
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
comparison operator instead of hiding path polarity. Predicate and semantic
evidence retain the complete MMIO `producer_path`; `access_functions` names
the leaf that actually reads the register while `functions` also inventories
the intervening wrappers. `semantic_evidence`
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
recoverable per-bit provenance; producer predicates follow the exact direct or
return-chain linkage described above. Neither path guesses through unknown
arithmetic or unresolved calls. No access policy such as W1C, reset value,
field name or peripheral semantics is inferred. The JSON records this scope
with `direct_mmio_predicate_mode`,
`direct_mmio_predicate_completeness_claim: false`,
`mmio_field_candidate_mode` and `mmio_field_semantics_claim: false`.

Schema-v45 linked-IR bundles can be supplied to `registers review` as optional
enrichment. The register workspace merges their field candidates, predicate
details and semantic navigation links with artifact-wide MMIO facts while
keeping all generated evidence outside the reviewed model and release SVD/PAC.
See [register workspace](register-workspace.md#linked-ir-enrichment).

Repeated exports can be declared once as project-owned profiles and generated
or checked together with `ir build`. See
[project linked-IR builds](project-ir-build.md).

## External ABI and trampolines

The lower-level symbol and definition facts are described in
[artifact and symbol inventory](symbol-inventory.md). In particular, a project
symbol candidate is not enough to activate the semantic behavior below.

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

A directly relocated platform function may additionally carry an executable
call model. This is a stronger contract than a semantic annotation: only a
reviewed `void`, `constant`, `symbolic-u32` or `symbolic-u64` return model lets
structural execution cross an otherwise unresolved direct-call boundary. `void` means the
ordered call is the modeled observable effect and no ABI result exists; it must
not be represented by a dummy constant. `symbolic-u64` preserves independent
RV32 `a0` and `a1` words. Reviewed private-stack byte outputs remain independent
from the return value and carry their call token and output ordinal through
data-flow and generated reference code. A `modeled-direct-external` call
retains its operation, evidence ID and replacement hint. ESP32-S31 uses a constant model for
the fixed 40 MHz `rtc_clk_xtal_freq_get` platform input. An annotation with
`unmodeled` return/effects remains fail-closed.

Reviewed trampoline calls additionally persist the complete executable model
in linked-IR schema v45: model ID, return model, and each output kind, pointer
argument and width. This keeps navigation and later review honest without
teaching the generic schema RTOS-specific meanings.

An executable `allocated-zeroed` return model is stronger than an opaque
pointer result. Structural analysis assigns each call a stable
`zeroed-allocation:<call-token>` memory-object root and retains affine field
offsets through subsequent loads, stores and calls. Concrete execution takes a
scenario-owned arena, reads the requested size from the reviewed ABI argument,
checks alignment, capacity and overlap, initializes exactly that prefix to
zero, and records the allocation as CPU-owned environment evidence. The model
never guesses a host heap address. Static analysis currently uses the zeroed
marker as provenance and ownership evidence; it remains conservative about a
byte's value until concrete execution or an explicit write establishes it.
Deallocation and use-after-free validation are a separate lifetime layer.

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

## Context views and completeness

Pointer-relative RAM is rendered as an inferred context view. For example, an
access rooted at ABI argument 0 becomes `ctx0.read16(+0x8)` or
`ctx0.write32(+0x4, value)`. Each function also receives `context_fields` and
`context_accesses` JSON records. A field groups `(argument, byte offset,
width)`, counts reads/writes and unions the observed write mask; individual
accesses preserve their branch/call path, symbolic value and, for recognized
read-modify-write values, preserved/forced-zero/forced-one masks. These are
layout and data-flow facts only: the tool does not infer a C type name, field
name, ownership, validity invariant or concurrency semantics.

A project can attach those human-reviewed presentation claims without changing
the generated IR through a [function and context pack](function-packs.md). The
derived function report keeps closure blockers, semantic links and pseudo-code
beside the reviewed names; the pack does not make the IR complete.

The JSON is the machine-readable linked view: each function contains artifact
identity, `global-or-weak` or `local` binding, address or relocatable object
offset, flow quality, dependencies, direct call edges, symbolic arguments,
blockers and the same pseudo body. `ir export` deliberately loads sized local
text symbols as well as exported global/weak functions, while validation and
verification commands retain their narrower exported-symbol inventory. A
local call resolved through an archive `R_RISCV_CALL` relocation is therefore
linked to its private callee. Repeated local names in a linked ELF are given an
`@0x...` address suffix so identities and call targets remain deterministic.

This is still symbol-guided recovery, not a proof that every executable byte
has a function boundary: stripped functions, zero-sized labels, hand-written
code without `STT_FUNC` metadata, jump tables, and undiscovered indirect calls
can be absent. As with MMIO discovery, the report declares
`"completeness_claim": false`.
