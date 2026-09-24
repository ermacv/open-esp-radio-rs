# Blobray application

`blobray-application` owns import/query orchestration, supervision and explicit recovery.
Its internal dependencies are [domain](../domain/README.md),
[artifacts](../artifacts/README.md), [analysis](../analysis/README.md) and
[store](../store/README.md), [knowledge](../knowledge/README.md) and
[verification](../verification/README.md).
It does not depend on the legacy engine or a concrete Linux process adapter.

Saved register research uses the existing scoped navigation fact owner and one
operation-local declaration interval/name index. It retains exact evidence,
review state, physical occurrence applicability and partial coverage. The query
cannot promote observed access widths or masks to hardware declarations; source
publication remains with the independent register tool.

`Application::new` receives a host capability. `start_import`, `start_query`,
`start_plan`, `start_reopen_plan` and `start_run`
return a `RunHandle` from one owned job set; `import` and `query` are blocking
adapters over those paths. Import, image preparation and function analysis register typed durable project runs.
The application retains job ownership after client handles disappear. Shutdown
closes admission, cancels jobs and drains workers outside the registration mutex.
Concurrent shutdown callers wait for the same cleanup. Handles expose status,
bounded progress events, cancellation and a repeatable terminal outcome.

`ApplicationLimits` bounds active operations and retained results: default 16
slots and 64 events per operation, configurable within 1..1024 and 1..4096.
Admission returns `busy` when full. Completed job entries are reaped on admission
or shutdown; a retained client handle, transferred result or live plan keeps its slot.
Plan clones share a slot, while each execution requires another slot.
`RunHandle::take_output` transfers query output once. Waiting does not transfer it.

Import owns ordered roles, target selection, source provenance and thin-member
resolution. Worker-only `prepare_import` receives a staging capability, captures
inputs and returns a validated receipt; it cannot publish. The supervisor retains
one writer, validates/promotes the closure, linearizes cancellation against commit
and publishes the revision and completed run together. Signals and process-tree
mechanics belong to the injected host, not this library.

`create_project`, `inventory`, `revisions`, `runs`, `doctor` expose
project operations. `Application::recover` combines store ownership checks with
host process-identity and containment checks. Read operations do not migrate,
repair or schedule analysis. `inventory_stream` and `doctor_stream` receive explicit memory/control ports
and borrowed-record consumers. Materializing conveniences remain capped adapters
for small results. `ReadView` exposes selection, inventory and diagnosis without
writer/recovery access. `InventoryView` owns a verified manifest lease and lends
its byte source; it is not a transitive retention pin. Query admission resolves
current to a revision ID once before launching the worker.

`OperationHost`, `OperationWorker` and versioned execution messages belong to
this crate. Persisted run records and receipts belong to store. `prepare_query`
produces a private typed disk stream and captured manifest outside the project;
Human/JSON rendering belongs to the frontend. Workers are reaped before output
becomes available. `QueryOutput` continues the original work/deadline budget and
retains the maximum observed working capacity during one delivery attempt,
including failure. Drop removes temporary output; no query journal is persisted.

Missing inputs may produce a completed but incomplete inventory. Execution
failure, cancellation, timeout, resource exhaustion and abandonment remain
separate outcomes. See the [implemented contracts](../../next/README.md) for
schemas, resource modes, cancellation semantics and lifecycle limits.

