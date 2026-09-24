# Function, library and PHY analysis

Analyze captured code and inspect coverage, symbolic values and explicit gaps.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Function analysis contract

Function analysis selects one imported occurrence by frozen revision, input ordinal
and full static `SymbolId`. The selected RV32 ET_REL or static ET_EXEC symbol
must define code in an executable section. ET_REL addresses are section-relative;
ET_EXEC addresses belong to the selected image. A nonzero symbol size
supplies its extent; an explicit extent starts at the symbol and is recorded as
user-supplied even when a size exists. Zero-sized symbols require an explicit
extent (`needs-extent`); no neighboring-symbol heuristic is used.

The recipe identifies source bytes, extent and its authority, analysis policy and
decoder implementation. Analysis uses a bounded iterative worklist rooted at the
entry. Calls do not expand into callees; continuations are possible control flow,
not proof that a call returns. Unknown instructions, indirect transfers and
uninterpreted relocations preserve separate decoding/control-flow/reference gaps.
Unvisited bytes are reported without claiming they are instructions or dead code.
Semantic incompleteness can be retained; resource failure/cancellation cannot
publish a partially written result. There is no execution or equivalence claim.

Domain owns values and the ISA port; artifacts lends captured code and structural
ELF facts; `blobray-backend-riscv` interprets instructions/relocations;
`blobray-analysis` builds local graphs and coverage. Application owns selection,
supervision and publication, store owns verified closures and atomic metadata,
and the host injects the backend and renders results. No analysis/backend module
opens projects or origin paths. Graph allocations require working-capacity
admission and emitted records use the existing temporary disk quota. Result reads
never rerun analysis. Prepared-image results retain their `PreparedImageId`;
imported executable results retain their input occurrence and payload identity.

### Analyze and reopen a function

`analyze-function --request request.json` accepts `FunctionRequest` with `revision`
(ID or null to freeze selection at admission), `source`, `selector`, optional
`extent` and optional `research`. Source is `{"kind":"input","input":0}` for
captured inputs or `{"kind":"image","image":"IMAGE_ID"}` for a prepared image.
Image selection freezes the image's original revision independently of current;
its object identity addresses the retained ELF.

- `selector: {"kind":"symbol","symbol": SYMBOL_ID}` selects an exact physical
  static/dynamic table occurrence. Optional `extent: {"start": 0, "length": 64}`
  overrides its declared size, starts at that symbol and is mandatory for zero
  size. Names enumerate candidates, including undefined references; they never
  choose an occurrence implicitly.
- `selector: {"kind":"range","object": OBJECT_ID,"section": 1,"extent":
  {"start": 8,"length": 16}}` selects explicit code bytes without a symbol.
  The outer `extent` must be null or absent. No symbol is fabricated and an ELF
  without either symbol table is valid for this selection.

Extents use section offsets for ET_REL and virtual addresses for ET_EXEC. They
must be nonempty, start at a halfword boundary and stay in the selected
file-backed executable section. Captured archive/member identity, ELF section
and exact extent survive analysis and export. Unselected executable bytes do
not acquire boundaries or semantic coverage.

```console
cargo blobray analyze-function --project /path/to/investigation \
  --request /path/to/function.json --limit-mode watchdog --format json
cargo blobray analyses --project /path/to/investigation \
  --limit-mode watchdog --format json
cargo blobray analysis --project /path/to/investigation \
  --id <analysis-id> --limit-mode watchdog --format json
cargo blobray export-analysis --project /path/to/investigation \
  --id <analysis-id> --output /path/to/new-directory --limit-mode watchdog
```

The API uses `Application::start_analyze_function`, `FunctionRequest`,
`ReadQuery::{Analyses,Analysis}`, borrowed `QuerySink::function_record` callbacks
and `QueryOutput::export_analysis`. `FunctionAnalysisId` identifies the exact
manifest, including recipe, coverage/counts and the digest of `records.jsonl`.
An export contains those records and `manifest.json`, written last to a new
directory. Existing destinations are never overwritten; interrupted exports can
leave an incomplete directory. Reopening verifies retained digests and requires
neither original input paths nor a decoder invocation. The store retains all
source revisions; there is no pruning or automatic analysis cache lookup.

### Research a linked image

