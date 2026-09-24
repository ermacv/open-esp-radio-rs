# Blobray domain

`blobray-domain` owns shared identities, schema-1 revisions, resource budgets,
outcomes and portable byte/record/control ports. It has no internal crate,
filesystem, database, backend or frontend dependencies.

[Revision types](src/lib.rs) retain physical object/symbol identities, lossless
names and origins, capture outcomes and ordered bindings. `Revision::validate`
checks manifest relationships without storage. Recorded malformed ELF references
remain coverage diagnostics; broken manifest ownership is a different error.

[Execution values](src/jobs.rs) distinguish attempt identity from content identity,
requested kernel/watchdog enforcement, lifecycle states and terminal outcomes.
Persisted run/owner records and import receipts belong to
[store](../store/README.md); host launch, observation, cancellation and reaping
ports and private worker messages belong to [application](../application/README.md).

A `Snapshot` owns selected revision data, with no writer authority or live source
handles. The [implemented contract](../../next/README.md) describes schemas and
authority. Knowledge values are implemented; verification remains outside this
crate's implemented scope.

[Resource ports](src/resources.rs) define `RunControl`, injected `RunEnvironment`,
physical progress and bounded diagnostic records. Work policy 1 and its default
budget are shared values; the application owns enforcement. `ResourceBudget` uses
optional work fields only to represent unknown values in old records; new-run
admission requires a positive limit and supported policy. Fixed-size `ControlStop`
is separate from rendering/storage errors that still use normal host allocations.
`Revision::validate_controlled` visits retained relationships under the caller's
control; `validate` remains the synchronous unmetered snapshot convenience API.

[Memory contracts](src/memory.rs) provide `WorkingMemory`, RAII reservations and
fallible `ScratchBytes`. Borrowed scratch cannot outlive its authority, and live
reservations cannot exceed its capacity. These values account requested capacity,
not resident process pages. `MemoryFailure` carries a rejected reservation without
requiring memory from the exhausted pool. Old records omit new observations.

[Streaming ports](src/stream.rs) separate stable positional byte reads from ELF
and inventory consumers. Callbacks borrow records for one call; retaining copies
requires consumer-owned capacity. Query admission, worker ownership and rendering
formats are outside domain. The wire revision schema remains 1.

[Selection values](src/selection.rs) qualify object/symbol IDs with an input ordinal
inside a fixed revision. Exact-byte name searches return candidates, not implicit
resolution. `RevisionHeader` supplies producer/target context through streaming
callbacks. `PlanId` identifies a bounded application recipe; recipe construction,
execution, storage leases and Human/JSON formatting remain outside domain.