`RunContext` owns operation-wide work accounting through the domain `RunControl`
port and obtains time, cancellation and observation from `RunEnvironment`.
`prepare_import` requires that port explicitly. Worker accounting continues in
coordinator retention; a successful worker receipt without accounting is invalid.
Progress and terminal errors retain phase and physical position. The primary
failure stays separate from bounded cleanup/persistence diagnostics. See
[cooperative control](../../next/reference/resources-storage/README.md#cooperative-control-and-failure-diagnostics)
for policy, protocol versions and working-memory boundaries.

The application creates one `WorkingMemory` authority per worker operation.
Import releases each thin payload and object scratch before advancing the member
cursor. Artifact records go directly to store-owned streams; the application does
not accumulate member leases, object inventory or revision data. Capacity failure
retains structured allocation context and prevents publication.

`Plan` owns an immutable inspection recipe and captured manifest; it exposes no
store project, writer or recovery capability. `planning` owns creation, bounded
serialization, reopening and execution admission. `selection` filters the common
store stream and retains only bounded recipe metadata or one selected code occurrence's
section index. Domain supplies revision-local selectors; names only enumerate
candidates. No scheduling graph or analysis engine is introduced for inspection.

Each execution retains the plan independently of client handles, validates its
manifest/recipe and streams the selected records under the saved execution
budget. Reopening explicitly checks a selected project; it cannot rebind current.
See [selection and plans](../../next/reference/capture-images/README.md#selection-and-inspection-plans)
for formats, CLI use, lifecycle, capacity and coverage limitations.

`TemporaryStoragePolicy` and the `temporary` module own per-operation admission,
the aggregate Application pool and versioned runtime workspace lifecycle.
`OperationHost::temporary_root` supplies a validated private root;
`TemporaryStorageStatus` exposes reservations and cleanup residue. The local
policy is absent from Plan serialization. A completed result shrinks its full
admission to retained file lengths; Plan clones share that reservation. Cleanup
failure retains the charge and diagnostics. Query runtime reconciliation is
automatic for proven abandoned workspaces; project import recovery stays explicit.
`write_control_message` is the bounded 64 KiB host-control serializer, including
guard and worker reports. See the [temporary storage contract](../../next/reference/resources-storage/README.md#temporary-storage-and-crash-cleanup)
for accounting, defaults, observations and cleanup limits.

`linking` owns the synthetic image policy and the `LinkerHost` port. `LinkPlan`
retains a frozen description and captured manifest; image admission revalidates
its project and revision. `LinkInvocation` passes layout, exact ordered members,
roots and companion definitions to an identified ElfAnalysisLinkV1 adapter.
`LinkWorkspace` lends quota-admitted files and a bounded capability probe;
`LinkOutputSink` receives raw output, normalized observations and reaped seekable
ELF ownership. Application never parses linker-specific map text. `start_prepare_image` uses the same durable supervisor,
staging and commit boundary as import with a typed image receipt. Normalized placement and extraction evidence
must prove the exact entry/root occurrences before publication. Saved image
queries and exports use the ordinary read-query lifecycle. See
[prepared images](../../next/reference/capture-images/README.md#synthetic-prepared-images) for the supported
profile, mapping limits, metadata capacities and resource ownership.

`functions` owns exact captured-object/prepared-image function selection and the schema-1 `FunctionWork`
message. It passes admitted bytes and raw relocations to `blobray-analysis` with
an injected `FunctionSemantics`, never a concrete ISA dependency. The same durable
supervisor publishes typed function receipts, including semantic incompleteness.
Read/export operations consume retained records without scheduling computation.
See [function analysis](../../next/reference/analysis/README.md#function-analysis-contract).


`investigations` owns object-input or prepared-image selection and execution through
`ReadQuery::PlanInvestigation` and `Application::start_analyze_project`. It freezes
an `InvestigationPlan`, validates every explicit selector, and streams exact
function occurrences plus input/object coverage. The worker re-enumerates the
frozen revision before executing, then calls the same `FunctionEngine` as single
function analysis. It shares one work/deadline/working-memory/disk budget, caches
only the current container/object, and releases function state between calls.
Missing extents and unsupported functions become blocked outcomes; resource,
cancellation, integrity and I/O failures prevent publication.

The existing supervisor owns registration, containment, retention and the final
store transaction. Publication members and child results are invisible until
that transaction completes. Publication/status/access/reference queries consume
retained records only. `RunControl::progress` lets the store persist final
accounting in the same transaction after streamed child inserts. This operation
adds no scheduler, backend dependency or subprocess per function. See the
[library contract](../../next/reference/analysis/README.md#library-investigations).

## Knowledge and preservation

`occurrence` acquires exact captured objects for both data delivery and knowledge
admission. Prepared callbacks validate optional physical symbols once and keep
source/range checks under the same owner. Thin-member payload roots remain
explicit; the helper grants no publication or review authority.

`start_knowledge` admits proposals and review changes through the durable job
supervisor. The worker resolves exact retained occurrences, verifies evidence
and uses `blobray-knowledge` for conflicts/transitions. Store atomically publishes
the event and run outcome. `ReadQuery::Knowledge` freezes its head at admission;
`ValidateKnowledge` checks a change without publication.

Backup, restore and legacy import use the same supervised query lifecycle. The
legacy adapter can create a writer only for its new private staging project;
it cannot mutate the original legacy project or an existing destination. The
caller publishes a verified new project with `QueryOutput::publish_restore`.
Original bytes, unresolved references and unsupported representations remain
explicit catalog records. [Commands and contracts](../../next/reference/knowledge-review/README.md#knowledge-and-preservation)
define the formats, bounds, publication boundary and supported conversions.

`ReadQuery::NamedLinkPlan` resolves one defined entry in an explicit input and
returns the ordinary exact link plan. Missing/ambiguous names return candidate
records without selecting a definition. Prepared-image analysis freezes that
image's original revision, retains its ELF lease, and uses the existing function
engine. Queries expose saved functions, call targets, memory accesses and mapping
precision. No query performs linking or analysis as a side effect.

## Research ownership

`research` resolves a saved function through image-qualified publication entries,
walks the reachable call closure iteratively, and supplies acyclic callee facts to
analysis. Recursive components remain explicit gaps. It applies only the selected
knowledge revision. The local ELF borrow ends before summary composition starts.
No second worker, parser, scheduler or provider registry is introduced.

`companions` validates exact retained ROM definitions for the common link recipe.
It rejects name collisions and synthetic-placement overlap; the host still only
executes the explicit linker invocation. Knowledge occurrence validation shares
input/image identity with function analysis. See the
[native workflow](../../next/reference/analysis/README.md#phyrom-research).

## Concrete execution

`start_execution` pins the executor/environment/verifier identities and admits an
`ExecutionRequest` before cloning worker state. `execution` resolves captured
inputs and prepared images; `execution_memory` owns mutable session regions,
initialization state and bounded events. `Executor` is injected from domain;
application has no concrete ISA dependency. The pure verification crate owns
comparison. All cases share the ordinary durable supervisor, budget, staging and
publication boundary. Read/replay clients use `ReadQuery::Execution`.
See [execution and comparison](../../next/reference/execution/README.md#concrete-execution-and-comparison)
for stateful lifetimes, resource obligations and claim limits.

`ReadQuery::AuditTargets` reads an explicitly selected ELF outside a project under
the same ephemeral supervisor. `prepare_query_with_tools` injects its decoder and
linker capabilities. The artifact owner validates executable sections; analysis
owns target policy evaluation. Findings are streamed and no project is opened.

Concrete compound operations (`start_analyze_project` with automatic/saved input,
`start_research`, `start_propose_register`, `start_replay`) belong to application.
They retain one supervisor, worker and original budget through resolution and
publication. CLI does not carry budgets between separate operations.
`QueryOutput::assessment` and `QuerySummary::assessment` use the same scoped
assessment as `RunRecord`; lifecycle, research coverage, policy and comparison
are separate. See [contracts](../../docs/design/contracts.md#result-assessment).

Whole-library execution shares a prepared object/section across its function
views. Research uses admitted publication/MMIO indexes and one frozen knowledge
snapshot. Fixed phase costs and work counters survive coordinator retention.

Data research resolves an exact occurrence once and borrows ranges from one prepared
object. `ReadQuery::Data` and `ReviewedData` share the query/delivery budget;
`start_propose_data` and `start_propose_constant` resolve exact evidence within one
supervised scenario and publish through existing knowledge transactions.
`QueryOutput::export_data` transfers captured object/bytes/records and provenance
to a new directory. Review state and analysis coverage remain separate.

`coverage` streams selected extent unions against captured inventory, while
`storage-usage` observes persistent file sizes without a writer or recovery.
Single-function enrichment starts after the prepared ELF and references end;
research records retain capacity only until their last composition consumer.

Function requests carry `FunctionSelector::Symbol` or an explicit
`FunctionSelector::Range`; both use shared captured-occurrence acquisition.
Reviewed executable ranges and symbol-size overrides enter one investigation
enumeration and one function engine. Coverage joins ranges directly to section
metadata and symbols to their physical table records before counting the union.

Data observation/review/export also owns pointer tables. Requests explicitly
select one captured range and a count/stride profile; the application injects
backend relocation semantics into the streaming analysis port. Accepted exports
preserve the profile, raw bytes, physical relocation identities and classification
counters without promoting external symbols or numeric addresses to callees.

Interface proposals and reviews use the existing knowledge lifecycle and captured-occurrence helper. Exact symbol and function-range roots and captured-payload guards are checked against retained bytes. Runtime guards are conditional metadata and do not resolve callbacks or grant execution authority.

The `interfaces` read scenario selects saved analysis facts or a captured pointer
span plus an explicit knowledge revision. It owns one admitted record/object view
and one sorted path/slot binding index. It exposes every matching review state and
unverified runtime condition without hidden analysis, callee selection or model
execution. CLI and API share this query; JSON export retains reference identities.

`ReadQuery::Navigate` owns explicit publication/analysis/knowledge selection,
operation-local physical indexes, context argument mapping and streamed evidence.
It releases each function record owner before reading the next; shared dependency
handles belong to one store reader. Navigation never starts analysis implicitly.

`ReadQuery::Flow` reuses navigation selection and edge resolution, owns compact
call graph metadata, and streams reached effects in a second linear pass. Native
path proposal/review rechecks exact immutable hops through this shared resolver.
No path query invokes analysis, and review is not executable reachability.


`ReadQuery::MemorySlice` loads one authenticated saved record owner and streams
bounded local definition queries under the same supervision/export lifecycle.
Application owns acquisition and atomic delivery; analysis owns CFG/alias rules.


`ReadQuery::EventRoute` and native route proposal/review share one evidence path.
Navigation supplies exact selected callees and a synchronous borrowed-facts port;
route orchestration retains only admitted values/metadata between participants.
Physical structural checks are distinct from service semantics and temporal
conditions. No route query launches hidden analysis or executable delivery.

`start_build_ir` packages explicit saved scopes into immutable named semantic
profiles. The common navigation resolver supplies physical links and publication
names; finite profile propagation and a separate admitted provenance worklist
preserve the distinction between selected callees and evidence-only dependencies.
The build owns one supervised budget/publication. `ReadQuery::SemanticIr` expands
original function facts through `QuerySink::semantic_ir`; no hidden analysis or
live-origin access occurs. See [IR profiles](../../next/reference/ir-traces/README.md#saved-semantic-ir-profiles).

`ReadQuery::Trace` selects original local members of retained IR profiles and exposes
`QuerySink::trace` evidence and a scoped static comparison verdict. Each side's saved
facts are loaded once and released before the next side. Only ordered observables
and an admitted canonical expression index overlap. Unknowns and composition-only
facts never trigger an implicit analysis or execution workflow.

Concrete invocation setup owns the aligned stack argument area and its knownness.
Explicit unknown words invalidate seeded bytes before execution. Domain validates
physical ABI word capacity/placement; the backend receives optional a0–a7 words.
All setup and phase resets share the execution budget and retained request.

Each concrete session owns one exact four-byte LR reservation. SC always clears
it and checks write permissions; overlapping successful writes invalidate it.
AMO updates validate/admit before invoking backend arithmetic once. The environment
executes one hart in program order; reservations reset between phases and atomic
MMIO is unsupported.

Execution cold phases recreate captured mappings; warm phases retain writable ELF
and session RAM. Phase RAM and stack release after evidence serialization/comparison.
An incomplete phase blocks warm successors; a cold phase starts another independent
chain within the same budget and publication. Live RAM lifetime changes conflict.

All execution goals resolve through exact captured occurrences before sessions are
allocated. Borrowed operation indexes group phase/side goals by object, prepare it
once and release ELF bytes immediately after address validation. Early goals close
a phase without claiming return or callee-body execution; warm successors start
their own entry with only session-owned memory retained.


`devices` owns admitted configuration/state, exact sorted ports and phase/session closure. Session snapshots model participation before releasing closed instances; verification sees both code outcomes and due model obligations. Warm continuation cannot redeclare a live id. All models use the shared execution/replay path and budget.


`external_calls` owns immutable response copies, admitted instances and cursors. `execution_memory::execution_calls` validates complete effects, writes only checked normal memory, and owns bounded allocations. Phase/session closure and call evidence share execution supervision, capacity and atomic publication.

`execution_interfaces` resolves explicitly selected accepted interface roots from
captured objects before sessions. Admitted in-place sorting groups requests first
by selected knowledge snapshot, then by revision/source/object. Each request still
receives its own occurrence, layout and root validation; returned tables retain
phase/side/declaration order. Captured ELF owners end before session allocation.
`runtime_tables` owns admitted live instances and
bounded current-target indexes; `execution_memory::execution_tables` places bytes,
checks conditions and connects eligible indirect transfers to captured code or
explicit call models. Stores, atomics and model outputs update lifecycle evidence.
The [runtime interface contract](../../docs/design/contracts.md#runtime-interface-instances)
defines phase release, snapshot selection and the limits of value association.

`fifo_services` owns admitted FIFO rings and binding indexes by phase/session;
`execution_memory::execution_services` validates reviewed slot dispatch, current
ABI words and private-stack effects before mutation. It signals a selected
successful dequeue goal through the existing execution port. Queue state is
independent for each implementation and never implies real RTOS scheduling.

`execution_memory::execution_observation` admits and captures exact selected normal
memory at the phase stop before releasing phase owners. It preserves unchanged,
unknown and unavailable bytes in bounded chunks, without device reads. Snapshot
capacity lives through serialization/comparison and is released on recycle. Each
comparison case supplies its own relation to verification; no frontend composes it.

`layout_projections` validates both captured entries and branch instructions through
the required knowledge-worker decoder capability. It shares generic proposal/review
publication with `start_propose_projection`; CLI only supplies the request. Execution
borrows store-owned accepted projection selections through comparison/serialization.