From the repository root, build `cargo build --manifest-path tools/blobray/Cargo.toml --profile blobray -p blobray-next`.
Use `tools/blobray/target/blobray/blobray` as `blobray` below. Linking requires the
explicit supported ELF linker executable; reading and analysis do not require a linker.

```console
blobray init --project research
blobray import --project research --input vendor=/absolute/path/to/library.a --limit-mode watchdog
blobray link-plan --project research --entry entry_function --entry-input 0 --inputs 0 --code-start 0x10000000 --data-start 0x20000000 --linker /usr/bin/ld.lld --output research/link-plan.json --limit-mode watchdog
blobray prepare-image --project research --plan research/link-plan.json --linker /usr/bin/ld.lld --limit-mode watchdog
blobray analyze-project --project research --image IMAGE_ID --limit-mode watchdog
blobray functions --project research --id PUBLICATION_ID --name entry_function --limit-mode watchdog
blobray analysis --project research --id ANALYSIS_ID --limit-mode watchdog
blobray calls --project research --id PUBLICATION_ID --caller 0x10000000 --limit-mode watchdog
blobray calls --project research --id PUBLICATION_ID --callee 0x10000100 --limit-mode watchdog
blobray find-accesses --project research --id PUBLICATION_ID --address 0x20000000 --limit-mode watchdog
blobray find-references --project research --id PUBLICATION_ID --address 0x10000100 --limit-mode watchdog
blobray image --project research --id IMAGE_ID --limit-mode watchdog
```

