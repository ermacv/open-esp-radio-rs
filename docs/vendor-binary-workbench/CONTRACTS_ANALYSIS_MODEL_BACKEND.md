# Contracts, analysis model, and backend responsibilities

## Contracts

- opaque external callback-table and function references;
- harness-owned external semantic overlays with opaque operation IDs, C types,
  argument directions and replacement hints;
- optional harness-owned event-dispatch projections with opaque mechanism and
  execution-context names plus roles bound to reviewed semantic arguments;
- architecture-neutral direct-function semantic contracts whose platform hook
  may accept a function only from its complete structural definition;
- immutable ABI table descriptions and return models;
- entry lifecycle, pointer-cell and function-table contracts;
- platform-independent contract lookup by caller-supplied string identity.

The contracts crate is deliberately dependency-free. Its identifiers are opaque strings
supplied by configuration. It must not have enums such as
`Esp32s31Eco0Rom`, `Esp32s31WifiOsiV9` or `Esp32s31Channel`.

An external slot may be ABI- and semantics-known while still using the
`Unmodeled` effect model. The backend preserves its call and opaque result in
exploratory IR, but emits a reference blocker. This separation is important:
adding a friendly `rtos.event.post` or `nvs.blob.read` label must never make an
effect-equivalence proof accept scheduler, storage or pointer effects that it
does not model.

Linked IR aggregates those labels into a report-level semantic-boundary index
containing callers, ABI targets and replacement hints. This index is a
migration inventory for manual analysis; it does not weaken the per-function
completeness or reference-eligibility checks.

Direct internal functions may receive the same typed semantic overlay through
a separate harness hook. The current ESP32-S31 contract recognizes
`pp.o:pp_post` only when its exact body and full relocation schema match the
reviewed definition, and labels it `wifi.internal-signal.post`. The hook is
architecture-neutral at the contracts boundary; byte and relocation matching stays
with the platform/backend integration. A near match remains an ordinary
internal edge, and the function body is still analyzed rather than replaced by
the label.

The same proven external calls form a structured trampoline inventory keyed by
the complete registered table/slot contract: pointer and backing symbols,
table version, magic, size, slot, reviewed C signature, return model and
semantic overlay. A backend event reaches this inventory only after exact
pointer-cell provenance and exact slot resolution; the linked layer does not
reinterpret arbitrary indirect calls. Per-root effect summaries retain each
reachable trampoline call and compose affine pointer arguments into root
context coordinates. Unresolved or dynamic pointer provenance is exposed as a
projection status/blocker rather than filled in heuristically, and recognizing
the ABI does not turn an `Unmodeled` external effect into a validation-grade
model.

Project export may aggregate several named primary artifacts. Function IDs are
source-namespaced and report summaries span all inputs, but each primary keeps
an independent address space. The machine-readable `linkage_mode` makes that
boundary explicit. A linked ELF with companions remains the mode for genuine
cross-image address and relocation resolution; resolving undefined symbols
across independent static archives requires a later project linker layer.

Function selection is separate from call resolution. A symbol prefix selects
explicit roots; an opt-in reachable mode follows only recovered internal edges
to other code definitions in the same primary resolver. Selected functions
retain root-versus-reachable provenance in every report view. Closure discovery
uses the same bounded exploratory call graph, so an unresolved edge or exhausted
state limit remains a blocker rather than causing a guessed callee to enter the
report. Companion definitions and unique cross-primary project associations are
not closure-selection authority.

As a narrower navigation aid, project IR associates an unresolved call
relocation with a callee only when one exported definition exists across all
named inputs. It ignores local definitions and leaves duplicate global/weak
definitions ambiguous. This produces a `project-linked` call-graph edge but
does not substitute arguments, return values or addresses; the caller therefore
retains its reference blocker and incomplete status.

Exploratory branch/loop recovery can observe many symbolic argument forms at
one static call site. Linked IR compacts them under the stable call identity and
retains a distinct argument-shape count. Values that disagree become an
explicit varying marker. Affine bindings are intersected across all shapes, so
only universally proven caller-to-callee context relationships reach effect
projection; disagreement removes the binding instead of selecting one path's
value. These shape counts describe recovered IR alternatives, not dynamic
execution multiplicity.

