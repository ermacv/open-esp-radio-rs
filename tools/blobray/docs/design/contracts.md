# Interfaces and ownership contracts

This is the contract authority for Blobray Next. Implemented boundaries are
identified below; broader interfaces explicitly marked target describe unsupported
capabilities. The [command reference](../../next/README.md) owns CLI syntax.
[Architecture](architecture.md) owns components and [workflows](workflows.md)
owns supported use cases.

## Result assessment

`RunState` describes lifecycle only. `Completed` means a valid result was delivered
or atomically published; it does not promise complete research or a matching
comparison. `ResultAssessment` is shared by `QuerySummary::assessment`,
`QueryOutput::assessment` and `RunHandle::wait().assessment`:

| Field | Meaning | Applicable operations |
| --- | --- | --- |
| `coverage: {subject, status, scope}` | `complete`, `partial` or `unknown` for the identified result, never an unscoped boolean | Inventory revision, function analysis, investigation publication, execution evidence or static-target audit |
| `check` | `pass`, `fail` or `inconclusive` policy decision | Doctor, link-plan, knowledge validation and static-target audit |
| `comparison` | `MATCH`, `DIFF` or `INCOMPLETE` under the retained relation | Concrete comparison only |

A failed/cancelled/limited attempt has no assessment. Successful listings,
selection, inspection, image preparation, knowledge mutation and exports have an
empty assessment unless a specific result contract supplies one. A function
without semantic analysis has unknown semantic coverage even if decoding is
complete. A partial inventory returns partial coverage in both query summary and
handle. CLI returns success for valid partial research and comparison evidence;
a failed/inconclusive check command returns failure. No `clean` alias or generic
run-level `complete` field exists.

Store validates assessment subject against the published result identity. A
completed durable run contains exactly one result reference. Journal schema 24
and database schema 23 are mandatory; incompatible projects/journals are rejected
without conversion. Execution and revision manifests keep independent versions.

`coverage.scope` is mandatory: `inventory-occurrences`, `function-extent`,
`selected-function-extents`, `execution-scenario` or `static-resolved-transfers`.
Whole-library completeness does not quantify over every executable byte. The
`coverage` query reports section-relative intervals outside the union of selected
extents separately; these intervals are neither inferred functions nor padding.
Unknown structure remains explicit and does not become a zero-byte observation.
`PASS` for target auditing covers resolved static transfers only. Unknown indirect
transfers remain separately counted. Comparison retains its explicit relation:
ordered MMIO/fence events and, when selected, low u32 return values; RAM, call
observations and the high return register are outside that relation.

### Durable and ephemeral acquisition

Import captures live inputs for durable research. `audit-targets` is an explicit
exception: the supervised read operation captures one external ELF into admitted
memory, checks detectable file changes and returns the digest of bytes actually
analyzed. It does not create a project revision or claim an atomic filesystem
snapshot. Durable operations never use this path to reopen missing source files.

## Concrete application scenarios

`start_analyze_project(InvestigationInput)`, `start_research`,
`start_propose_register` and `start_replay` each own one admission, run, writer,
worker, deadline, work counter, working-capacity authority and disk budget.
Automatic investigation freezes the source revision at admission and retains its
plan. Research resolves an exact function from the selected immutable publication.
Register proposal derives occurrence/evidence from the selected analysis. Replay
requires the original executor/environment/verifier identities and exact request.
CLI parses parameters and renders outcomes; it performs none of these resolutions.

A `Scenario` journal operation retains original intent. `resolved_operation`
records the concrete operation only in the successful publication transaction;
coordinator validation binds it back to immutable inputs. Failed attempts retain
no resolution/result/assessment. Resolution is not a nested run or a generic
workflow framework. Saved-plan investigation uses the same execution path with
its selection independently verified. Automatic planning passes its temporary
entry stream directly into execution under the original budget.

## Research memory and read-only observations

Single-function local analysis returns an owned staged manifest. Its prepared
ELF, section views and reference indexes end before enrichment starts. Library
analysis keeps one prepared object across its local functions; enrichment cannot
run inside that owner. No global object cache is introduced.

Research loading admits a bounded JSONL decoding workspace before deserialization,
then transfers records into `RecordBuffer`: vector capacities, boxes, names,
identities and nested value collections remain charged with their owners. Shared
reference targets are conservatively charged per reference. Composition admits
mapping/return workspaces from type sizes and maximum resident counts; output
grows through admitted containers, charging old/new buffers simultaneously.
Original records end after replacement; callee facts end after their last parent.
Cyclic components keep explicit partial local facts. This is capacity admission,
not a no-heap or whole-process RSS guarantee.

Phase diagnostics contain `reserved_bytes` at the last admission/release
observation and `peak_reserved_bytes` at such observations, alongside work/time.
They include allocations carried into that phase; phase peaks must not be added.
`load-research` and `compose-research` distinguish retained facts from ELF
preparation. These measurements are diagnostic and do not enter result identity.

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
are local to one run. Store's `knowledge_snapshot` verifies every selected event
and evidence root once, then folds review/supersession into admitted owned entries.
No global cache, scheduler or allocator is introduced. `WorkingMemory` remains
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
| `CodeRangeId` (target) | Object, section and byte range for code without a symbol; boundary justification is a separate fact |
| `RevisionId` | Immutable source manifest: ordered input roles/bindings, captured occurrences, target and inventory producer |
| `KnowledgeRevisionId` | Immutable review event chain, independently selected by analysis recipes; does not change a source revision |
| `PublicationId` | Immutable completed result root for an identified revision; independent of the mutable current-publication reference |
| `PlanId` | Immutable operation recipe bound to a revision and requested obligations |
| `RunId` | One execution attempt of a plan; retries have separate run identities |
| `EvidenceId` (target) | Immutable evidence record and its recorded dependency closure |
| `SubjectId` | Reviewed semantic subject; association with physical occurrences is explicit |

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

`ObjectOffset` identifies an offset in an identified object section.
`ImageAddress` identifies an address in a selected prepared image. Conversion
requires that image's recorded mapping. Address widths are validated against the
selected target; no narrowing to RV32 occurs in generic identities or storage.

Every observation records its subject, input identities, producer identity,
applicability and completeness. An evidence reference identifies both record and
relevant location within it. Assertions additionally record the review decision,
accepted interpretation and supporting evidence. A missing supporting record
cannot be reconstructed from a display label.

## Resource owners

