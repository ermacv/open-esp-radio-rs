# Interfaces and ownership contracts

This is the contract authority for Blobray Next. It describes implemented
boundaries; a capability it does not describe is not provided. The
[command reference](../../next/README.md) owns CLI syntax.
[Architecture](architecture.md) owns components and [workflows](workflows.md)
owns supported use cases.

## Result assessment

`RunState` describes lifecycle only. `Completed` means a valid result was delivered
or atomically published; it does not promise complete research or a matching
comparison. `ResultAssessment` is shared by `QuerySummary::assessment`,
`QueryOutput::assessment` and `RunHandle::wait().assessment`:

| Field | Meaning | Applicable operations |
| --- | --- | --- |
| `coverage: {subject, status, scope}` | `complete`, `partial` or `unknown` for the identified result, never an unscoped boolean | Inventory revision, function analysis, investigation publication or static-target audit |
| `check` | `pass`, `fail` or `inconclusive` policy decision | Doctor, link-plan and static-target audit |
| `comparison` | `MATCH`, `DIFF` or `INCOMPLETE` under the retained relation | Static trace comparison only |

A failed/cancelled/limited attempt has no assessment. Successful listings,
selection, inspection, image preparation and exports have an
empty assessment unless a specific result contract supplies one. A function
without semantic analysis has unknown semantic coverage even if decoding is
complete. A partial inventory returns partial coverage in both query summary and
handle. CLI returns success for valid partial research and comparison evidence;
a failed/inconclusive check command returns failure. No `clean` alias or generic
run-level `complete` field exists.