Backend blocker construction remains lossless and participates unchanged in
reference eligibility. At the exploratory linked-report boundary, exact
semicolon-delimited diagnostic fragments are compacted into a first-occurrence
inventory. The linked representation retains the source fragment count,
occurrence count and first ordinal for each exact fragment; human and pseudo
views render repeated fragments once with an explicit count. This prevents
symbolic path multiplication from dominating manual-analysis output without
pretending to recover semantic categories, dynamic execution frequency or the
full ordering of later duplicates.

The exploratory layer may follow those resolved edges to produce a reachable
effect inventory. The fixed-point summary groups MMIO, delay and typed semantic
shapes by the functions in which they were recovered, and identifies recursive
components. It is closed only when every reachable function body and edge is
closed. This propagation is intentionally not an effect-equivalence claim.

A separate affine projection associates a callee pointer argument with one
caller argument plus a constant byte offset. Such bindings may be composed over
simple call paths, allowing callee context accesses to be expressed in the root
function's argument/offset coordinates while retaining origin and path
provenance. Dynamic bindings, recursive revisits, arithmetic overflow and the
explicit path-state limit fail the projection closed without invalidating the
lower-level direct context inventory.

That projection also emits path-qualified semantic actions for both reviewed
external slots and reviewed direct functions. An action retains operation,
target, origin, static site, complete simple call path, typed argument shapes,
replacement hint and the exact contract/evidence identifier that authorized
the label. A separate lexical site-path array gives a stable source-order key
from the report root through nested calls; missing instruction sites remain
explicit rather than receiving invented offsets. Pointer arguments reuse
affine root bindings; scalar values retain their recovered symbolic form.

A conservative event-dispatch projection consumes those semantic actions only
when their reviewed contract declares the projection. The declaration maps
named ABI arguments onto mechanism-neutral roles such as channel, selector and
payload; mechanism and execution-context strings remain opaque to the contract
layer. The
generic linked layer therefore has no platform or operation-name dispatch
table. The result points back to the underlying action by stable index, so its
lexical site path, call path and factorized guard scopes remain the single
source of provenance. A complete interface record means that the reviewed
contract and expected argument schema matched; it does not mean that scheduler,
queue storage, callback execution or delivery effects were modeled. Receiver
inference is intentionally absent. A receiver appears only when the reviewed
contract names it; otherwise it stays unknown rather than being derived from
symbol spelling or pointer values. Schema blockers remain explicit.

Exploratory forced branch decisions are attached to direct calls as minimized
DNF guard paths. Semantic projection keeps them factorized by the function in
which they were observed: scopes compose conjunctively, paths within one scope
compose disjunctively, and decisions within a path compose conjunctively. This
avoids multiplying alternatives at every nested call while retaining lexical
provenance. Only exact complementary alternatives and absorbed supersets are
removed; symbolic conditions are not assigned guessed register, bit or event
semantics. Aligned bits derived from one symbolic source are losslessly
canonicalized to an ordinary mask expression; non-uniform bit provenance keeps
the existing explicit symbolic fallback. Guard atoms separately retain
call-result provenance as a producer identity, trace-local result token,
operand, compared-value bit mask and source-bit mask. For equality and
inequality against a constant, exact bit provenance also projects the constant
into producer-result coordinates. Producer identities share the
function-identity namespace,
which provides a structural join to the producer's return and MMIO inventory
without making the condition string into an API. An unresolved producer stays
explicit.

Function returns carry a separate bit-provenance layer. It partitions the
recovered 32-bit value into known-zero, known-one, unknown and dynamic source
ranges. A dynamic range records the exact source/output bit mapping and source
identity, including concrete MMIO address and SVD name where the value came
from a register read. This exactness describes the recovered value only and is
orthogonal to function/control-flow completeness.