[Temporary storage ports](src/temporary.rs) define `TemporaryCapacity`,
`TemporaryUsage` and `StorageFailure`, independently of Plan identity. The shared
`storage_io` adapter preserves typed capacity errors and distinguishes filesystem
exhaustion (`disk-full`). Application chooses local policy; store owns bounded
files and accounting. Optional progress/error fields preserve unknown historical
observations. See [temporary storage](../../next/README.md#temporary-storage-and-crash-cleanup).

[Image values](src/image.rs) own exact root selections, bounded RV32 regions,
linker identity, versioned synthetic recipes and retained manifest/provenance
values. `LinkPlanId` and `PreparedImageId` are distinct from inspection-plan,
revision and run IDs. Recipe identity excludes local emergency budgets; image
lifecycle and publication belong to application/store.

[Function values](src/function.rs) define physical symbol or explicit
object/section/range selectors, exact requests, extent authority,
typed input/image sources, address spaces, per-obligation coverage and the `FunctionDecoder` / `FunctionSemantics` ports. Typed operations and
abstract values describe local effects without granting access to machine state.
`FunctionAnalysisId` identifies a retained manifest. Dynamic execution, inferred
function extents and cross-archive definition selection are not implicit in these
values. Calls and possible continuations remain distinct graph observations.


[Investigation values](src/investigation.rs) distinguish a bounded saved plan
from its immutable publication. Entries retain exact function identities and
explicit gaps; membership assigns each function either a saved analysis or a
semantic blocker. Coverage distinguishes analyzed and complete functions.
`InvestigationStatus` separates revision freshness from coverage.
`InvestigationFinding` preserves publication/function/instruction provenance.
Selection, hashing policy, I/O, execution and publication authority belong to
application/store, not these values. `InventorySink::container` reports framing
coverage before object callbacks, so a partial archive cannot appear complete.

`ImageMemory` is a borrowed immutable-byte port, not a loader or mutable execution
bus. `RiscvAbi` records the ELF calling convention separately from the RV32
integer-analysis target. Function transfers preserve image-qualified source
identity and unknown targets. `NamedLinkRequest` is explicit input order and a
name-selection request; only application can resolve it into an exact recipe.


Concrete execution values include exact targets, explicit scenarios, producer
identities, observations and comparison evidence. `Executor` and `ExecutionMemory`
are injected ports; domain selects neither an ISA nor an environment. Request
validation bounds control cardinalities. Host admission also bounds serialization
before copying requests. See the [execution contract](../../next/README.md#concrete-execution-and-comparison).

`ResultAssessment` separates scoped coverage from policy checks and comparison verdicts. `ScenarioRequest` defines four concrete application actions. `AdmittedVec` admits overlapping buffers before growth. Fixed `WorkMeasurements` and phase costs report resource work independently of result identity.

Data contracts identify exact captured ranges, integer table encodings and constants
selected from retained analysis operands. Observations, review state and function
coverage remain separate; file offsets and executable VMAs are distinct values.

`CoverageScope` makes the completeness universe explicit. `RecordBuffer` owns
retained function records and their admitted variable capacities. Phase memory
observations describe admission, not RSS. Extent and storage reports are typed
observations without discovery, pruning or hardware claims.

Native interface declarations retain exact symbol/function/address roots, explicit pointer paths, index preconditions, conditional guards and typed slot signatures. Their semantic keys are labels, not runtime models; function range roots need no fabricated symbol.


`MemorySliceQuery` and its records distinguish exact anchor identity, local spans,
incoming uncertainty, last-write classes and unresolved barriers. Saved expression
IDs scope pointer identity to one analysis; witnesses are structural evidence.


`ReviewedEventRoute` defines three finite conditional mechanisms; `EventRouteQuery`
and its output separate exact saved evidence, structural check outcomes and runtime
conditions. No schema field promotes static bindings to completed delivery.

[Saved IR values](src/semantic_ir.rs) define configured profile selection and original
fact/provenance membership. [Static traces](src/trace.rs) define explicit physical
observation scope, ABI/register inputs, symbolic values and MATCH/DIFF/INCOMPLETE
results independently from operation completion. Their evaluation belongs to analysis
and application. Function schema 7 / policy 8 includes typed fences and outgoing tail
inputs; display strings have no semantic authority.

Concrete execution schema 9 uses bounded optional RV32 ABI words: a0–a7 followed
by ascending stack slots. `Invocation::entry_stack` validates placement within the
declared stack; `register_arguments` preserves missing values as unknown. These
are physical words, with type/variadic lowering owned by the caller.

The atomic memory port owns reservation/access/update indivisibility; backend
callbacks supply pure word arithmetic. Ordering bits are explicit inputs. Missing
access is distinct from a failed SC reservation.

Execution cases declare cold/warm resets and per-invocation entry PCs. RAM mappings
declare phase/session lifetimes; the first case must be cold. Targets own captured
address spaces and stack geometry, not one fixed entry.

Invocation goals distinguish return, reach-symbol and observe-call. `ExecutionStart`
lends resolved boundaries to the executor. Completed goal outcomes, premature return
and ordinary gaps remain distinct; `ExecutionStop::completed` means the declared
phase goal was reached. Non-return goals cannot compare return registers.


Device declarations bind caller applicability, phase/session lifetime and every ordered configuration field to a stable content identity. Model observations separate code goals from participation and closure obligations; successful return alone cannot complete an unconsumed transcript.


`CallDeclaration` and `CallObservation` retain exact modeled ABI boundaries, response/effect identities and phase/session obligations. The required call-dispatch port distinguishes captured code from explicit modeled returns and issues; argument words and output ownership are never inferred.

`runtime_interface` defines selected review identities, runtime placements, physical
slot targets, condition gaps and lifecycle observations. Invocation `tables` is
explicit, including an empty array. Code-goal completion and table/call/device
obligations remain separate. Indirect target association records current value,
not inferred pointer provenance.

`fifo_service` defines explicit bounded queue owners, reviewed slot bindings,
physical input/output contracts, transitions and closure observations. A selected
`observe-dequeue` goal observes a successful modeled event and output write;
code return, queue status and comparison verdict remain separate claims.