| Resource | Owner and acquisition | Handoff and release |
| --- | --- | --- |
| Source bytes | Store import transaction copies caller-selected inputs | Immutable file leases retain captured content; caller keeps ownership of original files |
| Parsed container | Artifacts constructs views over a lease | Views cannot outlive their bytes; parsing never reopens a source path |
| Project snapshot | Store reader captures revision and selected publication in one metadata transaction | Cloneable read handles retain leases; final release removes transient retention |
| Provider set | Host constructs one validated set for an application | Runs retain exact descriptors/implementations until completion; no global installation |
| Write transaction | Store grants one writer authority | Commit consumes the transaction; rollback/drop leaves published roots unchanged |
| Analysis memo | Analysis worker, scoped to its recipe and dependencies | Drop or eviction loses only recomputable work |
| Prepared image | Image-preparation operation under a job | Artifact and mapping leases transfer to execution; source and tool identities remain attached |
| Execution session | Application, created for a declared scenario lifecycle | Warm phases retain session-owned state; cold reset/completion/cancellation releases it |
| Run handle | Application supervisor registers a job | Client requests cancellation or waits; supervisor owns cleanup regardless of client handle lifetime |
| Child process and temporary files | Host process adapter under the supervisor | Termination includes descendants and reaping before job resources are released |
| Export bundle | Application export operation over a snapshot | Caller receives materialized files and manifest; repository evidence ownership is unchanged |

Read handles expose no writer, maintenance or provider-installation capability.
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
| `plan` | Snapshot, operation request and resource budget | Immutable plan, dependencies, obligations, cache decisions and missing prerequisites; no analysis publication |
| `start_run` | Plan and application capabilities | `RunHandle`; executes the recorded plan rather than resolving new inputs |
| `query` | Snapshot and typed selector | Observations/projections with provenance, completeness and diagnostics; no implicit analysis or repair |
| `review` | Base revision, assertion changes, decision and evidence references | Validated knowledge transaction producing a new revision or a conflict |
| `compare` | Snapshot, both compiled implementations, scenario and relation | A planned verification run with explicit claim scope |
| `export` | Snapshot, selected records, destination and overwrite policy | Verified bundle/manifest; never changes accepted knowledge |
| `maintenance` | Retention preview and explicit cache/repository scope | Separate authorized operation; revalidates roots and active leases before deletion |

`RunHandle` exposes status, typed progress events, cancellation and completion
waiting. Every event identifies its run and phase. A late result from an older
run cannot replace a frontend's selected revision. Event delivery is bounded:
intermediate progress may be coalesced, but the terminal outcome is retained and
available through status/wait even when a client stops consuming events.

Plans retain the exact revision, requested entry occurrences, ordered input
roles, linker recipe where needed, provider/model identities, pass and schema
versions, parameters, declared environment inputs, outputs and resource budget.
Planning may hash/imported-read inputs and inspect existing cache metadata. It
does not populate a missing cache, execute passes or migrate a repository.

A run can complete on an older revision after the project advances. Its result
remains bound to that revision. Advancing the current publication requires the
expected base revision/publication to still match; otherwise the completed
result is retained and the caller receives an explicit publication conflict.
There is no automatic rebase of a running computation.