For a guard result produced by a selected function, the linked layer projects
each tested result bit through that function's return provenance. A direct MMIO
range terminates successfully; an exact internal `call-result` range continues
at the mapped callee output bit. The resulting evidence retains result and
register masks, projects a known comparison value into both coordinate systems
and records the complete producer path to the MMIO leaf. Caller-side and every
producer-side inversion are composed, so shifted or inverted wrappers do not
get mistaken for aligned raw register tests. Traversal requires a resolved
function identity in the selected report and rejects a recursive `(function,
output bit)` revisit. Arguments, unknown arithmetic, external results and
unresolved calls remain explicit stopping points rather than guesses. Missing
guard evidence likewise stays explicit, and the report makes no CFG guard
completeness claim because exploration is bounded. Consequently this is
deliberately not a total execution trace: mutually exclusive paths coexist,
dynamic loop counts are not inferred and recursive revisits are bounded
exactly like context projection.

The linked report also projects reference-flow MMIO into per-function access
shapes and a project-wide `(address, width)` register index. Static accesses,
bounded indexed candidates and poll shapes retain path, address-expression and
write-bit provenance. This connects the manual pseudo-source to the register
inventory without treating candidate sets as dynamic occurrence counts.
Distinct write masks are retained at the register level and split into
contiguous candidate bit ranges linked back to their producing functions.
Whole-register and read-modify-write shapes are counted separately, without
promoting those mechanical masks to a peripheral-semantics claim.

Direct local branch conditions have a separate structural MMIO predicate
layer. It groups exact `BitSource::Register` provenance by read token, address
and inversion, retaining both compared-value bit positions and original
register bit positions. Equality and inequality against a constant additionally
map that value back to register positions, including shifted and inverted
fields. Relational or non-constant comparisons retain their operation and
operand but no guessed register value. Predicate discovery follows the bounded
branch exploration used for call guards and therefore carries an explicit
false completeness claim.

The project-wide index also forms conservative field candidates by joining
equal contiguous subregister masks from writes, poll predicates, direct local
MMIO predicates and exact guard-result links to an MMIO-backed producer return.
Evidence classes retain separate shape counts and structured predicate records.
Guarded records preserve whether the branch was taken and the effective
comparison after complementing a false branch. Candidates link access and
predicate functions; when a guarded semantic action is reachable, they retain
its target, origin, call path, call site and lexical site path. Scope/path
indices are stable coordinates into the action's factorized guard. Each MMIO
link also retains the selected DNF alternative, guard position and residual
path after removing that MMIO literal. Machine-readable and tabular indices are
zero-based; pseudo-source labels are one-based. Opposite polarities from
different action occurrences or different remaining conditions therefore
remain distinguishable without duplicating the complete action guard per
field. Full-register masks remain separate counters and do not become fields.
Guard evidence is accepted only for an address with one observed access width.
Producer paths keep all exact return wrappers in the function inventory while
the leaf MMIO reader alone is classified as the access function.
This join does not infer register or field names, merge adjacent hardware
fields, recover W1C or reset semantics, or guess through arithmetic and
unresolved return calls.

## Analysis model

- observable effect IR: memory, MMIO, calls, delays, fences and state ranges;
- symbolic values that do not name physical argument registers;
- affine caller-memory provenance used to recover context-structure offsets;
- SVD/register catalogs;
- draft and resolved reference-control-flow types shared between analysis and
  code generation;
- indexed-MMIO domain proof independent of an instruction set.

Profiles, dispositions, report rendering and workflow scenarios remain in the
facade. Effect-policy and semantic-adapter request/result interfaces live in
the neutral semantics crate; target dispatch and production-driver projections
live in the ESP32-S31 semantic harness.

## Architecture backend

- accepted object architectures and endianness;
- instruction decoding and control-flow classification;
- relocation interpretation;
- register file, stack, call/return and trap semantics;
- supported calling conventions and argument/return locations;
- architecture-specific final-image target discovery.

An architecture and calling convention are selected explicitly as one
validated pair. Initial supported pairs are `riscv32` + `riscv-ilp32`.
Planned pairs are Xtensa `call0` and windowed conventions and Thumb code using
`aapcs32-softfloat` or `aapcs32-hardfloat`. A backend rejects an unsupported
or contradictory pair rather than guessing from a chip name or Rust target
triple.