Store validates assessment subject against the published result identity. A
completed durable run contains exactly one result reference. The [current native formats](../../next/reference/interfaces-formats/README.md#current-formats)
are mandatory; incompatible projects/journals are rejected
without conversion. Execution and revision manifests keep independent versions.

`coverage.scope` is mandatory: `inventory-occurrences`, `function-extent`,
`selected-function-extents`, `execution-scenario` or `static-resolved-transfers`.
Whole-library completeness does not quantify over every executable byte. The
`coverage` query reports section-relative intervals outside the union of selected
extents separately; these intervals are neither inferred functions nor padding.
Unknown structure remains explicit and does not become a zero-byte observation.
`PASS` for target auditing covers resolved static transfers only. Unknown indirect
transfers remain separately counted. Comparison retains its explicit relation:
per-case event channels, low/high return words, exact paired final-memory
selections, physical call observations and the selected internal physical
timeline, explicitly reviewed layout projections and reviewed effect contracts.

### Durable and ephemeral acquisition

Import captures live inputs for durable research. `audit-targets` is an explicit
exception: the supervised read operation captures one external ELF into admitted
memory, checks detectable file changes and returns the digest of bytes actually
analyzed. It does not create a project revision or claim an atomic filesystem
snapshot. Durable operations never use this path to reopen missing source files.

## Automatic investigation

`start_analyze_project(InvestigationInput)` owns one admission, run, writer,
worker, deadline, work counter, working-capacity authority and disk budget.
Automatic investigation freezes the source revision at admission and retains its
plan.
CLI parses parameters and renders outcomes; it performs none of these resolutions.

An `AutomaticInvestigation` journal operation retains the admitted request and
producer. `resolved_operation` records the `Investigate` run of the plan only in
the successful publication transaction; coordinator validation binds the plan
back to the admitted request. Failed attempts retain no resolution, result or
assessment. Resolution is not a nested run. Saved-plan investigation uses the same execution path with
its selection independently verified. Automatic planning passes its temporary
entry stream directly into execution under the original budget.

## Research memory and read-only observations

Single-function local analysis returns an owned staged manifest. Its prepared
ELF, section views and reference indexes end with the analysis. Library analysis
keeps one prepared object across its local functions. No global object cache
exists.

Record loading admits a bounded JSONL decoding workspace before deserialization,
then transfers records into `RecordBuffer`: vector capacities, boxes, names,
identities and nested value collections remain charged with their owners. Shared
reference targets are conservatively charged per reference. This is capacity
admission, not a no-heap or whole-process RSS guarantee.

Phase diagnostics contain `reserved_bytes` at the last admission/release
observation and `peak_reserved_bytes` at such observations, alongside work/time.
They include allocations carried into that phase; phase peaks must not be added.
`load-research` distinguishes retained facts from ELF preparation. These measurements are diagnostic and do not enter result identity.

`storage-usage` is a supervised read query. It counts logical file sizes in CAS,
metadata and staging, including unreachable CAS files. Metadata row counts share
a read transaction; filesystem sizes are observations over the traversal interval.
A disappearing entry fails explicitly. Symlinks are counted as non-regular entries
and never followed. No reachability, reclaimable-space, physical allocation or
project-wide disk quota is claimed. Neither this query nor `coverage` acquires a
writer, recovers staging or prunes results.

## Prepared data ownership

Artifacts owns `with_prepared_object`: captured bytes, parsed ELF/program view,
shared symbol targets and prepared section metadata live in one callback scope.
Borrowed function views cannot escape; IDs preserve exact occurrences and physical
relocation records. Application owns grouping, captured leases and archive ordinal
indexes. Analysis owns normalized section references and logarithmic offset,
symbol and physical-pair lookup. Whole-section HI/LO resolution preserves missing
and ambiguous outcomes. Memory admission uses element sizes and owned name bytes;
it does not reserve a fixed large allowance for each relocation.

`AdmittedVec` admits simultaneous old/new buffers before growth, fails without
changing existing contents and releases its reservation on drop. Research indexes
are local to one run. No global cache, scheduler or allocator exists. `WorkingMemory` remains
capacity admission, not an RSS meter or a claim of zero system allocations.
Fixed phase/counter diagnostics continue across worker/coordinator handoff.

## Identity and provenance

Identifiers are typed values with validated, versioned encodings. Paths, labels
and formatted addresses are presentation metadata and cannot substitute for an
identity at a subsystem boundary.

| Identity | Meaning and scope |
| --- | --- |
| `ProjectId` | Persistent investigation identity, independent of directory name |
| `ArtifactId` | SHA-256 identity of exact captured or derived payload bytes |
| `ObjectId` | Artifact identity plus standalone-object or archive payload ordinal |
| `SymbolId` | Object identity plus symbol-table kind, table section and entry index |
| `RevisionId` | Immutable source manifest: ordered input roles/bindings, captured occurrences, target and inventory producer |
| `PublicationId` | Immutable completed result root for an identified revision; independent of the mutable current-publication reference |
| `PlanId` | Immutable operation recipe bound to a revision and requested obligations |
| `RunId` | One execution attempt of a plan; retries have separate run identities |
| `SubjectId` | Caller-assigned semantic subject of a reviewed contract; association with physical occurrences is explicit |

Archive payload ordinals preserve independent occurrences even when names and
bytes repeat. An identical content object may be physically deduplicated while
its distinct occurrences remain addressable. Thin-archive container identity
alone does not identify its external payloads: the revision records each member
occurrence's imported artifact binding.

An occurrence selector is interpreted inside a retained revision and ordered
input binding. `InspectionScope` includes the input ordinal because a revision
can import the same container more than once. In particular,
unchanged thin-container bytes can name different external bytes in two imports:
the same `ObjectId` or symbol-table position is not proof of unchanged payload.
Analysis dependencies and evidence include the revision's resolved payload
binding, not only the container digest and ordinal. Cross-revision reuse requires
equal relevant captured bindings and interpretation inputs. This rule preserves
schema-1 selector encodings; it does not silently assign new meanings to old IDs.

A section offset identifies a location in an identified object section;
`ImageAddress` identifies an address in a selected prepared image. Conversion
requires that image's recorded mapping. Address widths are validated against the
selected target; no narrowing to RV32 occurs in generic identities or storage.

Every observation records its subject, input identities, producer identity,
applicability and completeness. An evidence reference identifies both record and
relevant location within it. A missing supporting record cannot be
reconstructed from a display label.

## Resource owners

| Resource | Owner and acquisition | Handoff and release |
| --- | --- | --- |
| Source bytes | Store import transaction copies caller-selected inputs | Immutable file leases retain captured content; caller keeps ownership of original files |
| Parsed container | Artifacts constructs views over a lease | Views cannot outlive their bytes; parsing never reopens a source path |
| Project snapshot | Store reader captures revision and selected publication in one metadata transaction | Cloneable read handles retain leases; final release removes transient retention |
| Write transaction | Store grants one writer authority | Commit consumes the transaction; rollback/drop leaves published roots unchanged |
| Prepared image | Image-preparation operation under a job | Artifact and mapping leases transfer to execution; source and tool identities remain attached |
| Execution session | Application, created for a declared scenario lifecycle | Warm phases retain session-owned state; cold reset/completion/cancellation releases it |
| Run handle | Application supervisor registers a job | Client requests cancellation or waits; supervisor owns cleanup regardless of client handle lifetime |
| Child process and temporary files | Host process adapter under the supervisor | Termination includes descendants and reaping before job resources are released |
| Export bundle | Application export operation over a snapshot | Caller receives materialized files and manifest; repository evidence ownership is unchanged |

Read handles expose no writer capability.
Domain operations receive borrowed typed inputs and ports with the minimum
authority they need. A callback or generic context object cannot recover the
whole application or database connection.

## Application interface

All frontends use these operation families. Requests are typed; JSON is a
versioned serialization of the same requests/results, not a second workflow
implementation. Human output is a renderer of those results.

| Operation | Inputs | Result and authority |
| --- | --- | --- |
| `import` | Explicit file bindings, order, target context and optional expected digests | Staged import followed by a new revision; the only operation that acquires live binary paths for durable project research |
| `snapshot` | Project and explicit revision/publication selection or current selection | Read-only retained snapshot with resolved identities |
| `plan` | Snapshot, operation request and resource budget | Immutable plan, dependencies, obligations and missing prerequisites; no analysis publication |
| `start_run` | Plan and application capabilities | `RunHandle`; executes the recorded plan rather than resolving new inputs |
| `query` | Snapshot and typed selector | Observations/projections with provenance, completeness and diagnostics; no implicit analysis or repair |
| `in_process::verify` | Request, the executables of both implementations, selected contracts and projections | Records, verdict and completeness in memory, with explicit claim scope |
| `export` | Snapshot, selected records, destination and overwrite policy | Verified bundle/manifest; never changes retained evidence |

`RunHandle` exposes status, typed progress events, cancellation and completion
waiting. Every event identifies its run and phase. A late result from an older
run cannot replace a frontend's selected revision. Event delivery is bounded:
intermediate progress may be coalesced, but the terminal outcome is retained and
available through status/wait even when a client stops consuming events.

Plans retain the exact revision, requested entry occurrences, ordered input
roles, linker recipe where needed, provider/model identities, pass and schema
versions, parameters, declared environment inputs, outputs and resource budget.
Planning may hash and read imported inputs. It does not execute analysis or
migrate a repository.

A run can complete on an older revision after the project advances. Its result
remains bound to that revision. Advancing the current publication requires the
expected base revision/publication to still match; otherwise the completed
result is retained and the caller receives an explicit publication conflict.
There is no automatic rebase of a running computation.

Next implements `Plan` for inventory inspection only: one revision, input,
object or symbol scope. Its live owner retains a captured manifest; portable
reopening revalidates project membership and recipe dependencies. It is not a
general pass/provider graph or a transitive retention pin. See [inspection plans](../../next/reference/capture-images/README.md#selection-and-inspection-plans) for
the implemented schema, resource handoffs and command semantics.

### Handles and capability boundaries

External link definitions acquire one captured function address, not a runnable
program view. Artifacts validates the selected static symbol table, nonzero
extent, allocated executable section and unique file-backed executable mapping.
Non-executable data sharing a virtual address, TLS/dynamic metadata and executable
mappings unrelated to that extent remain carrier facts; they grant no loading or execution support. Application retains
the exact source/symbol selection in the link identity and never emits a stub.
Execution and research must separately admit any carrier they actually map or
analyze under their existing whole-image profile.

An import is the acquisition operation before a closed revision exists. Its
admission freezes requested origins, order, expected digests and policy; capture
then determines the new content identities. This request is not an analysis plan
over immutable bytes. All later plans use the resulting captured bindings.

Read selection may use `current` only at admission; subsequent work uses the
resolved revision/publication even if current advances. A live plan is an owned
recipe, not a command that re-resolves the latest project on execution. Planning
does not persist a new project record implicitly. Rendering or exporting the
recipe is a separate caller action.

Application requests and results carry semantic selections, not human output
formats. CLI/JSON render the same results; host-internal transport may stream
or spool them. Collecting an entire stream is an explicitly bounded adapter.
Low-level ports do not acquire an implicit supervisor; their caller must supply
the declared control, memory and retention capabilities.

## Import and revision capture

Import streams bytes into private staging, hashes the copied content and
validates supplied digests. Files are copied, never hard-linked to mutable
caller inputs. The importer rejects detectable source mutation during capture
and permits an explicit retry. Without an expected digest or a caller-provided
filesystem snapshot, it guarantees the identity of captured bytes, not that
several mutable external files existed together at one instant.

The application resolves configuration, reviewed packs, target declarations and
external thin members into a closed revision manifest before admitting a run.
Parsing and validation use captured content. Changes to working files after that
point belong to a future revision. A manifest can retain a declared missing or
unsupported input with diagnostics for investigation; an operation requiring
that input cannot report its obligation as satisfied.

Thin-member paths are resolved relative to their archive import origin and the
explicit input bindings. Their bytes become imported objects with independent
digests. Later parsing/linking never follows those original paths. Materialized
link inputs preserve membership/order and record any reconstructed container as
a derived artifact, retaining a mapping to the original member occurrences.

## Artifact inspection and image preparation

Artifact inspection returns every enumerated payload member and its parse outcome.
Broken framing also records that the remaining membership is unknown; it cannot
claim to enumerate members whose boundaries cannot be established. Mixed
or unsupported content contributes explicit coverage gaps. Consumers can request
supported subsets, but results name the selected scope and excluded obligations.

Symbol candidate association and linker selection use different result types.
An archive inventory never decides which duplicate, weak or common definition
the original firmware selected. A request using an ambiguous human name returns
the physical candidates; machine requests select their identities directly.

Image preparation accepts a `LinkPlan`: target/ABI, ordered inputs, entry and
retained callback roots, companion mappings, tool identity, script/options and
declared environment. It returns a `PreparedImage` with bytes, source mappings,
selected definitions, relocations, unresolved boundaries and the executed recipe.
Every consumer of a prepared image shares this operation. Companion-provided data
definitions participate before relocation validation, just as call definitions do.

The implemented [synthetic image profile](../../next/reference/capture-images/README.md#synthetic-prepared-images)
uses an explicitly identified LLD or GNU ld capability and captured relocatable
inputs. Application validates normalized observations against captured occurrence
identities; Linux adapters alone interpret linker-specific text. Existing linked
firmware/ROM bindings are analyzed directly rather than relinked.

An existing linked firmware image preserves its actual placement. A synthetic
analysis link is labeled as such; its addresses and member choices do not prove
firmware placement or original selection. When an external linker cannot provide
an exact source mapping, that mapping remains unknown rather than inferred from
a coincidentally equal name. An operation requiring it remains incomplete.

The host resolves and identifies the selected tool before execution. No adapter
silently substitutes another linker dialect when the selected one is absent.
Identity includes implementation/version and semantic recipe inputs, not just
the executable's filename. Failed tool diagnostics and exit outcome are attached to the
run; successful bounded stderr and exit status are retained in the image manifest. Unresolved relocations are explicit blockers, never valid placeholder data.

## Analysis and verification ports

The implemented [function operation](../../next/reference/analysis/README.md#function-analysis-contract)
consumes an imported occurrence or a symbol in a selected prepared image, with an
explicit/declared extent, producing a retained local graph and separately
qualified coverage. `FunctionSource` and `CodeAddressSpace` distinguish section
offsets from image addresses. Prepared-image identity includes its link recipe;
the image's source revision is frozen even when the current revision differs.
Its ISA port is injected;
the analysis module never selects an external symbol implementation. No pass
scheduler, provider registry or computation cache exists: each operation runs
its declared analysis directly.

Artifacts owns the borrowed `ImageMemory` view over validated static ELF load
segments. The view and its segment capacity cannot outlive the admitted input
buffer. Analysis can read constant bytes through that narrow port; it cannot
load files or request mutable machine memory. Application retains the image
lease while computing and stages results through the same function engine used
for ET_REL. Source mappings retain their precision; a synthetic address is not
an assertion about original placement. The implemented static memory profile
and transfer limitations are documented by Next, without claiming execution
or interprocedural completeness.


Reviewed interpretations such as effect contracts and layout projections are
reviewed through Git outside Blobray and reach a comparison by content. A changed
body or ABI does not inherit an old contract merely because its name or
normalized representation matches.

Verification receives two identified compiled implementations, explicit input
and environment scenarios, a comparison relation and the allowed claim ceiling.
Each executor returns observations, coverage and participation of models.
Verification owns the verdict. Providers, reviewed dispositions and frontends
cannot supply a verdict in place of that comparison.

Concrete and symbolic analysis remain distinct evidence classes. Generated
reference execution, shared-core execution and exact production-entry execution
retain their different claim scopes. `MATCH` requires all obligations of the
declared relation; a proven counterexample is `DIFF`; unavailable required
behavior or coverage is `INCOMPLETE`. A recorded difference is retained even
when other coverage is incomplete. Qualification evaluates eligibility externally.

Execution-session state belongs to the scenario contract. Independent cases
start fresh; stateful phases carry only declared persistent state. Unknown state
after an incomplete phase blocks dependent phases. Models record applicable
preconditions and cannot silently replace the implementation under comparison.

## Durable repository

Store owns transactional metadata and immutable content-addressed objects.
Repository identity and object references are independent of installation paths.
The implementation uses SQLite transactions for metadata and durable immutable
object storage for payloads. Pack layout, compression and indexes are internal
storage choices; no public ID contains a pack offset or database row number.

Store retains every revision, publication, run record and
evidence payload. It has no cache, pruning, compaction or garbage collection,
so nothing retained is ever reclaimed. An active read lease protects the
referenced bytes through publication. Persisted records and transient process
leases are distinct; crash recovery cannot discard a record as a stale lease.

The store owns the full closure of evidence retention: cited records, their
captured inputs, interpretations, producer identities and comparison context.
Re-reading an archived result does not require the old analyzer to execute.
Re-execution additionally requires the recorded tools/providers; their absence
is an explicit limitation, not permission to substitute current versions.

Disk quota failure prevents a new write that cannot complete; it never deletes
retained evidence to make an operation fit.

## Publication and recovery

Publication proceeds in this order:

1. Write staged immutable payloads and verify their content identities.
2. Make payloads and required filesystem metadata durable.
3. Validate the complete result manifest and dependency closure.
4. In one metadata transaction, record the completed run and its references,
   and conditionally advance the selected current publication.
5. Release staging resources. Materialize requested exports from the publication.

The current reference is the commit point. Readers capture it and its revision
consistently, then hold leases to its objects. They never fill a missing published
record from a current export path. A corrupted referenced object is a store
integrity error; nothing recomputes or reinterprets corrupted retained evidence.

Exports are rebuildable projections. Each file is staged and verified before
replacement. A multi-file export is delivered as an immutable bundle with a
manifest; updating a set of arbitrary loose destinations does not claim an
atomic repository transaction. Export failure leaves the durable publication
available and is reported separately from analysis completion.

Recovery distinguishes unreferenced staged objects, incomplete run records,
completed publications and retained evidence. It reconciles abandoned writer
state after establishing that the owning process is gone. It may remove orphaned
staging, mark interrupted runs abandoned and rebuild disposable indexes. It
never fabricates a completed run from loose files or silently deletes corrupt
evidence. Repeating a plan can reuse independently validated completed work.

## Jobs, cancellation and failures

The application supervisor owns the job from registration through cleanup.
Workers have immutable inputs and staged output authority; publication authority
stays with the coordinator. Long CPU loops check cancellation and work budgets.
External tools run in owned process groups or equivalent supported containment.

Durable runs and ephemeral queries use the same supervisor mechanics with
different capabilities. A query owns a private output spool outside the project
and has no project journal or commit phase. Its process outcome, result coverage
and output-delivery outcome are separate facts. Staging output does not imply
permission to modify the selected project.

```text
registered -> running -> validating -> completed
     |           |           |
     +-----------+-----------+-> cancelled, timed-out, resource-limited or failed
interrupted nonterminal run -> abandoned (during recovery)
```

Completed means the operation produced a coherent result, not that it proved
equivalence or closed all research questions. `DIFF` and `INCOMPLETE` are valid
completed verification results. Missing mandatory output records or failed
integrity validation make the run fail, rather than publishing a partial bundle
as complete. Coverage gaps inside valid outputs remain explicit result data.

Cancellation accepted before publication prevents the commit. Once commit has
completed, cancellation returns the completed outcome. Cancellation cannot
interrupt the metadata transaction halfway; its race with commit is resolved
by the coordinator. Timeouts and budget exhaustion report their cause and keep
the previous publication. Diagnostic partial results do not advance that reference.

The coordinator serializes cancellation acceptance with the decision to commit.
Before that decision an accepted cancellation prevents publication; after it the
coordinator reports that cancellation was not accepted and waits for the actual
transaction outcome. Entering commit is not evidence that commit succeeded.
The store atomically records durable completion with the new publication root.
A stale-base conflict retains the validated result with an explicit retention
reference and returns its identity plus the conflicting base/current selections;
it does not advance current or automatically retry publication.

| Lifetime | Completion boundary | Release and exceptional path |
| --- | --- | --- |
| Worker/process tree | Validated terminal report and descendant reaping | Supervisor owns termination even if the frontend or a client handle disappears; unconfirmed termination remains a cleanup obligation |
| Private staging | Outputs validated and transferred to store or caller | Abort drops private fragments; failed cleanup records owned residue for explicit reclamation |
| Durable publication | Payload closure durable, expected base checked, metadata committed | Before commit, previous root survives; after commit, delivery/cleanup errors cannot revoke it |
| Read-query computation | Validated result and worker cleanup | Failure releases spool and returns diagnostics without a project write |
| Output delivery | Caller destination accepted the complete output | Error/cancellation may leave a prefix on a stream; no successful delivery is reported and no implicit retry duplicates bytes |
| Application | Admission closed, owned operations drained | Dropping a run handle does not end ownership; process death transfers durable reconciliation to explicit recovery |

Owned temporary disk output has a declared quota just as memory does. Its
admission and failure paths include manifest fragments, reconstructed archives,
linked images and query spools. A full disk or exceeded quota aborts before
publication without deleting retained inputs or evidence to make room.
Temporary disk policy is local to Application execution; it is not an input to
semantic Plan identity or serialized recipes. Admission reserves an operation's
full allowance; retained results keep their remaining charge through the last
owner, including Plan clones. Failed cleanup preserves the charge and identifies
residue without replacing the primary outcome. Private runtime cleanup requires
known version/ownership, a dead process identity, an exclusive lease and empty
containment; unknown or active entries are preserved. Durable project recovery
remains a separate explicit authority. Next's [temporary storage contract](../../next/reference/resources-storage/README.md#temporary-storage-and-crash-cleanup)
specifies implemented defaults, control reserve, accounting and reconciliation
bounds, including materialized link inputs and prepared images.

Application shutdown stops admission, cancels owned jobs, terminates/reaps
remaining children within the configured shutdown grace, and releases leases.
Dropping a client handle does not detach unmanaged computation. Closing an owned
CLI application closes its supervisor; an embedded client may keep its own
application alive explicitly. An uncooperative in-process workload must run in
an owned worker process if it cannot satisfy bounded shutdown.

The first supported resource-limited host is Linux. The platform adapter reports
whether memory limits are kernel-enforced or sampled. A host without the required
containment cannot claim the same resource contract. Libraries do not install
signal handlers; host composition maps signals to application cancellation.

Errors distinguish invalid request, missing input, ambiguity, unsupported
operation, stale base, integrity failure, unavailable dependency and I/O failure.
Run outcomes separately distinguish completed, cancelled, timed out,
resource-limited, failed and abandoned attempts. Frontends preserve these typed
distinctions and the selected revision; prose errors are not machine protocols.

## Resource ownership and bounded computation

Every module obeys these contracts. The [Next references](../../next/README.md)
state the bounds each operation enforces; the existence of a supervisor does
not establish bounded working memory inside an algorithm.

| Resource | Owner and contract |
| --- | --- |
| Run admission, aggregate work and deadline | Application; one budget across workers and coordinator validation, never reset at a phase boundary |
| Working memory | Computing module borrows an explicit allocation capability; input-dependent capacity exhaustion returns a typed failure |
| Input buffers and mappings | Store provides stable captured bytes and owns leases; original mutable files are not parser memory |
| Scratch | Scoped owner releases temporary storage on success and error; references and IDs cannot escape or remain usable after reset |
| Cross-phase tables | Computing module retains only data needed by subsequent phases, with checked IDs and explicit lifetime |
| Staged output | Store owns unpublished bytes; stream records when full materialization is unnecessary |
| Clock, signals, process memory and descendants | Host; application receives narrow capabilities, libraries install no global signal handlers |
| Publication | Coordinator alone; resource failure leaves the previous publication intact |

Working-memory capacity, input residency, work units, elapsed time and emergency
process-memory limits are distinct quantities. Arena occupancy is not RSS; a
successful mapping does not reserve physical RAM. Runtime, stacks, libraries and
mapped inputs require space outside algorithm allocations. Missing measurements
are unknown, not zero; each memory measurement identifies its source.

Input-controlled recursion is prohibited. Explicit stacks and queues are bounded;
cyclic traversals declare repeated-visit and termination rules. Nested loops,
string scans, hashing and serialization participate in work accounting and
cooperative cancellation. Checkpoints must bound the work between observations,
not merely count outer-loop iterations. Opaque library calls remain under process
supervision and must not be described as cooperatively interruptible.

A resource failure never silently truncates inventory or converts unfinished work
into a completed publication. Internal indices are distinct from durable artifact
and occurrence identities. Persistent computation storage means a run lifetime,
not durable repository storage. File handles and locks have explicit destructors;
resetting an arena cannot discard their release obligations.

Next implements these boundaries with explicit capacity reservations and owned
scratch buffers. Its streaming ports are `ByteSource`, `ElfSink` and
`InventorySink`; the consumer owns any retained copies. Store validates manifests
without materializing a revision and never uses an origin as a read fallback.
Read-only workers use `Application::start_query` in the same owned job set as
imports. Application owns `OperationHost` and the private execution protocol;
store owns persisted run records. Query output stays outside the project, with
no project journal or writer. `ReadView` and `InventoryView` expose read authority
without store project handles. Selected current becomes an explicit revision
at admission. Result transfer and delivery are each allowed once; repeated wait
retains the terminal outcome. A successful query computation is distinct from
successful delivery to an output destination.
The [implemented memory boundary](../../next/reference/resources-storage/README.md#current-memory-boundary)
defines admission costs, defaults, single-object limits and convenience APIs.

A two-ended arena is an allowed implementation, not a universal storage mandate.
Scoped scratch must prevent use after reset, including error paths. Container
growth, alignment, allocator metadata and unreclaimed replaced buffers count
against its capacity. A custom global allocator is not the ownership contract;
`no_std` alone does not establish absence of allocations through dependencies.
Strict allocation-free computation boundaries are independently checked where
implemented, without requiring the CLI, database or wire types to be `no_std`.

### Allocation and retention obligations

| Memory class | Authority and lifetime | Required accounting |
| --- | --- | --- |
| Request/control state | Application admission through terminal cleanup | Bound decoded requests, job/event queues and outstanding results; admission itself cannot allocate an input-sized graph before checking limits |
| Persistent computation | One operation's capacity authority | Retained nodes, edges, strings and memo entries consume capacity until released or transferred with their owner |
| Phase scratch | Borrowed scope inside that authority | Charge simultaneous buffers, alignment and replacement overlap; reset/drop on success, error and unwind; no escaping references or stale reusable IDs |
| Captured input views | Store lease borrowed by parser/executor | Buffered copies count against working capacity; mapped residency and runtime overhead remain distinct process measurements |
| Stream consumer state | Consumer, under the same operation or an explicitly transferred result budget | Borrowed callbacks retain nothing by default; copying, indexing, formatting and queues require capacity before growth |
| Opaque dependency workspace | Declaring adapter under host containment | State supported size/admission bounds and checkpoint limits; do not claim exact accounting or cooperative cancellation without evidence |
| Diagnostics and OS/runtime state | Host outside exhausted computation capacity | Bounded emergency reporting and process containment; unknown observations remain unknown |

Nested phases share remaining work and capacity. Child workers receive explicit
shares; they cannot each restart the parent's full budget. A returned in-memory
result retains its capacity owner until dropped, or is moved into admitted
caller storage; ending a run must not release accounting while its data survives.
Large durable results are streamed into store-owned staging rather than retained
as a run-long graph solely for serialization.

Reservation accounting establishes admission of declared allocations, not a
proof that every heap allocation was charged. Exact arena occupancy, admitted
capacity and process memory are separate guarantees. A module may claim strict
allocation control only when its containers, dependencies, formatting and error
paths obey that boundary. Allocator replacement alone cannot establish it.
The emergency error payload is fixed-size; human formatting occurs in bounded
host diagnostics, independently of the exhausted computation capacity.

Container/decoder limits state their units and scope: member count, nesting,
expanded bytes, worklist entries, graph nodes/edges and total work are not
interchangeable. A supported nested format uses iterative jobs and a shared
expanded-byte budget. Unsupported compression or nested containers remain
explicit coverage gaps; this design does not imply ZIP/TAR support in Next.

Import, snapshot queries and doctor are bounded operations. Read-only operations retain their no-write/no-repair contract.
One user command owns supervision; no separately invoked limiter is required.
Uncooperative loops and process crashes still require an internal worker boundary.
Deadlines cannot guarantee hard real-time teardown of uninterruptible kernel I/O.

### Failure diagnostics

The run retains a primary cause, last known phase and input/member/table/entry
position, charged work, elapsed time, and available memory observations. A rejected
allocation reports the rejected request size, not an invented estimate of memory
needed to finish. Diagnostic emission must not depend on allocating from the
exhausted computation arena. Progress is bounded and coalesced; a last observed
checkpoint is not evidence of the exact crash location.

Exit status, signal and observed cgroup OOM evidence remain distinct facts.
SIGKILL alone does not prove OOM. Cleanup, persistence and diagnostic-channel
failures are secondary diagnostics and never overwrite the original cause or
revoke a committed success. Recovery retains the last valid checkpoint before
removing owned staging. An absent or invalid checkpoint is explicit, not fabricated.

## Versions and compatibility

Repository storage, public interchange records, producer semantics and provider
implementations have separate version identities. A storage migration cannot
relabel old evidence as output from a new analyzer. Unknown required fields or
unsupported versions produce explicit incompatibility; they do not select an
older interpretation automatically.

Public exports carry schema/version and the identities needed to interpret them
without the generating CLI. Downstream consumers use those contracts or a
snapshot reader, never private SQLite tables or pack offsets.

### Local value analysis boundary

The implemented [value and memory-effect contract](../../next/reference/analysis/README.md#values-and-memory-effects)
extends local function analysis through an injected semantic port. The ISA owner
lifts operations, analysis owns fixed-point state, application owns execution
and store owns retained bytes. Unknown values, incomplete effects and failed
resource admission are distinct outcomes. No hardware knowledge or executable
memory environment is acquired implicitly.

### Implemented preservation boundary

[Blobray Next preservation](../../next/reference/preservation/README.md#backup-and-restore)
owns the implemented wire contracts. Storage metadata and journal format support
follow the result-assessment contract above. Backup/restore preserve source,
analysis and publication identities. Register publication remains
with the independent register tool.

### Implemented concrete execution boundary

Domain owns `ExecutionRequest`, observations and the injected
`Executor`/`ExecutionMemory` ports. Artifacts lends validated static ELF segments;
application owns the loaded executables, mutable session buffers, explicit device
state and all phase transitions. The RV32 backend owns concrete register/PC state;
verification alone computes the ordered-observation verdict and, separately,
validates the record structure of every run. None of those modules selects live
paths or opens a project.

The invocation owns a bounded vector of known/unknown physical RV32 ABI words.
Domain validates stack geometry and computes entry SP; application initializes
stack arguments in its freshly owned stack, and passes eight optional register
words to the backend. Unknown argument slots override stack seeds. No register or
stack word becomes zero by omission, and setup emits no guest events. The caller
owns type/variadic lowering. The execution request and manifest format pins this interpretation;
producer identity pins both backend unknown-value semantics and stack environment.

The `byte-addressed-memory-1` environment admits ordinary unaligned 1/2/4-byte
RAM or ELF-backed data accesses entirely inside one mapping. Reads require all
bytes known/readable; stores validate the complete writable range before mutation.
No access stitches adjacent mappings together or wraps RV32. Timeline records keep
the original address and width; domain and record validation permit these ordinary
transactions while continuing to require aligned atomics. Misaligned MMIO never
consumes a response or splits into smaller operations. Instruction fetch alignment
is unchanged. This policy is part of `EXECUTION_ENVIRONMENT`, which keys reused
vendor results, so none comes from an aligned-only environment. It makes no hardware,
timing or concurrent-atomicity assertion. Regression ownership is
`execution/unaligned.rs` and `verification::records::timeline`.

`ExecutionMemory` also owns LR/SC reservation state, permission checks and indivisible
word updates. The backend supplies pure AMO arithmetic through a borrowed callback;
application invokes it once after admission/access checks, with no fallible step
between update and commit. No default read/write emulation is provided by the port.
The selected environment has one hart and no concurrent device writers, honors
program order, invalidates overlapping reservations and clears them between phases.
Atomic MMIO is unsupported and cannot invoke the register-bank model by fallback.
Atomic effects are ordinary captured guest behavior, not an expanded comparison
relation or a proof about multi-hart ordering.

Each case owns an explicit cold/warm transition and each invocation selects its
own captured-address-space entry. The first case is cold. Implementations have
separate mutable state: writable ELF and session RAM survive warm transitions;
phase RAM, stack and reservations do not. Models retain their own phase/session lifetime. RAM lifetime is part of its mapping
identity and cannot change while that mapping is live. Phase buffers release after
comparison/staged serialization. A warm successor to incomplete execution is blocked;
a cold case discards prior state/dependencies and starts a fresh chain without
erasing earlier incompleteness. All chains share one control and memory authority
within one request.
Physical execution goals belong to each invocation. Application validates that
symbols belong to the side's executables; artifacts resolves exact FUNC/NOTYPE
table entries against executable file-backed mappings. Goals resolve before any
session exists, and each executable's content identity is computed at most once
per request. Backend receives only a resolved goal and initial
register/stack state; it never resolves symbols or retains ELF metadata.

Returned, reached-symbol and observed-call are distinct completed outcomes. An early
return before a non-return goal is incomplete; a completed early goal permits a warm
successor but never promises the callee body ran. `complete` means all declared goals
and model closure obligations were met; it does not require every phase to return. Verification compares only the declared
observable prefixes; non-return goals cannot compare return registers. Record validation checks
outcome/goal kind and cold/warm dependency blocking without owning ISA interpretation.

A known difference survives other incomplete cases; a resource failure returns
no records. The implemented relation compares exact ordered
MMIO/fence/delay events and optionally one 32-bit return, with a caller-declared compiled
binding ceiling. It cannot claim arbitrary-domain or hardware equivalence. See
[concrete execution](../../next/reference/execution/README.md#concrete-execution-and-comparison) for
request limits, memory initialization, schemas and unsupported behavior.


### Captured data interpretations

`DataRequest` binds an `Occurrence`, explicit ranges and retained analysis
IDs. Artifacts owns the verified ELF buffer and prepared section metadata; each
`DataView` borrows them for one callback. Application resolves the occurrence
once, owns the query budget and streams observations. Store neither parses ELF
nor interprets a table. Query delivery owns its private exported files until
transfer or drop, and publishes nothing as a side effect.

Data exports retain the captured object, selected bytes and analysis coverage.
Original paths are
provenance only. Image VMAs are checked against file-backed load mappings;
section-relative and file-relative offsets are separate fields. Mutable section
bytes are initialization only. Relocations are retained, never implicitly applied.
The manifest retains total, overlapping and unknown-extent counts. Unknown transforms
are never excluded merely by their offset, and invalid known extents are rejected. Successful export makes no general completeness or comparison claim.

### Physical symbol selection

Prepared function/data views select SHT_SYMTAB or SHT_DYNSYM by physical table
kind, section and entry index; conventional section names are not identities.
One table of each kind is supported. Missing, duplicate or mismatched tables and
invalid indices fail before publishing an analysis.
Dynamic symbols in static RV32 ET_REL/ET_EXEC remain ordinary captured metadata;
they grant no dynamic-loading or TLS support. Static relocations keep their own
`sh_link` target table independently of the selected function/data symbol. A
relocation referencing another table is explicitly outside the current profile.

Whole-object enumeration preserves both static and dynamic STT_FUNC occurrences,
including aliases, as separate physical requests and results. Coverage unions
selected byte intervals without turning duplicate symbols into additional bytes.
Function selection policy 8 and investigation policy 4 record this interpretation;
reading does not convert older policies. Regression coverage lives in Next
`functions` tests `dynamic_occurrences_keep_physical_indices_through_analysis_and_reopen`,
`dynamic_function_selection_keeps_static_relocation_target_identity` and
`dynamic_and_static_function_aliases_remain_distinct_in_saved_publication`.

### Explicit executable ranges

`FunctionSelector` distinguishes physical symbols from `{object, section, extent}`
code ranges. The latter require no symbol table and reject a separate symbol-size
override. Artifacts owns section/type/alignment/bounds/backing validation; the
application's shared occurrence acquisition owns source/image/revision identity.
No range is inferred from neighbors, padding or disassembly. Prepared views keep
the existing callback lifetime and admitted memory owner.

Explicit ranges enter investigations through `InvestigationRequest.ranges`;
unused or duplicate selections fail planning, invalid bytes produce blocked function outcomes. Store
validates selector/section/extent against the admitted request before publishing
and when reopening retained results. Coverage joins explicit ranges to section
metadata and unions them with symbol extents, leaving other bytes unclassified.

Function and investigation records carry these identities under the
[current formats](../../next/reference/interfaces-formats/README.md#current-formats). Old formats are rejected without
mutation or conversion. `functions::ranges` regressions cover invalid ranges and
explicit range planning;
`explicit_image_code_range_keeps_vma_identity_and_unions_symbol_coverage` covers
prepared-image ranges and virtual addresses.

### Captured pointer-table observations

`PointerTable` specifies count/stride for captured little-endian RV32 four-byte
slots. `DataRequest.pointer_table` is an explicit observation request over one
range; export shares physical occurrence and exact byte-evidence validation.

Artifacts supplies borrowed bytes, sorted physical relocations and structural
write bounds. `analysis::pointers` streams individual slots with work accounting;
`PointerDecoder` supplies backend-owned relocation semantics. The RISC-V profile
is `rv32-absolute-rela/1`; unavailable profiles fail explicitly. No new owner holds
a table-sized result vector. Lookup is logarithmic plus a bounded candidate-write window. Transient owned
record metadata uses the admitted output envelope; no table-sized value array is
retained.

Pointer values distinguish null, numeric address, defined symbol plus addend,
external symbol plus addend and a typed unresolved transformation. Linked words
must agree with known R_RISCV_32 results; captured bytes are never patched. Address
values establish neither executable mappings nor function/ABI boundaries.
Data manifest schema 4 includes producer identity and classification counters;
resource failure aborts the query, never truncates a successful table.

Next pointer regressions cover null, address, defined and external slots,
overlapping and partial writes and unknown/wider relocations.

## Finite value alternatives

The current RV32 values profile and function policy 8 preserve at most eight
canonical exact alternatives at a register join. Domain owns nonrecursive leaves
and validates 2..=8 sorted distinct entries when decoding saved values. Analysis
owns the finite lattice and admitted, operation-local set/index storage. Values
and expression outputs borrow no set owner after analysis. No global cache or
additional allocator authority exists.

Joins retain a may-set; arithmetic applies to each bounded operand pair and
immutable image loads require all candidate reads to be modeled. Unknown inputs
absorb exact information. Distinct expression IDs and incomplete relocation
uppers are not exact alternative leaves. Overflow emits `SemanticGap::AlternativeLimit`
and unknown, never a truncated set or a chosen target. Work/cancellation admission
includes set lookup/growth and each candidate evaluation. Cycles converge under
the finite-height lattice; resource exhaustion creates no published partial result.

Read queries match each possible address/symbol without changing or recomputing
the saved facts. Call filters include ambiguous finite targets as unresolved and
never select one of them. CFG edges and original
instruction/relocation records remain provenance; sets do not encode path
correlation, prove reachability or expand indirect control flow.

Contract regressions: analysis `value_sets::tests` and
`values::tests::joins_and_loops_converge_independently_of_visit_order`; Next
`persisted_alternatives_are_flat_bounded_and_canonical`.

## Register research and source publication

Register discovery reads an explicit saved analysis/publication scope.
Candidate addresses, instruction access widths and expression masks are
observations, not physical register/field declarations. Unknown addresses,
alternatives, partial analyses and unavailable members remain visible. No
hardware meaning is inferred from a coincident address; the reviewed register
model owns register identity.

The independent register owner initializes editable source models from explicit
peripheral geometry and imports CMSIS-SVD into the same native source format.
Neither operation accepts hardware claims. Imported XML is retained verbatim;
unsupported XML extensions are not promoted to native semantics. Reviewed source
assertions, applicability, evidence and publication policy remain explicit inputs
to validation and the existing four-output publisher. Blobray does not acquire
register-generation or production dependencies.
## Saved semantic IR packaging

The native semantic representation remains the saved `FunctionRecord` stream and
its `FunctionManifest`. An IR build selects frozen publications/analyses and named
profiles; it does not reacquire origins, decode code or choose an alternative
engine. All, name-prefix and exact-analysis roots are explicit. Prefix selection
uses physical names retained in selected publication membership; missing name
metadata for a named function is an error. Symbol-less ranges have no inferred name.

Call closure uses the same application navigation resolver. Only one unambiguous
selected physical target can extend a profile. Unknown/outside/ambiguous links and
partial function coverage remain separate evidence. Cycles are finite graph edges,
not input-controlled host recursion. Packaging does not certify an executable path.

One operation owns admitted profile/graph indexes, temporary index output and a
staged immutable manifest. Original function streams remain immutable. The normal
supervisor validates/promotes the closure and publishes the result with the run;
capacity, cancellation or validation failure cannot publish an incomplete bundle.
Trace extraction reads original facts with exact analysis and record identities
and is responsible for its explicit exactness claim.

## Static trace relation

The implemented static trace profile is defined in the
[operator reference](../../next/reference/ir-traces/README.md#static-observable-traces). Domain owns the
request, observable values, blockers and result schema. The RV32 semantic producer
emits typed fences, pre-transfer call/tail inputs and separate architectural
register writes. Trace policy 3 applies the saved link write before callee entry;
unknown section-relative links never inherit the caller's old register value. Analysis owns per-function borrowed
indexes, canonical symbolic expressions and iterative invocation/path traversal;
application acquires the explicitly selected saved profile and releases one side's
facts before loading the other. Only compact observable events and the shared admitted
expression index survive between sides. Store remains unaware of trace semantics.

A structural unknown indirect edge can be closed by its unique saved physical
callee in the selected IR profile. This does not repair decoding/reference gaps,
conflicting boundaries, missing flow or unsupported semantics; those still block
exactness. Original function coverage is not upgraded.

A trace is conditional on explicit inputs, original immutable-image assumptions and
ordinary integer ABI call/return behavior. Unknowns are blockers, not zeroes; all incomplete paths prevent
MATCH. Canonical symbolic equality can prove the selected relation, while undecidable
symbolic inequality remains INCOMPLETE unless a later observed event proves a
difference. A proven prefix mismatch yields DIFF even with incomplete paths,
preserving their blockers and exactness flags. An unequal length proves DIFF only
when the shorter side is exact. Return rows/call sites are provenance and are
excluded from the physical MMIO/fence relation. A successful query denotes delivery,
not exactness or termination proof. The same frozen IR and request reproduce the
result after source-free backup/restore. Regression owners are Next
`functions::trace` and the static trace execution tests.


### Implemented device ownership and completion

Concrete platform scenarios may supply captured guest setup code and register-input
shims through ordinary execution targets. Their bytes and input words are retained
dependencies, not implicit engine initialization. A warm phase can consume setup
writes to captured writable ELF memory; a RAM seed cannot overlap or replace that
mapping. Unknown register copies and spills propagate as unknown values, even
into filled stack memory, and stop execution only where they decide control flow,
form an address or reach a device. The explicit-input regression in
[session tests](../../next/tests/execution/sessions.rs) checks both paths. The
[PHY I2C scenario](../../../../verification/esp32s31/README.md#captured-i2c-command-memory-comparison)
records this narrower input/observation scope and uses shipping HAL/PAC behavior.
Chip-specific setup, addresses and expectations stay with the verification owner.

`DeviceDeclaration` is a caller assumption with explicit applicability, stable
content identity and phase/session lifetime. Application `devices` owns configuration
clones, admitted mutable banks, transcript cursors and an exact sorted port index.
The ISA backend sees only memory ports; it neither resolves models nor invents a
response. Atomic accesses never fall back to device behavior. Model geometry cannot
claim a mapped byte or the gaps between indexed ports.

Each phase snapshots cumulative participation separately from its code outcome.
Phase instances close there; session instances close at the last phase of a chain.
Warm continuation omits live declarations; cold reset or expired phase ownership
permits fresh ones. Closure releases owned configuration and mutable data, while
bounded index capacity may remain for reuse. All allocation, traversal and access
work shares the existing operation budget, with no global cache or new runtime.

`ModelObservation` distinguishes open, complete and incomplete obligations. Required
Sequence reads use bounded value/count runs with a checked logical total;
application retains a cursor and remaining count without expansion. Record validation checks
logical consumption, and identity binds the encoded runs. Sequence/FIFO values
must be consumed at closure; failed accesses preserve an issue.
A returned goal cannot override incomplete environment evidence. Verification
requires both code and due model obligations for MATCH. Record validation checks identity,
monotonic counts, declared transcript totals, exact closure and required observation
presence; it rejects MATCH with unmet obligations without executing model semantics.
Models and code observations are recorded together. Full current
mechanism syntax and claim limits belong to the [operator reference](../../next/reference/execution/README.md#concrete-execution-and-comparison).

Packed-command banks follow the same owner. Domain defines bounded wire geometry,
seeded cells, script inputs and command accounting; application `command_bank`
owns admitted per-port pending commands and shared mutable cells. `devices` owns
installation, exact port lookup, access work and closure. Only a ready read commits
a staged write. Reset can explicitly abort one port's pending command, preserving
shared cells and script cursors. No chip encoding, polling loop or RF algorithm is
inside the generic mechanism. A model with pending commands cannot close complete.

Record validation checks command/sample conservation and phase deltas against
the declarations. Its accounting checks cannot claim the ISA or device responses
were re-executed.
The [native command tests](../../next/tests/execution/command_bank.rs) cover
warm completion, incomplete closure, gaps and capacity; domain, application and
record-validation tests cover geometry, independent shared-bank values,
reset, sample consumption, cancelled admission and forged evidence counters.


### Implemented external-call boundary

`CallDeclaration` owns exact target/boundary, caller applicability, physical argument
width, immutable response sequence and phase/session lifetime. Application
`external_calls` owns admitted copies and cursors; `execution_calls` validates full
responses and mutates only session-owned normal memory. Device and call models have
separate explicit ports and ID namespaces, and share completion/closure semantics.
No global response registry participates.

The backend invokes the required `ExecutionMemory::call` port at eligible transfers,
after checking observe-call goals. An unselected transfer executes captured code;
a selected response has explicit returns, output/allocation/delay effects and ABI
clobbers. Root entries and canonical returns cannot be modeled accidentally. Model
responses never constitute evidence that replaced code executed. All participating
assumptions remain in the immutable request and model identities.

Output geometry/ownership, allocation freshness and required argument values are
validated before response mutation. Resource failure during admitted effects fails
the request. Allocation capacity is owned until its declared lifetime
ends; only the requested zero-initialized prefix is accessible. Model responses and
memory allocations have independently declared lifetimes. Warm continuation cannot
redeclare a live model or reseed live allocation storage through ordinary RAM.

Fixed-size streamed events retain modeled boundary, arguments, effects and returns.
Event capacity is admitted for the full successful response before mutation. Record
validation checks selected bindings, ordered arguments/effects/returns, exact response consumption
and closure; missing/forged evidence cannot complete the run. The explicit relation
selects MMIO/fence/delay, return words, final memory, physical/reviewed calls and
internal timeline channels; excluded model records remain available for inspection. Delay values are assumptions, never wall-time or
hardware timing evidence.

## Selected final memory and comparison relations

Every comparison case owns a `ComparisonRelation`; single-implementation cases
use none. It selects independent low/high return words, ordered MMIO read/write,
fence and delay channels, and physical `MemoryPair` indices. The relation must
select at least one domain. Return words require Return goals on both sides. No
other phase's relation or old request flag supplies a default. The request retains
each selection.

`Invocation.observe_memory` names exact nonempty disjoint normal-memory ranges.
A pair selects one range per side with equal byte length. Names and pair indices
are explicit; matching names/addresses do not infer corresponding layouts. There
are at most 128 selections and 1 MiB selected bytes per invocation. Bounds are
validated before worker execution; selections never truncate silently. Observations
may be retained without being selected for comparison.

Application snapshots every requested byte at the phase stop, before releasing
phase RAM, allocations, tables or stack. It accesses readable normal-memory views
without invoking MMIO/device callbacks. Availability distinguishes readable mapped
bytes from inaccessible/unmapped bytes; knownness distinguishes initialized values
from unknown contents. Selected unchanged bytes are included. Canonical fixed-size
16-byte chunks identify selection/offset/length and both masks; masked-out bytes
are stored as zero only as an encoding rule, never as an observed value. Blocked
phases execute and capture nothing, with explicit blocked outcome.

Session admits the snapshot vector before allocation and owns its reservation
through evidence serialization and comparison. Recycling drops the snapshot before
releasing capacity; a following phase cannot accumulate previous snapshots. These
records are separate from guest events and do not consume `max_events`; their
independent byte bound and shared memory/work/disk budgets still apply. Capture
failure fails the whole request. No extra memory cache or lifecycle owner exists.

Record validation checks chunk order, exact range coverage, canonical masks/bytes, phase/side
ordering, blocked absence and selected-knownness before accepting a comparison
MATCH. Missing or forged chunks are integrity failures. A difference descriptor
must name a selected event/return/memory domain. Record validation checks structure;
verification remains the sole owner of comparison computation.

Verification compares selected event prefixes and known returned words, then paired
memory bytes using bounded selection lookups. Known selected differences dominate
other unknowns. Memory snapshots from unfinished code goals cannot establish a
DIFF in completed final states; they remain retained intermediate observations.
When both code goals completed, any known differing selected byte proves DIFF even
if another selected byte is unknown. Selected unknown/unavailable bytes prevent
MATCH. Full goal/model completion plus equality of every selected observation is
required for MATCH; excluded observations cannot silently enter the relation.

`ExecutionObservation::completed` and execution-scenario completeness still mean
code goals and due environment obligations were met. They do not claim that all
retained memory/returns are known. Unknown selected outputs can therefore yield an
INCOMPLETE comparison beside completed execution coverage. Comparison verdict,
operation state and scoped execution coverage remain independent machine fields.
A failed phase blocks warm execution; an unknown selected output or comparison
DIFF alone does not change the session's successful code-goal transition.

### Physical call observations

`Invocation.observe_calls` explicitly selects transfer capture for this phase.
`null` disables it; a profile declares `include_tail`, `argument_words` (0–256)
and at most 128 unique aligned physical target overrides. Zero words means
target-only observation. This is physical RV32 word selection, with no inferred
signature, variadic lowering or pointer equivalence.

The backend invokes the observation port on x1/x5 transfers and optionally x0
noncanonical transfers. Canonical x1/x5 returns and the root return sentinel are
excluded. Opt-in x0 transfers are tail candidates: this convention also includes
intrafunction jumps and does not prove function boundaries. Capture precedes
observe-call goal completion, model dispatch and callee
code. It never invokes the destination. A `call-transfer` header retains physical
site/target, direct/indirect form, tail flag, current SP, word count and selected
target kind (captured code, call model or unavailable). Target kind
describes the available boundary, not proof that its body executed; the request,
model identities and adjacent dispatch evidence retain that distinction.

The following ordered `transfer-argument` records contain a0–a7 then words from
the current private stack. Unknown registers/bytes remain unknown. Missing SP,
misalignment and outside-private-stack accesses remain unavailable with a reason.
Capture never redirects a stack read to RAM or MMIO, and unknown captured data
alone does not stop code. Each group admits header plus all words against shared
event capacity before emission. Application owns the admitted sorted override
index for one phase and releases it at phase completion; retained event capacity
remains charged through serialization/comparison. Work, memory, deadline or disk
failure fails the entire request.

`ComparisonRelation.calls` selects exact ordered physical targets and configured
words, interleaved with the selected MMIO/fence/delay channels. Both invocations
must select the identical capture profile. Sites, SP, transfer form and target
kind remain provenance rather than implicit equivalence conditions. An event
index counts selected calls/effects, not individual argument rows. Different
targets or known selected words yield typed `call-target`/`call-argument`
differences; different call/effect order or completed stream lengths yields
`event`. Unknown/unavailable selected words prevent MATCH; known differences
remain DIFF despite other unknowns. Completed code/model obligations remain
required. Excluding calls does not erase their evidence.

Record validation checks requested geometry, contiguous word groups, unavailable reasons,
observe-call boundary presence and selected knownness before admitting MATCH.
It does not replay ISA semantics to authenticate the physical trace. Verification
borrows grouped slices without allocating or rescanning preceding calls. The
[capture regressions](../../next/tests/execution/capture.rs) exercise application,
CLI, persistence, word/effect differences and resource atomicity;
[record validation](../../crates/verification/src/records/capture.rs) rejects malformed
groups. Layout projections require their explicit reviewed relation below; this
physical profile infers none.

### Internal timeline

The invocation explicitly selects normal-memory reads/writes, atomics and
conditional branches for capture; the relation independently selects those
channels for comparison. Selected channels must be captured by both sides.
Application owns admitted event storage and physical normal-memory ownership;
the backend supplies the current instruction identity and branch decision.
Instruction fetch, argument inspection and setup initialization do not become
guest data transactions. Declared call-model memory effects retain their existing
records and enter selected normal-memory channels once. Memory site/origin remain provenance; physical address, width,
values and atomic outcome/order are comparison inputs. Conditional branches retain
physical site, target, fallthrough and taken decision. No cross-layout or branch
correspondence is inferred by this profile.

Unknown or inaccessible owned reads stay explicit. Failed instructions/accesses
also retain their execution gap and cannot establish completed equality. Successful
stores, LR/SC/RMW and declared model effects are ordered with selected calls/MMIO/
fence/delay. Setup/inspection cannot add extra guest effects. A known prefix
difference can establish DIFF; equal final memory cannot erase a different selected
timeline. Capture failure fails the entire request. Reviewed projections use
the separate explicit contract below.

`Invocation.observe_timeline` contains explicit `reads`, `writes`, `atomics` and
`branches` booleans. `ComparisonRelation.events.timeline` independently selects
these channels and must be a subset of each side's capture. Raw memory events
carry instruction PC plus a fixed-size typed transaction. Reads retain known,
unknown or unavailable values. Successful normal stores retain truncated values
at widths 1/2/4. LR records its old word; SC records operand and success/failure;
RMW records old/new words. Atomic ordering bits are explicit program-order inputs,
not a multi-hart timing guarantee. Invalid/unowned accesses stop with a gap rather
than inventing a normal-memory owner or using an implicit device fallback.

`observe_timeline.written` is not an event channel: it reports the persistent
bytes a phase stores into writable image segments, session RAM and session
allocations as ascending, coalesced `WrittenRange` records after the phase's
final memory, without values or order. Stack and phase-lifetime memory are
excluded. A phase holds at most `MAX_WRITTEN_RANGES` ranges; a store
that would add one more stops the run as resource-limited. Record validation
requires the capture flag, record order and ascending coalesced ranges. Clients
use the ranges to find written state that no relation compares.

The executor's `instruction` port identifies memory sites independently of shared
progress. A phase clears this identity and capture flags. Conditional branch
records include site, target, fallthrough and taken decision for both ordinary and
compressed instructions. Fetch, setup seeds, argument inspection, final snapshots
never duplicate guest transactions. Existing `CallOutput` writes enter the
selected normal-memory relation once via their original records, retaining model
provenance.
Dynamic `Allocation` records enter writes as one `initialize-zeroed` span for
the nonempty requested prefix, never the unused capacity. A zero-length request
retains allocation evidence but has no memory transaction. Bulk initialization is
an explicit transaction, not implicitly coalesced with individual stores.

Event storage remains admitted once per session and recycled per phase. Memory
writes/atomics check event capacity before mutation; RMW admission precedes its
pure update callback. Failures during subsequent evidence handling fail the
request. Record validation checks capture authorization, instruction/branch geometry,
transaction widths/values and selected-knownness; it does not reconstruct ISA
execution from the transcript. A claimed completed outcome plus an unknown selected
read still cannot admit MATCH. Verification borrows transactions and compares
normal-memory values without including source PC/origin as an implicit equality
condition. Branch comparison is exact physical control comparison; differing code
addresses require an explicitly selected reviewed projection.

[Timeline regressions](../../next/tests/execution/timeline.rs) cover all widths,
AMO/orderings and LR/SC, ordinary/compressed branches, intermediate differences
with equal final RAM, call-model effects, phase capture, unknown/unreadable
memory and capacity.
[Record validation](../../crates/verification/src/records/timeline.rs) rejects malformed
or unrequested transcripts and invented MATCH; application verifies capacity before
atomic callback/mutation. No elapsed-time or real hardware equivalence is implied.


### Reviewed layout projections

`LayoutProjection` owns two exact captured code entry occurrences,
explicit normal-memory address domains, paired byte fields and conditional branch
coordinates, with applicability and reason. Entry boundaries must be physical code
symbols in retained linked executables. Domain/field declarations are reviewed
runtime layout assumptions, not claims that the ELF contains initialized data there.
The session's normal-memory permissions and byte knownness remain authoritative.
The selected execution source/revision and entry must match each endpoint; changing
an entry or source cannot silently reuse a projection.

At most 128 fields, domains per side and branches are accepted. Domains are bounded,
nonempty and nonoverlapping, and every domain has a declared field. Field names are
unique labels, never resolvers. Each field declares a domain index and offset per
side, equal element width 1/2/4/8, a nonzero `count`, and `final_state`/`timeline`
participation. Checked width × count defines the field/array span; all projected
spans together are limited to 1 MiB. Both aligned field
ranges must fit their domains; overlapping aliases or unused fields are rejected.
All declared final fields require captured snapshots containing every byte and
cannot overlap a physical whole-selection comparison. Unknown padding outside the
selected fields stays in evidence without claiming it is equal.

Branches declare site, target and fallthrough on each side. Duplicate branch sites
and invalid geometry fail validation; the comparison maps only branch records at
exactly the declared coordinates and does not decode the instructions. Projected
branches compare the paired identity and the same taken decision; predicate inversion
or different dynamic path shapes are not inferred.

The case selects one projection in `ComparisonRelation.projection` by the digest
of its canonical encoding. The projection is reviewed outside Blobray and supplied
with an in-process comparison, which validates its applicability to the request.
All field/branch scopes declared by that projection participate; a final-only
profile is explicit.

Verification borrows the selected contract and observations. Final fields compare
all selected known bytes at completed goals. Timeline memory is mapped to field and
within-field offset, retaining transaction kind, width, values and atomic order/outcome.
An effect must fit wholly inside one selected field/array; bulk allocation initialization
is not implicitly split or coalesced. Pointer values are compared exactly, never
rebased because their storage address moved. Branch/memory observations stay in the
same ordered stream as selected calls/MMIO/fence/delay. An unmapped selected memory
or branch observation makes equality unknown rather than disappearing or falling
back to physical equality. Known mapped differences remain DIFF; unknown bytes or
unfinished code/model obligations cannot MATCH.

The verifier traverses array snapshots with advancing borrowed byte cursors, without
rescanning their chunks per byte. Raw physical evidence and excluded bytes remain intact. This finite profile claims
selected observation equality under reviewed assumptions, not universal ABI, type,
algorithm or hardware equivalence. Arbitrary width conversion, pointer-value mapping
and path normalization are outside this profile and are never approximated.

[Projection scenarios](../../next/tests/execution/projections.rs) cover content
selection, a missing projection, invalid geometry, final fields,
unknown padding/fields and missing capture. [Verifier tests](../../crates/verification/src/projection.rs)
check ordered mapped memory/control and unknowns.

### Reviewed effect contracts

`EffectContract` binds exact captured vendor/replacement code entries. It is
reviewed outside Blobray and supplied with an in-process comparison, which
validates its structure and applicability to the request; verification owns the
comparison. There is no policy-name lookup, implicit current revision or inferred
replacement.

Each case selects one contract in `ComparisonRelation.effects` by the digest of
its canonical encoding. Selection
requires all four concrete MMIO read/write, fence and modeled-delay channels. Internal
memory/branch, call, return and final-memory selections remain independent and
compose with the effect relation. A selected contract the comparison was not
given is an error; absence
of an effect selection retains the explicitly selected physical comparison.

The finite profile permits up to 128 uniquely named rules. Every rule has a reason,
explicit per-side patterns and minimum/maximum occurrence counts. MMIO selectors
use aligned physical addresses and widths 1/2/4; fence selectors use the captured
predecessor/successor masks; delay selectors use a duration or explicitly all
modeled delays. Patterns may require an exact value or allow any value. Selectors
on each side must not overlap, including overlapping MMIO spans and wildcard
versus specific delays. There is no first-rule precedence. Fence patterns have no
separate value constraint because their complete observation is the masks.

A pattern may add a `followed_by` selector: it then selects an effect only when
the immediately next concrete MMIO, fence or delay effect on the same side matches
that selector. The end of the case has no successor. Two patterns whose base
selectors overlap are distinct only when both carry non-overlapping successor
selectors. Verification evaluates the successor with a one-event lookahead; no
other context or history is expressible.

`unclassified` selects the treatment of effects that no rule selects. The default
`incomplete` keeps the every-effect-classified policy below. The explicit
`required` policy compares such effects exactly, in order and value, as though a
required rule over their own identity selected them; it adds no count obligation.

| Disposition | Ordered comparison obligation |
| --- | --- |
| `required` | Identical patterns on both sides; every selected occurrence compares exactly, including values and order among other selected observables. |
| `omitted` | Identical patterns; replacement may omit vendor occurrences. Every retained replacement occurrence must equal a vendor occurrence in order. Removing an occurrence never licenses changing its value or moving it across a selected call, memory access or other effect. |
| `replaced` | Both explicit patterns must hold; corresponding occurrences compare by the same rule identity and order. Values are constrained by the individual patterns, without an implicit physical equality requirement. |
| `added` | Only a replacement pattern; matching replacement effects discharge its count/value obligations and remain raw evidence outside the paired stream. |
| `ignored` | Identical patterns with any value and a positive maximum. Matching effects on either side are implementation plumbing: they remain raw evidence outside the paired stream, discharge only count bounds and never shift alignment. |
| `forbidden` | Identical selectors on both sides, unconstrained value and zero counts. Any matching observed occurrence is a known violation. |

A minimum of zero means required when observed. Otherwise missing exercise is
INCOMPLETE, including when both complete runs have no occurrence. The replacement
minimum of an omitted rule is always zero. Counts reset for each case, including
warm continuations. Excess counts, forbidden effects and violated exact-value
constraints establish DIFF from actual observations, even if execution stopped
later. A completed shorter required stream also establishes a difference; an
unfinished prefix cannot establish equality or an absent future effect.

Under the default policy every raw concrete effect is classified. Unclassified observations produce
INCOMPLETE and stop positional alignment: an unknown omission/replacement must
not shift subsequent positions into a fabricated mismatch. Independently known
return/final-memory differences and directly observed policy violations can still
establish DIFF. Full policy accounting precedes alignment, so a violation after an
unclassified event remains visible. Neither a satisfied contract nor excluded
observations can turn unfinished execution or unmet environment obligations into
MATCH. Raw excluded effects, exact inputs and model participation remain in the
records.

`CaseComparison.effect_claim` exposes the selected claim ceiling even for DIFF or
INCOMPLETE. `selected-effect-equality` permits only required/forbidden rules;
`reviewed-effect-refinement` is mandatory for omitted/replaced/added/ignored rules. MATCH
under the latter means the reviewed refinement held for these concrete cases,
not physical event equality, general equivalence or hardware qualification.
`effect_gap` identifies the first vendor-side gap, then replacement-side gap:
an unclassified raw event ordinal, otherwise an unexercised rule ordinal.
`EffectViolation` includes side, raw event ordinal, rule ordinal and cause. Gaps
are retained alongside an independently established difference.

Verification borrows the resolved contract and raw observations. Bounded per-side
counters have no expanding heap allocation; every rule scan consumes the shared
work budget. [Effect scenarios](../../next/tests/execution/effects.rs) and
[verifier tests](../../crates/verification/src/effects.rs) cover these boundaries
and policy composition.