Next implements `Plan` for inventory inspection only: one revision, input,
object or symbol scope. Its live owner retains a captured manifest; portable
reopening revalidates project membership and recipe dependencies. This is not
the general pass/provider graph or a transitive retention pin described below.
See [inspection plans](../../next/README.md#selection-and-inspection-plans) for
the implemented schema, resource handoffs and command semantics.

### Handles and capability boundaries

The following are target API obligations, not signatures promised by the current
import implementation. Public research records, application handles and private
worker messages have distinct compatibility boundaries.

| Interface | Inputs and result | Ownership and failure contract |
| --- | --- | --- |
| `Snapshot` | Explicit project plus revision/publication selection; immutable selection with retained record/byte access | Store resolves selection once and pins its reachable closure atomically with respect to retention. Clones share the pin; last release ends it. No writer, recovery or origin-path capability is exposed. |
| `Plan` | Snapshot, typed operation request, pass/provider descriptors and budget; immutable dependency graph, obligations and recipe | Application owns planning. The live plan retains its snapshot; starting a run acquires its own retention. Missing prerequisites produce a blocked plan, never an implicit import or analysis. Reopening a serialized plan revalidates identities and reacquires leases. |
| `RunHandle` | One admitted attempt of an executable plan; status, progress, cancel and wait | Application retains ownership independently of client handles. Retries get a new `RunId`. Shutdown stops admission and drains owned jobs; dropping a client cannot detach them. |
| `LinkPlan` | Captured inputs in order, entry/root occurrences, companion bindings, target/ABI and identified tool recipe | Application owns the plan. Host materializes exactly those inputs in job-private storage. Reconstruction retains original occurrence mappings. No caller-supplied live binary path reaches the linker. |
| `PreparedImage` | Validated image bytes, recipe, selection/mapping evidence and unresolved boundaries | Owns leases for image and source mappings. Execution borrows these leases; memory views cannot outlive them. Missing exact mapping remains unknown; creating an execution session validates the obligations that session needs. |
| Analysis pass | Declared dependency views, ISA ports, control/capacity and result sink | Owns run-local computation and emits qualified records. Cannot publish or accept assertions. Consumer failure aborts production; it cannot be reclassified as malformed input. |
| Review candidate | Base revision, proposed assertions, explicit decision and retained supporting records | Knowledge validates meaning and applicability. Application commits a candidate bound to its validated inputs; store rechecks expected base and reference closure. Conflict requires a new review decision, not an automatic rebase. |
| Comparison | Two identified compiled implementations, prepared execution inputs, scenario and relation | Executor owns each session; verifier consumes observations and owns verdict construction. Application alone retains/publishes the resulting evidence. Models and frontends cannot supply the verdict. |

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
formats. CLI/JSON/TUI render the same results; host-internal transport may stream
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

Reviewed text packs remain portable, reviewable source material suitable for
Git. Loading or editing a pack does not mutate accepted knowledge in an existing
revision. The review operation validates and captures a new immutable version;
export materializes a selected version. An external edit must be imported or
reviewed explicitly before it affects a run.

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
Run, replay and comparison share this operation. Companion-provided data
definitions participate before relocation validation, just as call definitions do.

The implemented [synthetic image profile](../../next/README.md#synthetic-prepared-images)
uses explicit LLD 22 and captured relocatable inputs. Existing firmware/ROM ELF
bindings and execution sessions are not implemented.

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

## Analysis, knowledge and verification ports

The implemented [function operation](../../next/README.md#function-analysis-contract)
consumes an imported occurrence or a symbol in a selected prepared image, with an
explicit/declared extent, producing a retained local graph and separately
qualified coverage. `FunctionSource` and `CodeAddressSpace` distinguish section
offsets from image addresses. Prepared-image identity includes its link recipe;
the image's source revision is frozen even when the current revision differs.
Its ISA port is injected;
the analysis module never selects an external symbol implementation. Broader
pass scheduling and model policies below remain target interfaces; the concrete
integer execution/verification subset is implemented as described below.

Artifacts owns the borrowed `ImageMemory` view over validated static ELF load
segments. The view and its segment capacity cannot outlive the admitted input
buffer. Analysis can read constant bytes through that narrow port; it cannot
load files or request mutable machine memory. Application retains the image
lease while computing and stages results through the same function engine used
for ET_REL. Source mappings retain their precision; a synthetic address is not
an assertion about original placement. The implemented static memory profile
and transfer limitations are documented by Next, without claiming execution
or interprocedural completeness.


Analysis passes declare inputs, output kinds, semantic implementation version
and dependencies. One declaration drives planning, cache lookup and execution.
Passes consume snapshots and return typed results; they do not publish files or
select a current revision. ISA ports supply decode/lift/execute operations with
explicit target and ABI context.

A pass declaration specifies required/optional dependency kinds, interpretation
inputs, output schema, semantic version, coverage obligations and termination
policy. Planning and execution consume that same declaration. Optional absence
is an explicit input outcome; it cannot hide a missing required dependency.
The pass graph must be acyclic. Cycles in the analyzed program are handled inside
a pass by a bounded worklist with a declared repeated-visit or fixed-point rule;
they are not recursive scheduler calls. Nonconvergence reports an unsatisfied
analysis obligation; exhaustion of the operation budget is a resource failure.

Cache keys cover the semantic dependency closure: input content/occurrences,
configuration, selected models, producer and schema versions, link recipe and
execution parameters where relevant. Paths and modification times do not stand
in for content. Runtime memoization does not become evidence. Recursion-dependent
or otherwise context-sensitive results include that context or remain uncached.

An execution limit and a semantic analysis parameter are different inputs.
Changing a depth/precision parameter that defines an analysis result changes its
computation identity. A larger emergency process limit alone does not relabel
existing evidence. A cached incomplete result cannot satisfy a request for a
stronger obligation merely because its bytes are available. Cache hits still
validate identity, integrity and coverage under the current operation budget.

Knowledge validation checks subject identity, applicability, evidence existence,
claim strength and conflicts. It produces an acceptance candidate; the application
commits it with an expected base revision. Correspondence and lineage analyses
provide proposed associations, while reviewers accept their intended meaning.
A changed body or ABI does not inherit an old proof merely because its name or
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

## Durable repository and disposable cache

Store owns transactional metadata and immutable content-addressed objects.
Repository identity and object references are independent of installation paths.
The implementation uses SQLite transactions for metadata and durable immutable
object storage for payloads. Pack layout, compression and indexes are internal
storage choices; no public ID contains a pack offset or database row number.

| State | Retention root | Can cache clear remove it? |
| --- | --- | --- |
| Imported inputs in a retained revision | Revision manifest | No |
| Accepted assertions and review provenance | Knowledge revision | No |
| Evidence cited by retained knowledge | Evidence reference and its transitive dependencies | No |
| Current completed publication | Current publication reference | No |
| Explicitly saved investigation/result | User retention reference | No |
| Open snapshot and prepared image | Active reader/job lease | No |
| Computation lookup metadata and unreferenced intermediate results | Cache ownership only | Yes |

Successful historical runs are not all retained forever. Their objects remain
protected while reachable from these roots. Repository pruning is a separate,
explicitly previewed operation; cache clear cannot remove repository roots.
An active read lease protects the referenced bytes through publication and
compaction. Persisted user references and transient process leases are distinct;
crash recovery cannot discard a user reference as a stale process lease.

The store owns the full closure of evidence retention: cited records, their
captured inputs, interpretations, producer identities and comparison context.
Re-reading an archived result does not require the old analyzer to execute.
Re-execution additionally requires the recorded tools/providers; their absence
is an explicit limitation, not permission to substitute current versions.

Disk quota failure identifies protected data and prevents a new write that
cannot complete. Automatic maintenance may reclaim cache-only unreachable data;
it does not expire accepted knowledge or delete evidence to make an operation fit.

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
integrity error. Optional cache corruption is a cache miss with a diagnostic and
recomputation; it cannot replace or reinterpret corrupted retained evidence.

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
planned -> queued -> running -> validating -> completed
              |         |           |
              +---------+-----------+-> cancelling -> cancelled
                        +-----------+-> failed
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
remains a separate explicit authority. Next's [temporary storage contract](../../next/README.md#temporary-storage-and-crash-cleanup)
specifies implemented defaults, control reserve, accounting and reconciliation
bounds, including materialized link inputs and prepared images.

Application shutdown stops admission, cancels owned jobs, terminates/reaps
remaining children within the configured shutdown grace, and releases leases.
Dropping a client handle does not detach unmanaged computation. Closing an owned
CLI/TUI application closes its supervisor; an embedded client may keep its own
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

These are mandatory target contracts. The [Next implementation](../../next/README.md)
identifies which guarantees are implemented; the existence of a supervisor does
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
The [implemented memory boundary](../../next/README.md#current-memory-boundary)
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

Import, snapshot queries and doctor all owe bounded resource behavior in the
target system. Read-only operations retain their no-write/no-repair contract.
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
without the generating CLI. Downstream generators consume those contracts or a
snapshot reader, never private SQLite tables or pack offsets. Implementation
documents must publish their actual wire schemas before those APIs ship.

Legacy import is an explicit adapter described by the
[preservation workflow](workflows.md#preserve-existing-investigations). It retains
original bytes and identifiers, reports every mapping and unresolved record, and
does not overwrite its input project. Structural readability alone cannot
authenticate an old proof or establish its applicability to the new revision.

### Local value analysis boundary

The implemented [value and memory-effect contract](../../next/README.md#values-and-memory-effects)
extends local function analysis through an injected semantic port. The ISA owner
lifts operations, analysis owns fixed-point state, application owns execution
and store owns retained bytes. Unknown values, incomplete effects and failed
resource admission are distinct outcomes. No hardware knowledge or executable
memory environment is acquired implicitly.

### Implemented review and preservation boundary

[Blobray Next review and preservation](../../next/README.md#knowledge-and-preservation)
owns the implemented wire contracts. `KnowledgeRevisionId` identifies review
events, independently of the import `RevisionId`; existing import IDs retain
their original meaning. `SubjectId` is a project-scoped semantic key. Claims
retain their exact source occurrence and typed evidence references. Accepted
hypotheses do not become observations or proof. Selection of an accepted function
extent is explicit in an investigation plan.

Storage metadata and journal format support follow the result-assessment contract above.
Backup/restore preserve source, analysis, publication and knowledge identities.
The legacy adapter preserves bytes and records unsupported semantics; it does
not import the legacy engine or qualify old comparison results. Existing target
executable ABI/interface validators and register-publication policies remain outside
this implemented review vocabulary. Reviewed MMIO region/register interpretation is implemented.

### Implemented PHY/ROM research boundary

The [native research operation](../../next/README.md#phyrom-research) extends the
same local engine with a bounded expression DAG and acyclic call composition.
Application selects exact published callees; analysis receives borrowed facts and
returns admitted owned output. Recursive components retain local facts and gaps.
The explicit ABI assumption, source/companion publications and knowledge revision
are recipe dependencies. Callee may-effects do not establish ordered execution.
Expression provenance and source-qualified addresses survive composition.

Knowledge occurrences identify input or prepared image, object and optional
function symbol. Accepted MMIO descriptions label observations without providing
load values or hardware qualification. Root/image applicability is validated on
proposal and review; a mapping never silently transfers an assertion.

Prepared-image policy 4 records exact captured ROM function definitions and places
executable sections by ELF flags. No compatibility adapter, model stub or implicit
companion lookup participates. The original ELF/code view ends before composition;
working buffers and summary reservations end after their staged delivery. The
same supervisor retains cancellation, capacity and atomic publication ownership.


### Implemented concrete execution boundary

Domain owns `ExecutionRequest`, `ExecutionProducer`, observations and the injected
`Executor`/`ExecutionMemory` ports. Artifacts lends validated static ELF segments;
application owns captured leases, mutable session buffers, explicit register-bank
state and all phase transitions. The RV32 backend owns concrete register/PC state;
verification alone computes the ordered-observation verdict. Store validates
receipt/record structure and atomically publishes the evidence with its run.
None of those computation modules can select live paths or acquire publication.

The invocation owns a bounded vector of known/unknown physical RV32 ABI words.
Domain validates stack geometry and computes entry SP; application initializes
stack arguments in its freshly owned stack, and passes eight optional register
words to the backend. Unknown argument slots override stack seeds. No register or
stack word becomes zero by omission, and setup emits no guest events. The caller
owns type/variadic lowering. Request and manifest schema 5 pin this interpretation;
producer identity pins both backend unknown-value semantics and stack environment.

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
phase RAM, stack, MMIO and reservations do not. RAM lifetime is part of its mapping
identity and cannot change while that mapping is live. Phase buffers release after
comparison/staged serialization. A warm successor to incomplete execution is blocked;
a cold case discards prior state/dependencies and starts a fresh chain without
erasing earlier incompleteness. All chains share one control, memory/disk authority
and publication, never separate CLI operations.
Physical execution goals belong to each invocation. Application validates that
symbols belong to mapped sources in the target revision; artifacts resolves exact
FUNC/NOTYPE table entries against executable file-backed mappings. Operation-local
borrowed goal indexes group all requests by captured object, with one prepared owner
per group released before sessions. Backend receives only a resolved goal and initial
register/stack state; it never resolves symbols or retains ELF metadata.

Returned, reached-symbol and observed-call are distinct completed outcomes. An early
return before a non-return goal is incomplete; a completed early goal permits a warm
successor but never promises the callee body ran. `complete` means all declared goals
were met, not that every phase returned. Verification compares only the declared
observable prefixes; non-return goals cannot compare return registers. Store validates
outcome/goal kind and cold/warm dependency blocking without owning ISA interpretation.

Reading execution evidence owns decoded-manifest capacity through its lease;
SQLite journal cells are admitted before incremental loading. Querying does not
execute or repair anything.

Replay requires the original executor, environment and verifier identities. A
known difference survives other incomplete cases; an emergency resource failure
publishes no completed evidence. The implemented relation compares exact ordered
MMIO/fence events and optionally one 32-bit return, with a caller-declared compiled
binding ceiling. It cannot claim arbitrary-domain or hardware equivalence. See
[concrete execution](../../next/README.md#concrete-execution-and-comparison) for
request limits, memory initialization, schemas and unsupported behavior.


### Captured data interpretations

`DataRequest` binds a `KnowledgeOccurrence`, explicit ranges and retained analysis
IDs. Artifacts owns the verified ELF buffer and prepared section metadata; each
`DataView` borrows them for one callback. Application resolves the occurrence
once, owns the query budget and streams observations. Store neither parses ELF
nor interprets a table. Query delivery owns its private exported files until
transfer or drop, and does not publish knowledge as a side effect.

`KnowledgeClaim::IntegerTable` describes integer encoding/count/stride over exact
captured bytes. Proposal and review require matching Source evidence for the
whole selected file range. A `Constant` claim requires a known selected operand
at an exact Analysis record ordinal. Both require purpose and applicability and
use the existing expected-base/review/supersession transaction. Source revisions
remain separate from knowledge revisions. A constant's value is an RV32 bit
pattern, not a fabricated data payload.

Data exports retain the captured object, selected bytes, analysis coverage and
optional accepted assertion at a fixed knowledge revision. Original paths are
provenance only. Image VMAs are checked against file-backed load mappings;
section-relative and file-relative offsets are separate fields. Mutable section
bytes are initialization only. Relocations are retained, never implicitly applied;
this integer profile withholds numeric decoding when a known relocation write
intersects the selected range or any section relocation has unknown write extent.
Proven disjoint fixed-width writes do not block unrelated integer data. The
manifest retains total, overlapping and unknown-extent counts. Unknown transforms
are never excluded merely by their offset, and invalid known extents are rejected. Successful export makes no general completeness or comparison claim.

### Physical symbol selection

Prepared function/data views select SHT_SYMTAB or SHT_DYNSYM by physical table
kind, section and entry index; conventional section names are not identities.
One table of each kind is supported. Missing, duplicate or mismatched tables and
invalid indices fail before publishing an analysis or accepting knowledge.
Dynamic symbols in static RV32 ET_REL/ET_EXEC remain ordinary captured metadata;
they grant no dynamic-loading or TLS support. Static relocations keep their own
`sh_link` target table independently of the selected function/data symbol. A
relocation referencing another table is explicitly outside the current profile.

Whole-object enumeration preserves both static and dynamic STT_FUNC occurrences,
including aliases, as separate physical requests and results. Coverage unions
selected byte intervals without turning duplicate symbols into additional bytes.
Function selection policy 8 and investigation policy 4 record this interpretation;
reading does not convert older policies. Regression coverage lives in Next
`functions` tests `dynamic_occurrences_keep_physical_indices_through_review_export_and_reopen`,
`dynamic_function_selection_keeps_static_relocation_target_identity` and
`dynamic_and_static_function_aliases_remain_distinct_in_saved_publication`.

### Explicit executable ranges

`FunctionSelector` distinguishes physical symbols from `{object, section, extent}`
code ranges. The latter require no symbol table and reject a separate symbol-size
override. Artifacts owns section/type/alignment/bounds/backing validation; the
application's shared occurrence acquisition owns source/image/revision identity.
No range is inferred from neighbors, padding or disassembly. Prepared views keep
the existing callback lifetime and admitted memory owner.

`KnowledgeClaim::ExecutableRange` uses a symbol-independent occurrence and exact
section/extent. Proposal and acceptance validate the same captured bytes as
analysis. Overlapping differing ranges in the same section conflict. Reviewed
boundaries supply native `InvestigationRequest.ranges`; unused or duplicate
selections fail planning, invalid bytes produce blocked function outcomes. Store
validates selector/section/extent against the admitted request before publishing
and when reopening retained results. Coverage joins explicit ranges to section
metadata and unions them with symbol extents, leaving other bytes unclassified.

Function schema 7 / policy 8, investigation schema 3 / policy 4, database schema 23
and journal schema 24 carry these identities. Old formats are rejected without
mutation or conversion. `functions::ranges` regressions cover table-free ordinary
and thin archives, generic review, invalid ranges, source-free export and coverage;
`reviewed_image_code_range_keeps_vma_identity_and_unions_symbol_coverage` covers
prepared-image review and virtual addresses.

### Captured pointer-table observations

`DataLayout` distinguishes integer and pointer table proposals. `PointerTable`
specifies count/stride for captured little-endian RV32 four-byte slots. Raw
`DataRequest.pointer_table` is an explicit observation request over one range;
accepted exports derive that request from `KnowledgeClaim::PointerTable`. Generic
and specialized proposals, review and export share physical occurrence and exact
byte-evidence validation. Conflicting overlapping integer/pointer interpretations
cannot coexist as accepted assertions.

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
values establish neither executable mappings nor function/ABI boundaries. An
accepted layout is not a claim that every target is executable or resolved.
Data manifest schema 3 includes producer identity and classification counters;
resource failure aborts the query/proposal, never truncates a successful table.

Next pointer regressions cover source-free ordinary/thin exports, overlapping and
partial writes, unknown/wider relocations, raw addresses, generic review and shared
budget exhaustion. The real PHY scenario checks all eleven physical relocation
targets and an independently established digest for `phy_i2c.o` `.rodata`, then
reviews, exports and reopens the table after project restore.

## Finite value alternatives

The current RV32 values profile and function policy 8 preserve at most eight
canonical exact alternatives at a register join. Domain owns nonrecursive leaves
and validates 2..=8 sorted distinct entries when decoding saved values. Analysis
owns the finite lattice and admitted, operation-local set/index storage. Values
and expression outputs borrow no set owner after analysis. No global cache or
additional allocator authority is introduced.

Joins retain a may-set; arithmetic applies to each bounded operand pair and
immutable image loads require all candidate reads to be modeled. Unknown inputs
absorb exact information. Distinct expression IDs and incomplete relocation
uppers are not exact alternative leaves. Overflow emits `SemanticGap::AlternativeLimit`
and unknown, never a truncated set or a chosen target. Work/cancellation admission
includes set lookup/growth and each candidate evaluation. Cycles converge under
the finite-height lattice; resource exhaustion creates no published partial result.

Read queries match each possible address/symbol without changing or recomputing
the saved facts. Call filters include ambiguous finite targets as unresolved.
Research keeps their transfer records and does not compose an arbitrarily selected
callee. Callee image alternatives become source/object-qualified addresses; callee
stack alternatives cannot be treated as caller storage. CFG edges and original
instruction/relocation records remain provenance; sets do not encode path
correlation, prove reachability or expand indirect control flow.

Contract regressions: analysis `value_sets::tests` and
`values::tests::joins_and_loops_converge_independently_of_visit_order`; Next
`finite_pointer_loads_keep_both_callback_targets_in_queries_research_and_reopening`
and `persisted_alternatives_are_flat_bounded_and_canonical`.

## Conditional interface declarations

`KnowledgeClaim::Interface` owns one bounded boxed `InterfaceContract`. Domain
owns roots, paths, layout/slot signatures, index domains, guards and semantic keys.
Knowledge performs pure layout/ABI/domain/guard validation and acceptance conflict
checks. Application uses the shared captured-occurrence owner for all proposals
and reviews: symbol roots are physical data-symbol identities, argument roots
validate a real symbol or explicit function range, section roots validate a physical
section location, and payload guards match the
actual captured object bytes. Optional occurrence symbols have the same validation
as data export. No caller reopens a live path or guesses a symbol by name.

Accepted interface declarations are conditional assertions. Runtime guards and
index ranges are preconditions, never observed execution facts. `semantic` is a
reviewed subject key, not a model/callee resolver. Literal-root bounds and static
range overlap are checked; different dynamic dereference paths do not establish
runtime non-aliasing. Slot ABI support is an explicit RV32 integer/pointer,
nonvariadic profile; an unknown slot signature is represented by `None`.
Declaration of an argument position does not imply that an
execution engine supports its ABI placement.

The existing knowledge lifecycle supplies expected-base admission, review states,
immutable revisions, query/export and atomic publication. Store accounts the
boxed contract and variable capacities when retaining knowledge snapshots. The
same 64 KiB event bound applies; no new catalog, compatibility format or execution
workflow is introduced. Source-free reopening retains exact interface/evidence
identities; knowledge JSON export remains a reference export, not a binary backup.

Regressions: knowledge `interfaces::tests` checks guards/domains/ABI and physical
static overlap; Next `native_interface_roots_validate_review_and_export_without_sources`
checks generic admission, every root kind, symbol-less arguments, thin members,
resource failure and source-free CLI review/export.


### Interface observation lifetime and matching

`ReadQuery::Interfaces` owns one explicit `InterfaceQuery`. Domain owns its request,
observation and summary values. Analysis interprets the retained flat expression DAG
iteratively and builds admitted instruction/call/expression indexes. Application
loads one saved analysis into a `RecordBuffer`, or prepares one captured data object
and borrows its span for streaming pointer interpretation. It loads only the requested
knowledge revision (`None` is empty) and builds a sorted, admitted physical path/slot
index. No per-call full knowledge scan, hidden analysis, extra resolver authority or
persistent discovery cache is introduced. Output bindings and variable payloads are
admitted until the synchronous sink returns; errors discard the supervised query's
private staging without publishing a truncated result.

An observation retains the input analysis record or pointer-slot ordinal, instruction
or data offset, exact paths, saved target/pointer value, issues and every candidate
binding's review state. Matching requires the same revision/source/object and physical
root/path/slot. Static offsets are canonicalized; dereference/index boundaries remain
part of identity. No address-to-symbol guessing or cross-occurrence alias inference is
performed. Accepted candidates can remain ambiguous. Guards and index domains remain
unverified runtime preconditions; signatures and semantic keys can remain unknown.
The query has counts, not a general coverage or verification assessment.

The current analysis profile recognizes saved indirect calls (also ET_REL), four-byte
loads, constant offsets, incoming stack words and scaled a0..a7 words under an explicit
integer ABI. Known numeric/finite targets without retained load provenance produce
`NoPointerPath`; unsupported expressions and foreign occurrences remain explicit.
It does not reverse-engineer an originating table from a destination address, expand
callee control flow or claim executable reachability. Data queries preserve captured
initialization and relocation uncertainty. Source-free query/export retains reference
identities; a JSON observation export is not a transitive research backup.

Regressions: analysis `interfaces::tests` covers exact argument/index paths, canonical
keys, malformed DAGs and resource failure; Next
`saved_callback_discovery_review_states_guards_and_export_share_one_query_contract`
and `captured_symbol_less_pointer_slots_match_only_selected_structural_declarations`
cover shared API/CLI, review states, conditional matches and source removal. The real
`phy_research.py` checks the independently read ROM global/slot path, explicit unknown
signature, acceptance and byte-identical query export after backup/restore.


### Function/context declaration boundary (implemented profile)

A function contract belongs to one exact captured function selector and occurrence.
It shares the integer/pointer call-signature vocabulary with interface slots; it
adds function/return roles, named argument contexts and field layouts, applicability
and explicit preconditions. A missing signature stays unknown. Context offsets are
signed and bounded by an explicit extent; fields have byte widths, optional types
and declared read/write roles. These are reviewed interpretations, not substituted
analysis facts, allocation ownership or execution permissions.

Typed argument predicates constrain raw ABI bit patterns (inclusive ranges and
masked equalities); context predicates constrain explicitly located bytes. Bounds,
contradictions and unsupported representations must fail admission. An uninterpreted
named assumption remains attributed text, never an evaluated predicate. Review does
not turn any precondition into an observed runtime fact. Pure validation and conflict
checks belong to knowledge, captured selector/evidence validation to application,
and immutable review publication to store. No frontend owns a second validator.

Saved facts do not change when a function contract is accepted. Read/export must
retain exact occurrence, evidence and review identities after source removal. A
contract with the same physical selector and incompatible interpretation requires
explicit review replacement. Alias equivalence cannot be inferred from a display
name. Narrative paths and event-route witnesses are separate assertions with their
own evidence; this declaration alone does not prove an executable route.


`KnowledgeClaim::Function` boxes one `FunctionContract`; `CallSignature`,
`CallArgument` and `AbiValueType` are shared with interface slots. Application
validates the exact symbol or executable range through the captured prepared-object
owner, including image and thin-member occurrences. A data symbol cannot stand in
for a function. The signature has at most 32 arguments; the contract has at most
16 contexts, 128 fields in total and 64 preconditions, inside the same 64 KiB event
limit. Store accounts boxed and variable capacities with its knowledge snapshot.

The RV32 integer profile allows 8/16/32/64-bit integer and pointer values. Context
fields may have unknown types; known types must match their widths. Packed fields
are permitted but overlapping field declarations require another explicit profile
and are rejected here. A known signature's context argument must be a pointer;
unknown signatures do not invent arity or return types. A return role requires an
explicit non-void return type. Context read/write roles are declarations, never
substitutes for retained observed accesses.

Argument range/mask predicates require explicit signature types. Bit ranges are
unsigned even for signed ABI values, and non-null pointer types exclude zero.
Admission intersects ranges and masks using a fixed 64-bit tight-bound solver;
it does not enumerate the input domain. Little-endian context predicates reject
contradictions across overlapping byte/halfword/word/doubleword views. Assumptions
have unique attributed IDs and remain uninterpreted. None of these checks asserts
runtime satisfaction or modifies analysis identity/results.

Contract regressions: knowledge `functions::tests` independently enumerates small
predicate domains and checks 64-bit boundaries, layout/type errors and contradictory
predicates; Next
`function_contracts_validate_review_and_export_exact_symbols_ranges_and_contexts`
and `image_function_contracts_reject_data_symbols_and_preserve_review_after_source_removal`
cover ordinary/thin/image capture, shared CLI/API, resource/conflict atomicity and
source-free review/export. Signature validation also remains covered by the native
interface regressions through the shared `calls` validator.


### Saved research navigation

One application read query selects an explicit source revision, saved analyses and
publications, and optional knowledge revision. It never chooses current heads or
starts missing analysis. Function identities are source plus exact selector;
results retain analysis IDs and record ordinals, including unresolved and ambiguous
calls. Callers/callees describe retained structural may-edges, not execution traces.
Multiple saved interpretations of one function are not silently collapsed.

Application owns bounded operation indexes and reads each selected analysis's facts
once. Decoding/workspace ends before the next function; retained call/selection
metadata is admitted separately. Domain owns navigation values; analysis owns flat
expression/access-path interpretation; artifacts validates selected object locations
including NOBITS without inventing initialization bytes; store supplies retained
readers only. Memory-object matching uses physical symbols/sections or explicitly
qualified image addresses. Cross-object name matching cannot resolve an external
symbol. Unresolved addresses cannot establish absence of access.

Context-field queries require an explicitly selected accepted function contract.
They match observed address expressions against its exact function ABI word and
field extent. Read/write roles from review remain distinct from actual access kind;
preconditions are still unverified. Partial overlaps, finite alternatives and
unsupported paths stay visible with evidence. Read/export/reopen share this query
and impose the same work, memory, deadline and delivery limits.


`ReadQuery::Navigate` delivers `NavigationRecord` and `NavigationSummary` through
shared query staging and optional atomic JSON export. An explicit accepted contract
maps logical context arguments to physical incoming words; unknown signatures require
caller-supplied mappings. A known signature rejects contradictory maps. `AccessRoot`
and `AccessStep` are the shared physical vocabulary for interface and field paths;
entry words and index domains use `word`, not logical `argument` ordinals. ABI
placement and command examples are in the [operator reference](../../next/README.md#navigation-over-saved-research).

`store::AnalysisReader` owns at most 256 admitted, verified immediate dependency
handles until the query ends. Shared revision/publication/member roots are hashed
once; every function manifest and fact stream is still opened and verified. It is
neither an incremental cache nor a transitive retention pin. Snapshot and one-record
construction envelopes, sorted indexes, pending calls and emitted variable payloads
have separate admission. Failed admission/cancellation discards private query output.

Structural calls retain unknown/ambiguous targets and out-of-selection saved IDs.
A callers focus does not label unresolved edges as confirmed callers. Object queries
match exact physical ranges, including NOBITS; unknown and unqualified composed
addresses cannot prove non-access or inherit the caller's object. Context reads use
observed load/store/atomic kinds rather than declared roles. These queries neither
traverse an unbounded call graph nor schedule missing interpretation.

Regressions: Next `selected_navigation_keeps_call_evidence_deduplicates_scope_and_never_starts_analysis`,
`context_navigation_uses_logical_signature_arguments_and_explicit_unknown_mappings`,
`bss_reader_writer_navigation_uses_physical_ranges_without_fabricating_data_bytes`;
application `navigation::tests`; domain scalar ABI placement and store function
publication reader tests. The authenticated PHY scenario checks saved linked callees
and byte-identical navigation export after source removal and backup/restore.


### Structural flow and path review

A flow request chooses an explicit navigation scope, one exact root analysis and a
selected target analysis or effect profile. It never substitutes a different saved
interpretation of an entry. Application builds a bounded operation-local graph from
retained calls; analysis owns iterative reachability. Unknown/ambiguous transfers,
missing selected targets, partial analyses and depth boundaries remain frontiers.
The graph can return a representative structural path, never an executable witness
or absence proof outside the selection. A callee requires an unambiguous selected
physical edge; a reviewed interface label does not create one.

Graph construction releases each function's decoded facts. Effect delivery may
make a second linear pass over reached functions after graph construction; it does
not load a function for each caller or retain all function record buffers. Each
emitted effect refers to a source record and a predecessor path in the returned
selected graph. Local facts and composed facts retain their distinct origin IDs.
No source ELF preparation or new analysis occurs during flow reading.

A native ordered path assertion stores exact caller-analysis/record/callee-analysis
hops, purpose and applicability. Proposal and review recheck all participating
analyses in the same revision, require contiguous unambiguous edges, retain their
manifest/fact roots and reject missing/mismatched steps before publication. The
assertion is conditional reviewed structural navigation; it supplies no runtime
precondition satisfaction, event delivery or executable comparison claim.


`ReadQuery::Flow` shares navigation's selection/edge resolver and observed-manifest
callback; no second physical resolver or graph cache exists. Reachability accepts
only uniquely resolved selected edges. Parent links identify one deterministic
shortest call path; a depth bound reports a frontier for an unreached destination.
Effects retain source analysis/record and original `MemoryAccess` or `CalleeEffect`
values. They are evidence rows, not a deduplicated trace: a local callee effect and
its composed caller instances remain distinct. Address filters match access spans;
unknown or partly matching finite addresses remain visible with unknown match state.

`FlowSummary.target_reached` refers only to the selected structural graph, and is
null for an effect query. Frontiers/unavailable entries are separate counts; no
summary has an overall complete/PASS/executable claim. `facts_passes` counts decoded
function streams: one per selected analysis for the graph and, for memory effects,
one additional pass per reached analysis. The original ELF is never prepared.
Path assertions allow 1..64 contiguous acyclic hops and require exact root analysis
evidence. Changed paths for the same subject conflict until explicitly superseded.

Regressions: analysis `flow::tests` covers diamond/cycle/depth and failed admission;
application `flow::tests` checks unknown/alternative address filtering; knowledge
`paths::tests` checks finite declaration shape. Next
`selected_flow_paths_review_exact_hops_and_reopen_without_sources` covers review,
ambiguous physical interpretations, wrong records/occurrences, work failure and
CLI/API export after source removal. The PHY workflow checks a linked target,
44 composed fill records and the independently expected first encoded write, then
reopens identical flow exports after project backup/restore.


### Memory definitions at publication

A read request selects one retained analysis, an exact local call/transfer/store
record as the publication anchor, an optional explicit integer ABI, and locations.
An empty location selection discovers known local write spans; explicit selections
can name a local access record, literal span, entry-stack span or incoming ABI-word
pointee. Thus a selected read with no prior write can report incoming state. No
caller-invented symbol identity or hidden source reanalysis enters this query.

Analysis builds bounded indexes over saved instructions, edges, expressions and
local accesses. Per location, it walks CFG predecessors from immediately before
the anchor, stops at definite covering writes and retains possible/conditional writes,
incoming state, call clobbers and unknown/overlapping aliases. It reuses one admitted
worklist/visited/predecessor set per location instead of materializing all location
states at every CFG node. Loops use marked nodes and preserve the possibility of a
prior iteration of the anchor. Work and memory exhaustion fail the query atomically.

The returned local definition is `must`, `alternative` or `candidate` with an exact
record and structural suffix witness. A must classification requires one definite
last-write site, no incoming alternative or unresolved alias/clobber and closed
structural coverage. Partial-width overlap remains explicit without invented byte
composition. Different incoming pointers or dereferenced roots are not assumed
disjoint; composed callee effects do not erase an unmodeled call's clobber. These
are local write-definition relationships, not runtime memory contents, hardware
state, path feasibility or interprocedural effect-completeness guarantees.

`ReadQuery::MemorySlice` owns one authenticated retained fact stream. The pure
analysis port borrows it for instruction/access indexes, SCC membership and one
backward search per selected span. Explicit selections are bounded at 256; an
empty selection discovers preceding write spans within the operation budget.
Loading reports `LoadResearch`; local index/dataflow work reports `AnalyzeValues`.
No prepared ELF or global cache overlaps these owners.

`incoming` is `possible` when an unchanged entry value has a structural path,
`overwritten` when every such path meets a covering write, and `unknown` when
missing CFG or a surviving ambiguous path prevents that decision. A wide covering
write can establish overwritten while its narrower value remains candidate:
there is no byte-lane composition or projection. Definition facts retain original
operands, including atomic operands, not manufactured final RAM values.

Equal entry-register words, stack offsets and physical addresses share locations.
Different pointer roots may alias. A scalar computed once outside every CFG cycle
can identify one loaded pointer and its disjoint fields. Its expression ID belongs
to this saved analysis, never to a host allocation. Iterative SCC detection keeps
repeated loads dynamic; pure expressions of immutable inputs remain stable.
Stack argument cells are mutable: selecting an initial ABI pointee does not assert
that a later stack load still contains that pointer. Select its exact saved access
record to follow a particular loaded value. ABI omission does not infer a mapping.

Regressions: analysis `memory_slice` covers bounded predecessor search, SCCs,
joins/loops/conditional writes and overlap; Next `functions::memory_slice` covers
machine-code fixtures, API/CLI exports, unknown calls, killed clobbers, malformed
requests, capacity/work failures and source-free reopening. The PHY scenario checks
exact prologue stack writes/witnesses before and after an ordinary linked call;
its unexpanded tail retains partial CFG status even for the early local witness.

### Reviewed event routes

Three finite native declarations own selector delivery, static callback
registration/delivery and broker subscription. Participants are exact saved
analysis/call/condition/load/store identities, with explicit physical RV32 ABI
words and selector/object/queue/domain fields. Upstream and terminal paths retain
ordered exact hops. No name lookup, legacy pack or implicit analysis participates.

Application shares navigation's physical selection and target keys. A synchronous
borrowed-facts consumer prepares only required observations while one saved record
owner is live, then releases that owner; cross-participant checks retain admitted
values and metadata. Analysis owns local pointer coordinates, branch predicates,
CFG suffixes and callback-store reaching definitions. Knowledge owns finite shape,
conflicts and review; proposal and acceptance authenticate the same physical facts.

The read result separates established, unresolved and mismatched structural checks
from runtime obligations: object lifetime, registration before dispatch, delivery
order, execution context and guard satisfaction. Acceptance requires established
structural bindings and never discharges these temporal obligations. Unknown stack
arguments or dynamic pointers are explicit unresolved evidence, never substituted
with a different ABI word or selected callback. No complete/PASS/execution claim
is inferred from a structurally reviewed asynchronous route.


Service roles (send, receive, registration, invoke, attach and subscribe) are
reviewed interpretations of the explicitly selected callees. Structural argument
checks do not derive those service semantics from their bodies. The result retains
`mechanism-semantics` as an obligation, alongside lifetime/order/context/guard
conditions; runtime consumers must supply the matching reviewed service model.
Selector case checks show that the selected value is consistent with a chosen
branch and a structural suffix to the handler, not that this is its exclusive or
exhaustive runtime dispatch. A suffix cannot change the selected case by revisiting
the same condition. Full-width RV32 masks preserve pointer coordinates; partial
masks never silently become a resolved callback address.


Declaration bounds are 1..16 static dispatch sites, at most 16 hops on each optional
upstream/terminal path, physical ABI words below 64, and 4096 bytes per context or
applicability text. Current saved-call arguments expose words 0..7; higher words
remain unresolved. Selector widths are 1/2/4 bytes. A callback store is exactly four
bytes and must be the unique reaching write before subscription. The broker domain
selector is checked both at attach and subscribe; publish object and selector,
subscriber field and callback entry, callback selector branch and selected handler
remain separate checks. Payload words retain raw saved call inputs; no guessed
payload expression is required.

Preview queries return unresolved/mismatched checks without publication. Both
generic proposal and acceptance reject those checks through the same application
path. Accepted claims retain every participant manifest/fact root, require exact
root analysis evidence and conflict with different routes for the same subject.
`analyses_read` counts one decode per selected participant; a borrowed facts
consumer releases each full stream before the next. Local checks report
`AnalyzeValues`, acquisition reports `LoadResearch`. Work/capacity failure exposes
no partial query or knowledge revision.

Regressions: Next `functions::event_routes` exercises all three declarations,
physical values and cases, callback overwrite/truncation, competing interpretations,
wrong records/fields, cycle rejection, budgets, proposal/review and source-free
exports. A selected case cannot reach its handler by revisiting the same predicate
and taking a different edge on a later iteration. Runtime conditions remain in
all successful query results.
## Register research and source publication

Register discovery reads an explicit saved analysis/publication scope and an
optional frozen knowledge revision. Candidate addresses, instruction access widths
and expression masks are observations, not physical register/field declarations.
Unknown addresses, alternatives, partial analyses and unavailable members remain
visible. A catalogue matches declarations only in their exact source revision and
object applicability; no hardware meaning is inferred from a coincident address.
Proposals and acceptance use the existing knowledge evidence/conflict lifecycle.

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
staged immutable manifest. Original function streams remain immutable; their
transitive saved analysis provenance is retained separately from profile membership.
An evidence-only dependency never silently becomes a selected callee. The normal
supervisor validates/promotes the closure and publishes the result with the run;
capacity, cancellation or validation failure cannot publish an incomplete bundle.
Read/export expands original facts with exact analysis and record identities,
preserving local effects, composed may-effects and their original coverage. Trace
extraction is a separate consumer responsible for its explicit exactness claim.

## Static trace relation

The implemented static trace profile is defined in the
[operator reference](../../next/README.md#static-observable-traces). Domain owns the
request, observable values, blockers and result schema. The RV32 semantic producer
emits typed fences and outgoing tail-call inputs. Analysis owns per-function borrowed
indexes, canonical symbolic expressions and iterative invocation/path traversal;
application acquires the explicitly selected saved profile and releases one side's
facts before loading the other. Only compact observable events and the shared admitted
expression index survive between sides. Store remains unaware of trace semantics.

A trace is conditional on explicit inputs, original immutable-image assumptions and
ordinary integer ABI call/return behavior. Composed may-effects are never treated as
an ordered execution. Unknowns are blockers, not zeroes; all incomplete paths prevent
MATCH. Canonical symbolic equality can prove the selected relation, while undecidable
symbolic inequality remains INCOMPLETE. Return rows/call sites are provenance and are
excluded from the physical MMIO/fence relation. A successful query denotes delivery,
not exactness or termination proof. The same frozen IR and request reproduce the
result after source-free backup/restore. Regression owners are Next
`functions::trace`, linked call/tail tests and the authenticated PHY research scenario.