The virtual placements above are synthetic choices, not recovered hardware
addresses. `--inputs 0,1` supplies explicit archive order. Name selection requires
one defined static function in `--entry-input`; ambiguous or missing names return
candidates and a failing exit status. Exact selectors and additional roots remain
available through `link-plan --request`. Code/data regions each default to 16 MiB;
`--region-bytes` changes their size. The retained revision supplies the RV32 integer-analysis profile; the retained
link recipe supplies input order, layout and tool identity. Image manifests
(version 2) and function recipes record the actual ELF-declared ABI separately.
The selected linker checks ABI compatibility across selected inputs. Floating-point instruction
semantics remain unsupported in static research; concrete execution uses the separate
[execution contract](../execution/README.md#concrete-execution-and-comparison). Unsupported instructions stay gaps.
No legacy configuration reader participates.

For an existing static linked ELF, import it and run `analyze-project --project
research` without `--image`. With no `--plan`, the command creates a frozen plan
and executes it within one application operation, worker and original budget. Its publication
retains that plan. `plan-investigation --image IMAGE_ID --output study.json`
also produces an explicit reusable selection. Existing saved plans remain
immutable; callers choose them with `analyze-project --plan`.

`functions` lists every matching occurrence, its declared extent and analysis ID,
including blocked functions. `calls` reads saved calls and outgoing jump/tail
candidates; `--unresolved-only` keeps unknown targets. A resolved transfer does
not establish callee effects or prove return. Computed indirect destinations do
not expand the local CFG. Canonical return edges are calling-convention patterns;
a known outgoing target is retained separately as a transfer. Queries identify
the function source so equal numeric addresses in different images are distinct.
`find-references --address` includes resolved static relocations, image-address
formation and memory accesses. Unqualified integer constants are not treated as
pointers merely because their values match. `image` streams source mappings including their `exact` flag, then the synthetic
image manifest. Inexact mappings never become exact original instruction offsets.

The artifact view verifies nonoverlapping RV32 load segments and that selected
code agrees with executable file-backed bytes. Dynamic loading, runtime
relocations, TLS, RV32E/quad-float ABI and writable executable segments are outside
this static profile. Static relocation sections retained by `--emit-relocs` are
provenance; their edits are not applied again during value/flow interpretation.
Read-only ELF permissions are the declared static memory interpretation, not a
hardware-memory-map assertion. No reads of external MMIO or original source paths
occur during analysis. Whole-image buffers and segment tables share the operation
capacity; the borrowed `ImageMemory` view cannot outlive them.

### Structural decoding

Decoder policy 1 uses pinned `rv-asm 0.2.1` through an independently injected RV32 decoder.
It supports its RV32IMAC encodings and records unsupported encodings as gaps.
Printed instruction text is versioned with that decoder, not a parsing interface;
bytes, offsets and typed flow are the structural interface. Calls use ABI link
registers x1/x5; canonical indirect return patterns describe the calling convention,
not proof about the dynamic register value. Other indirect transfers remain
unresolved. A direct call records its target but does not enqueue the callee.

Records include instructions, maximal decoded basic blocks, instruction-origin
edges, relocation references and gap ranges. Block IDs are their start offsets;
edge `from` is the controlling instruction offset, not a guessed load address.
The function manifest supplies the common section identity. Direct fallthrough,
taken branches, jumps, calls, possible continuations, returns, stops and conflicts
are distinct. Unvisited bytes can coexist with complete reachable decoding;
gap records also explain explicit unsupported or conflicting boundaries.
ELF `$d`/`$x` mapping symbols prevent decoding declared data as instructions.

References retain relocation section/index/type/addend and original symbol
identity, raw name bytes, binding and definition kind. Interpreted references keep
separate target and HI/LO pairing evidence. CALL/CALL_PLT, branch/JAL/RVC branches,
absolute HI/LO, PC-relative HI/LO and R_RISCV_32 are recognized. HI/LO pairing uses
relocation identity, never adjacency or equal names. NONE/RELAX/ALIGN remain
metadata, including validated null-symbol references. REL implicit addends and
other relocation types remain uninterpreted rather than being filled with zero.
Named external references need no chosen implementation or final address.

Coverage has independent `decoding`, `control_flow` and `references` fields for
this local scope. A completed run can have assessment coverage `partial` and a retained
partial result; CLI returns that successful publication with exit 0. Failed
admission/execution returns nonzero. Missing extents are `needs-extent`; missing
captures, integrity failures, cancellation and exhausted limits publish no result.
Emergency memory/time/work/disk limits remain in run records, outside the semantic
recipe. The recipe includes extent authority, policy and decoder identity.

Memory admission includes one full ELF object, a 1 MiB operation envelope,
the concrete relocation/symbol structure sizes and retained name capacities,
32 bytes per ELF mapping symbol for mapping indexes, and 512 bytes per possible instruction
halfword for graph state. Value analysis additionally admits the actual type sizes
of 32 register values, operation metadata, a queue index and membership flag per
decoded instruction. These conservative reservations bound retained buffers;
host allocator overhead and decoder/library internals remain under process
containment. Names are capped at 4096 bytes and serialized records/manifests at
64 KiB. Graph queues are marked before insertion and cannot grow through cycles.
Records are streamed to quota-admitted staging files. Publication of manifest,
record closure and completed run uses one transaction and does not change current
revision. Recovery never promotes loose function output to success.

The standalone workspace and crate-boundary tests include these components and
prohibit dependencies on the legacy backend. Format support is defined once in
[JSON and checks](../interfaces-formats/README.md#json-and-checks).

### Values and memory effects

Function recipe/manifest version 7 records the semantic producer, typed source,
address space, register values, memory accesses, transfers and semantic gaps.
Older function schemas are unsupported; reading never converts or recomputes
a result. Captured inputs are unchanged. Function manifests are independent
of storage metadata version. Function selection policy 8 validates physical static/dynamic tables.
Investigation recipes use version 3 / policy 4 and enumerate both tables plus
explicit ranges; older
selection policies are unsupported.

The domain `FunctionSemantics` port extends decoding with typed operations and
relocation roles. The RISC-V backend lifts decoded instructions, never display
text. Analysis owns an iterative fixed-point computation over the existing CFG;
application owns admission and publication, and store retains opaque records.
The computation borrows captured bytes, an ISA port, working capacity, control
and a sink. It cannot obtain filesystem, project or hardware-description access.

Register values are unknown, RV32 constants, image virtual addresses, section-relative addresses, exact
symbol references with addends, offsets from the entry stack pointer, or flat expression IDs. Entry
`x0` is zero, `sp` denotes its entry value, and other registers have symbolic entry values.
Stack-relative values express provenance, not allocated or accessible memory.
Exact incoming values join into canonical sets of at most eight alternatives.
The leaves are constants, image/section addresses, physical symbols and entry-stack
offsets; sets cannot contain other sets, expressions or unknown leaves. Arithmetic
uses bounded Cartesian products and immutable loads read every candidate. These
are may-values: branch correlation is not retained and membership does not prove
that a runtime path selects that value. A ninth distinct result widens to unknown
and emits `alternative-limit`, making semantic completeness false. Unequal
symbolic expressions and incomplete relocation uppers still join to unknown.

Alternative storage and its lookup index belong to the analysis phase and consume
admitted memory and work; failures publish no analysis. JSON uses
`{"kind":"alternatives","values":[{"kind":"image-address","address":4096},{"kind":"image-address","address":8192}]}`;
human output uses `one-of{... | ...}`. Saved access/reference/call filters match
membership and preserve the complete set. `calls --unresolved-only` includes
multiple-target transfers. Research never selects one of these callees or composes
it as a definite call. Expressions and callee effects retain alternative operands;
imported image addresses remain qualified by their callee source/object.
Branches are not pruned and computed indirect destinations do not extend the CFG.

Loads retain address and width. In the static ELF image profile, file-backed
bytes in readable, non-writable PT_LOAD segments supply constants, with signed
byte/halfword extension. Writable memory, zero-fill tails, unmapped addresses
remain symbolic loads; atomic load results remain unknown. Stores retain address, width and source value. Atomic operations retain their read/write kind and
conditional store behavior without modeling memory or reservations. Calls are
opaque effects by default: their possible continuation forgets registers except x0 and symbolic x10/x11 call results. `research --abi-contract riscv-integer` explicitly preserves integer callee-saved registers, sp, gp and tp across calls; ELF flags do not select this assumption. Unknown operations and conflicting instruction
boundaries are explicit gaps and cannot propagate stale values. Missing decoded
regions remain structural gaps. Semantic coverage is separate from value
precision: an unknown input does not make a modeled instruction unsupported.

HI/LO address formation requires compatible relocation identities and an actual
flowing upper value. PC-relative pairs use their recorded label relationship;
a partial pair never becomes a complete address. RV32 integers wrap according to
the ISA; symbolic address arithmetic keeps provenance only for representable
transformations. Reviewed MMIO naming and bounded call composition belong to the explicit research operation below. Mutable memory forwarding, table models and machine execution remain outside this profile.

The solver reserves input-dependent state and queue capacity before allocation,
keeps at most one queued item per node, and charges every transfer to the shared
work/deadline budget. Fixed-point records are emitted once in offset order.
Resource exhaustion or cancellation cannot publish a result. Working-capacity
admission is not a claim that the process performs no system allocations.

During `analyze-values`, progress `table` is the code section index and `entry` is
the current instruction's section offset or image virtual address. The integer
semantics follow the
[RISC-V integer ISA](https://docs.riscv.org/reference/isa/unpriv/unpriv-index.html)
and [RISC-V ELF relocation ABI](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/).
Known-address counts include symbolic and stack-relative expressions; they do
not imply a final load address, valid mapping, alignment or successful access.

Address expressions denote RV32 base-plus-offset arithmetic modulo 2^32; their
signed metadata offsets use checked i64 arithmetic. An overflowing metadata
expression becomes unknown. Both linked calls and relocation-identified tail
calls are opaque effects at the transfer instruction; their AUIPC upper alone
is not a resolved address or an unknown relocation diagnostic.

## PHY/ROM research

`research` selects exactly one function from a retained publication and saves a
new function analysis. It uses the same decoder, local CFG/value engine, staging
and supervisor as `analyze-function`. Selection, computation and retention share
one application run and the original deadline, work, memory and disk budgets. Missing or ambiguous names fail with
no preferred candidate; `functions` lists candidates and exact addresses.

```console
blobray research --project research --id PUBLICATION_ID --name phy_force_dig_gain --abi-contract riscv-integer --limit-mode watchdog
blobray analysis --project research --id ANALYSIS_ID --limit-mode watchdog
blobray knowledge --project research --limit-mode watchdog propose-register --analysis ANALYSIS_ID --subject phy.force-dig-gain --name FORCE_DIG_GAIN --address 0x20100408 --width 4 --field ENABLE:16:1 --field GAIN_0:0:8 --field GAIN_1:8:8 --actor researcher --reason "Reviewed register evidence"
blobray knowledge --project research --limit-mode watchdog show
blobray knowledge --project research --limit-mode watchdog accept --base PROPOSAL_REVISION --assertion ASSERTION_ID --actor researcher --reason "Accepted interpretation"
blobray research --project research --id PUBLICATION_ID --name phy_force_dig_gain --abi-contract riscv-integer --knowledge ACCEPTED_REVISION --limit-mode watchdog
```

The register example is scoped to the exact selected function's source/object
and revision. Its address and fields must be reviewed for the supplied artifact;
the command does not infer hardware meaning from a write. `--base` is required
for proposing into an existing knowledge history. Proposal and acceptance are
separate explicit operations; `accept` cannot silently supersede a conflict.
Advanced `knowledge apply` accepts the same native typed claims.

`mmio-register` has `register: {name,address,width,fields}`; width is in bytes and
must be 1, 2 or 4. Fields have `{name,lsb,width}` in bits; names are bounded, fields
must fit and cannot overlap. `mmio-region` has `region: {name,range:{start,length}}`.
Overlapping contradictory accepted interpretations in the same source/object
scope conflict. A selected knowledge revision annotates matching local accesses;
read values remain symbolic. The frozen knowledge ID is in the research recipe.
Knowledge about an image cannot silently become knowledge about an archive member.

The human analysis is a flat pseudo-code listing: `vN` definitions, typed integer
operations, symbolic loads, branch conditions, returns, calls and memory writes.
The JSON records are the same representation, not a separate decompiler.
Expression nodes carry instruction offsets and, for imported callee expressions,
the original analysis ID. Imported image addresses retain their source/object;
callee stack addresses are not equated with the caller's stack. CFG conditions
are structural alternatives, not path-feasibility or termination proofs.

Call composition requires the explicit integer ABI assumption. It substitutes
entry-register values and return expressions through uniquely resolved calls.
`callee-effect` records are **may-effects**: callsite, original analysis/instruction,
address, width and value. They do not establish unconditional execution or a total
cross-call effect order. Local counts in the semantic summary exclude these
transitive records. Unknown/partial callees cannot supply a proven return value
or remove an opaque-call gap. Inspect their original analyses for local CFG
conditions. Tail candidates and computed local branches are not expanded.

Application walks a maximum of 1024 reachable functions iteratively. Acyclic
callees are composed before callers. Recursive components and dependent summaries
retain local facts and an explicit reason; there is no recursive analyzer call.
Working capacity, temporary disk, deadline and work counters cover the whole
operation. Expression buffers grow in admitted chunks; composition retains its
reservation until its records have been staged. Exhaustion aborts publication.

### Explicit ROM companions

Capture the archive and ROM in one revision, in that order. Analyze that revision
to obtain a ROM/source publication. Synthetic linking does not treat ET_EXEC as
a relocatable input. Instead select exact ROM functions as external definitions:

```console
blobray link-plan --project research --entry phy_set_ftm_en --entry-input 0 --inputs 0 --companion 1:ets_delay_us --companion 1:phy_wait_i2c_sdm_stable --code-start 0x10000000 --data-start 0x20000000 --linker /usr/bin/ld.lld --output ftm.json --limit-mode watchdog
blobray prepare-image --project research --plan ftm.json --linker /usr/bin/ld.lld --limit-mode watchdog
blobray analyze-project --project research --image IMAGE_ID --limit-mode watchdog
blobray research --project research --id IMAGE_PUBLICATION --name phy_set_ftm_en --abi-contract riscv-integer --companion-publication SOURCE_PUBLICATION --limit-mode watchdog
```

`LinkRequest.companions` and `LinkRecipe.companions` contain exact input/symbol
selectors. Named CLI selection must be unique. At most 64 definitions are allowed;
undefined/nonfunction symbols, duplicate names, collisions with selected input
definitions and addresses inside synthetic placements fail. Definitions are
validated against captured executable bytes; no stub implementation is emitted.
This address-only acquisition checks the exact physical symbol table, declared
nonzero function extent, allocated code section and unique executable file-backed
load mapping. Non-executable data sharing the virtual address, TLS/dynamic
metadata and executable mappings elsewhere in the carrier do not become a runtime
dependency. A definition neither loads that
carrier nor proves its code can execute under the static-image profile; selecting
it as an execution companion still applies all normal loader restrictions.
Code placement uses ELF allocation/execution flags, including vendor sections
whose names are not `.text`. Address assignments and selected occurrences enter
the link identity. Other unresolved symbols remain linker failures.

`FunctionRequest.research` and `FunctionRecipe.research` contain `publication`,
`companions`, optional `abi` (`riscv-integer`) and optional `knowledge`. Companion
publications must belong to the same revision. A cross-image callee is eligible
only through an exact companion selection in the prepared-image recipe. Equal
numeric addresses in unrelated sources do not create a call association.
Definitions do not map ROM into writable memory or authorize execution. The
existing memory profile supplies constants only from each function's own
validated immutable ELF view. Missing providers, FP semantics, mutable alias
analysis remains limited; concrete comparison uses the separate
[execution contract](../execution/README.md#concrete-execution-and-comparison).

## Library investigations

An investigation freezes a revision, selected input ordinals (all by default),
explicit per-function extents and the selected function producer. Its saved plan
binds the digest and counts of deterministic streamed inventory entries. Planning
is read-only; execution revalidates the same selection before publication.
Defined static/dynamic function symbols and explicit code ranges remain separate
occurrences, including aliases.
Unsupported objects, absent thin members, unknown code coverage and missing
extents remain visible. No neighboring-symbol extent or name-based binding is
invented. Inventory-only data objects are recorded without inventing functions.

One supervised worker analyzes functions sequentially through the same function
operation, sharing working capacity, work, deadline and disk budgets. Per-function
state is released before the next function. Semantic blockers can produce a
partial investigation; resource, cancellation, integrity and I/O failures abort
publication. No subprocess-per-function coordinator or interprocedural engine is
introduced. Names of external references never establish linker selection.

Store owns an immutable publication manifest and streamed membership, retains the
function closures, and commits their visibility and the completed run together.
The current-publication pointer advances only when the admitted source revision
is still current. Status reports stale publications after later imports; opening
a saved publication verifies bytes and never recomputes them. These publications
are research scope and coverage, not review acceptance or verification verdicts.


### Library workflow and query contract

```console
blobray plan-investigation --project research --output library-plan.json
blobray analyze-project --project research --plan library-plan.json
blobray status --project research
blobray investigations --project research
blobray investigation --project research --id PUBLICATION
blobray find-accesses --project research --id PUBLICATION --address 0x60000124
blobray find-accesses --project research --id PUBLICATION --unknown-only
blobray find-references --project research --id PUBLICATION --symbol symbol-id.json
```

These commands accept the same resource options as function analysis. Choose
`--limit-mode watchdog` explicitly on hosts without delegated cgroup enforcement.
`--format json` returns streamed records and a summary. `plan-investigation`
requires a new output path; optional `--request request.json` accepts:

```json
{"revision":null,"inputs":null,"extents":[]}
```

Admission resolves a null revision once. `inputs` is either null (all inputs) or
a nonempty set of distinct ordinals; enumeration preserves inventory order.
Each extent override is `{ "source": {"kind":"input","input":0}, "symbol": SYMBOL_ID, "extent":
{ "start": 0, "length": 32 } }`. An unused or repeated override is rejected;
an invalid selected extent is a blocked function outcome. The saved recipe
includes decoder and semantic producer identities, but excludes runtime budgets
and local paths. Changing the producer requires a new plan. Inspection `plan` /
`run` commands remain a separate read-only operation.

Enumeration selects static and dynamic `STT_FUNC` symbols defined in nonempty executable
sections. Aliases and occurrences in different tables remain separate selections
even when their names, addresses and bytes match. Every input and object is
accounted for, including data-only objects.
An executable object with neither selected symbols nor explicit ranges is a gap;
no function boundaries are inferred from disassembly. `InvestigationRequest.ranges`
adds `{ "source": SOURCE, "object": OBJECT_ID, "section": 1, "extent":
{"start":8,"length":16} }` selections. Duplicate or unused selections fail
planning; invalid selected extents become blocked outcomes. A reviewed
`KnowledgeClaim::ExecutableRange` uses `section` and `extent` with an occurrence
whose `symbol` is null. Generic proposal/review checks executable backing and
physical identity. Accepted references in `reviewed_extents` supply ranges for
input or prepared-image research. Different overlapping boundary proposals in
the same section conflict; review does not assert semantic completeness.
Symbol and explicit-range aliases remain separate results; coverage counts their
byte union once. Complete coverage means complete outcomes for this
selected symbol scope, not proof that every executable byte has a function or
that the library has been verified. `analyzed` includes structurally or
semantically partial results; `complete_functions` requires both coverages.
A publication can be current and partial. `current` compares source revisions;
the publication's plan still defines which inputs were selected.

Without filters, access/reference queries include unknown addresses and unresolved
references. `--address` accepts an RV32 decimal or `0x` constant; `--symbol` reads
an exact `SymbolId` JSON file. These queries never resolve an external name to an
implementation or read memory contents. Findings include publication, exact
function request, analysis ID and original instruction offset or relocation site.
No match in a partial publication is not proof of absence. Read queries verify
saved closures under their own budgets and never schedule analysis.

### Library ownership and failure boundaries

| Owner | Responsibility and lifetime |
| --- | --- |
| Domain | Revision-qualified requests, plan/publication identities, membership, coverage and findings |
| Application investigation operation | Stream and validate selection, cache one captured container/object, call the shared function engine sequentially, stage membership |
| Function engine / analysis / RISC-V port | Same local algorithm and recipe as `analyze-function`; function-local reservations end before the next function |
| Store | Verify membership digest and child recipes, retain closures, atomically publish child rows, publication, current pointer and completed run |
| Host / CLI | One contained worker, signals, resource policy and presentation |

The plan remains bounded by the 64 KiB control protocol. Selection and membership
are JSONL streams on quota-owned temporary storage; they do not become a resident
library-sized queue. Enumeration admits a fixed 1 MiB envelope and at most 16,384
executable section indices for the current object. Exhausting that capacity
aborts with a resource error. Function parsing and solver state use the common
working-capacity authority. The supervisor admits a 2 MiB publication envelope;
child metadata is verified one record at a time. SQLite/host allocations remain
within the documented process-level boundary, not a no-allocation claim.

A worker stages all outputs before returning a compact receipt. After reaping it,
the supervisor verifies the saved plan, canonical entry digest, coverage counts
and every child recipe/record closure. The metadata transaction streams child
inserts under the remaining work/deadline budget; failure rolls the whole
transaction back. Cancellation linearizes before commit admission. Crashes can
leave unreachable CAS files, but cannot expose a half-published investigation.
Explicit recovery abandons interrupted runs and removes owned staging; it never
promotes an unfinished result. A later import leaves the last current publication
readable and visibly stale. Publishing an older plan does not replace a newer
revision's current publication.

## Final-image target audit

```console
cargo blobray audit-targets --artifact firmware.elf --forbid radio=0x2f800bf0..0x2f8016bc --limit-mode watchdog --format json
```

`audit-targets` is an ephemeral supervised operation with no project writer. It
reads one regular RV32 static ELF into admitted memory, detects observed capture
changes and identifies the inspected bytes by SHA-256. The artifact owner visits
every executable section, including code without function symbols. A section-less
ELF cannot produce a vacuous pass. The shared artifact mapping parser validates
local, zero-sized `$d`/`$x` symbols (including `$xrv32…` ISA-qualified code),
rejects conflicting/out-of-section mappings and non-RV32 code markers,
and separates embedded data from instructions. Ordinary labels never authorize
skipping bytes. Every section must match its executable file-backed load range.
The common RV32 decoder/lifter and integer
constant evaluator provide the analysis; no legacy backend participates.

The linear scan records direct branch/jump targets and locally resolved JALR
targets, resetting values at data intervals, control transfers and unknown instructions. Unresolved
indirect transfers are counted separately: a clean result only answers the declared
statically resolved target policy, not absence of all dynamic calls. Unknown major
opcodes that might conceal a transfer, unsupported instruction lengths and missing
coverage make the result unclean. Recognized non-control opcode classes can be
skipped with a counter and a full value reset. CSR accesses are non-control; trap
returns count as unresolved indirect transfers. This classification does not add
CSR or privileged execution semantics. Truncation is an integrity error.

Each finding retains section, site, target and selected forbidden range. The summary
records decoder/semantics identity, captured digest, policy ranges and coverage.
`executable_bytes` counts the whole executable section; `embedded_data_bytes`
separately counts bytes excluded by validated mapping symbols. Coverage-gap
records retain the site, reason and at most four encoding bytes.
Findings stream through the normal query spool. Limits, timeout, cancellation and
output delivery follow the same supervisor as other reads. A policy violation or
coverage gap exits nonzero; resource/integrity failures publish no success output.
Ranges are half-open and the request accepts 1–64 named ranges.
