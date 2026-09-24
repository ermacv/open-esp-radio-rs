# Target investigation workflows

The following profiles define current support; broader pass composition and
qualification examples below are target contracts unless listed here. The
[command reference](../../next/README.md) owns syntax and format versions.

| Scenario | Status and boundary |
| --- | --- |
| Import → inventory → reopen without originals | Implemented, partial inventory remains explicit |
| Automatic whole-library or saved-plan investigation | Implemented, one application run and atomic publication |
| Link PHY entry with explicit ROM companions → research | Limited RV32 integer static profile, unsupported semantics remain gaps |
| Propose register from analysis → explicit review → research with selected knowledge | Implemented; proposal, review and observation remain distinct |
| Saved MMIO/mask discovery → physical catalogue → review → source-model publication | Implemented; Next evidence and source-owned hardware acceptance remain separate |
| Execute / compare / replay captured implementations | Limited explicit integer scenario profile, scoped MATCH/DIFF/INCOMPLETE |
| Move / backup / restore / recovery | Implemented for supported formats; no conversion or GC |
| Exact data ranges → integer table/constant proposal → review → provenance export | Implemented for captured RV32 ELF bytes; unresolved relocations and analysis gaps remain explicit |
| General equivalence, broader ISA/model support, TUI and cache reclamation | Target, not currently provided |

Concrete scenario orchestration belongs to application. Selection/planning and
execution share one original deadline and work/memory/disk budget. Failure before
publication leaves prior results and current selections intact.

### Contract verification links

| Contract | Implementation | Regression coverage |
| --- | --- | --- |
| Image data symbols agree across generic proposal, review and export | [shared occurrence](../../crates/application/src/occurrence.rs) | `generic_image_table_review_and_export_validate_the_same_physical_symbol` in [linked tests](../../next/tests/linked/mod.rs) |
| Shared child facts survive repeated consumers and release after the final edge | [research](../../crates/application/src/research.rs) | `shared_callee_facts_live_until_the_last_edge_then_release_capacity`; `diamond_research_keeps_shared_leaf_effects_for_both_parents_and_repeated_calls` in [linked tests](../../next/tests/linked/mod.rs) |
| Failed record growth releases the incoming payload without losing old records | [record memory](../../crates/domain/src/record_memory.rs) | `failed_record_growth_rolls_back_payload_and_allows_reuse` in the same module |
| Captured data, known constants and explicit review survive source removal | [data operations](../../crates/application/src/data.rs) | [data regression tests](../../next/tests/functions/data.rs) |
| Partial inventory is partial in summary and handle | [query](../../crates/application/src/query.rs) | `partial_inventory_has_the_same_assessment_in_handle_and_output` in [memory tests](../../next/tests/memory.rs) |
| One object preparation for multiple functions; linear archive indexing | [investigations](../../crates/application/src/investigations.rs) | `automatic_investigation_prepares_each_object_once_and_publishes_one_run`, `archive_lookup_work_grows_with_members_without_restarting_the_cursor` in [investigation tests](../../next/tests/functions/investigations.rs) |
| Section relocation admission supports small extents | [prepared object](../../crates/artifacts/src/function.rs) | `ten_thousand_section_relocations_fit_small_function_capacity` in [function tests](../../next/tests/functions.rs) |
| Original budget and atomic publication span automatic planning | [scenarios](../../crates/application/src/scenarios.rs), [supervisor](../../crates/application/src/jobs.rs) | `automatic_planning_and_execution_share_exhaustion_and_publication_boundary` in [investigation tests](../../next/tests/functions/investigations.rs) |
| One knowledge-history materialization per research | [research](../../crates/application/src/research.rs) | `image_mmio_knowledge_round_trip_has_native_commands_and_retained_evidence` in [linked tests](../../next/tests/linked/mod.rs) |
| Unsupported journal and project formats remain untouched | [store](../../crates/store/src/jobs.rs) | `incompatible_journals_are_rejected_by_all_readers_without_mutation`, `older_project_formats_are_rejected_without_mutation` in [store tests](../../crates/store/src/tests.rs) |
| Phase/counter accounting continues through retention | [resources](../../crates/application/src/resources.rs) | `fixed_measurements_and_phase_costs_survive_worker_handoff` in the same module |

## Workflow contract map

These routes define the user-facing responsibilities of the target system.
An arrow transfers a typed request/result or resource lease, not a database
connection. Existing CLI commands are not the authority for these boundaries.

| User question or action | Operation route and retained result | Governing contract | Acceptance scenarios |
| --- | --- | --- | --- |
| What is in these inputs, including missing/unsupported parts? | import → store capture → artifacts inventory → revision → read query | [Import](contracts.md#import-and-revision-capture), [identity](contracts.md#identity-and-provenance) | A1, A3, A4, A6, A8, R5, R7 |
| What does this occurrence do? | snapshot → plan → analysis with ISA ports → validated publication → query | [Handles](contracts.md#handles-and-capability-boundaries), [passes](contracts.md#analysis-knowledge-and-verification-ports) | A7, K2, I1, I3, R1, R8, R9 |
| Can this archive entry be executed under this environment? | snapshot → link plan → host tool → prepared image → execution session | [Image preparation](contracts.md#artifact-inspection-and-image-preparation) | A2, A5, A7, J1 |
| Can this interpretation be accepted and explained later? | evidence-bearing candidate → knowledge validation → review transaction → retained knowledge revision | [Review](contracts.md#analysis-knowledge-and-verification-ports), [retention](contracts.md#durable-repository-and-disposable-cache) | K1, K4, K5, P3 |
| What remains applicable after an input/model change? | new revision → correspondence/lineage proposals → explicit review → dependency-qualified recomputation | [Identity](contracts.md#identity-and-provenance), [passes](contracts.md#analysis-knowledge-and-verification-ports) | A8, K2, K3 |
| Does the compiled Rust implementation satisfy the declared comparison? | identified pair → prepared images → sessions → verifier → retained evidence | [Comparison](contracts.md#analysis-knowledge-and-verification-ports) | V1, V2, V3, A5 |
| Can I cancel, recover, clean caches or move the project? | supervisor or explicit maintenance/export → retained roots and leased closure → validated outcome | [Jobs](contracts.md#jobs-cancellation-and-failures), [retention](contracts.md#durable-repository-and-disposable-cache) | J1–J5, P1–P6, M1, M2 |

A workflow requires all its relevant authority, resource and coverage contracts,
not just its successful path. Resource containment does not make a wrong symbol
association correct; an atomic publication does not validate its scientific claim.

## Start an investigation

The researcher supplies ordered vendor libraries or linked images, the target
and ABI, optional companion images, and selected chip/ecosystem interpretation
inputs. A generic investigation can begin without reviewed function/interface
packs or a configured Rust replacement.

1. Import copies the inputs into private repository storage and reports their
   identities, roles and capture diagnostics. Original paths remain provenance.
2. The researcher inspects the physical inventory: all objects, member ordinals,
   symbols, sections, relocations, unsupported content and missing thin members.
3. The application records an immutable revision of the selected input set and
   configuration. Missing declarations remain visible research obligations.
4. The researcher selects a question and scope: an occurrence, entry closure,
   interface, register interaction or comparison scenario.
5. Planning reports required operations, exact inputs, reusable computations,
   missing prerequisites and resource budget before execution.

Success means that the revision can still be inspected after the original files
are moved or removed. A missing required artifact blocks the dependent operation
while inspection of available inputs remains possible. A malformed member is an
inventory outcome; it cannot disappear from archive-wide coverage counts.

The initial review documents can be created from observations when useful, but
their existence is not a prerequisite for structural inspection. Reviewed chip
facts and executable models remain optional, explicit interpretation inputs.

## Analyze and inspect results

Starting a plan returns a run handle. The client receives bounded progress and
can inspect the selected revision concurrently. Results are attached to the
revision that the plan captured, even when newer working files exist.

The result identifies analyzed scope, observations, hypotheses, blockers,
coverage and provenance. An investigator can navigate from a semantic subject
to its exact physical occurrences and from a finding to the bytes and producer
that support it. Queries use a retained snapshot and do not trigger analysis,
cache repair or publication as a hidden side effect.

Publication makes the completed result bundle visible in one metadata commit.
Switching to it is an explicit frontend action or completion handling for the
same selected revision. An older snapshot remains readable. A failed run leaves
the previous completed publication available and explains which operation failed.

Repeated requests can reuse computations when their dependency identities match.
The plan explains reuse or invalidation in terms of changed inputs, parameters,
providers or producers. A result without a reproducible dependency identity is
not made persistent merely to avoid repeated work.

## Build and execute an archive entry

The caller selects the exact entry occurrence or resolves an ambiguous name from
the physical candidates. Inventory and candidate relationships remain available
before a link plan exists. Supplying several libraries does not itself prove
their order or selection in an original firmware build.

The caller chooses an existing linked image or an explicit synthetic link recipe.
The recipe records library order, roots, companion definitions, target/ABI,
linker identity and options. The application prepares an image once and passes
the resulting image/mapping to execution, replay or comparison.

Companion data and call definitions participate in image preparation. Indirect
callback roots are explicit inputs. Unresolved relocations, conflicting layouts
and unknown source mappings remain visible and block claims that require them.
There is no path where a late companion attachment silently changes the meaning
of an already-prepared image.

Success means the image and executed recipe are retained with the result, the
selected source occurrences can be traced where known, and all execution entry
points apply the same preparation rules. Synthetic placement is labeled in every
export that exposes addresses.

## Review and preserve knowledge

The researcher proposes an assertion over a physical occurrence or semantic
subject, supplies its applicability, and references supporting evidence. Examples
include a recovered function boundary, interface signature, register meaning,
data layout or a correspondence between library revisions.

Validation checks that the evidence exists, the subject is unambiguous, the claim
does not exceed its evidence class, and the assertion is consistent with other
accepted assertions. The reviewer accepts a specific candidate against a specific
base revision. The commit creates a new knowledge revision and retains the full
evidence dependency closure. If the base changed, the application returns a
conflict with both revisions rather than overwriting another decision.

Code, function, interface and register assertions keep their own validation
rules. They share the transaction, provenance and retention mechanism. An
accepted display name does not become a physical symbol identity, and acceptance
does not erase a contradictory observation.

Success means accepted knowledge can be explained and exported after clearing
the computational cache. Required recovered hardware tables and calibration
coefficients retain source identity, purpose, representation and applicability
under the [source policy](../../../../docs/source-policy.md). Their binary origin
does not justify dropping them or substituting another profile.

## Recover a table or coefficient

The researcher selects an exact occurrence and sized symbol or explicit section/
image range. Application borrows all ranges from one artifacts-owned prepared
object and streams captured bytes, relocations and supporting analysis records.
The selected analyses retain their original scope and gaps. No hidden analysis
or preferred resolution of duplicate names occurs.

A table proposal supplies integer encoding, count, stride, purpose and applicability.
A coefficient proposal instead names an exact known operand in a saved analysis.
Knowledge validates the claim shape; application checks the physical evidence on
proposal and review. Only explicit review produces an accepted interpretation.
A table with relocations can have a reviewed layout while numeric values remain
unresolved. Writable bytes describe initialization, not runtime state.

Success means an export at the selected knowledge revision preserves the exact
object, data ranges or instruction evidence, layout/constant interpretation and
provenance after source deletion, move and restore. The export never presents
an instruction-derived coefficient as a contiguous table in the binary.

## Compare a Rust replacement

The caller selects identified compiled vendor and Rust artifacts, their entry
occurrences, explicit environment/scenario inputs, a comparison relation and the
requested claim scope. A generated reference, shared production core and exact
production entry remain different evidence classes.

The application prepares both images through the common image operation and
creates the declared execution sessions. Cold phases start fresh; warm successors
retain only session-owned state. Each invocation declares its own entry. Executable models
record their selected implementation and applicability in the evidence.

Verification compares observations and coverage. `MATCH` answers the declared
relation over its stated scope; `DIFF` retains the counterexample; `INCOMPLETE`
retains missing obligations. None of these outcomes is silently relabeled a
product readiness result. Clients can inspect valid incomplete research and
choose the next question without editing a status file to make it load.

Success means the retained evidence names both compiled implementations, scenario,
models, producer, comparison policy, verdict and claim ceiling. The independent
qualification evaluator decides whether that evidence is sufficient for its own
requirements.

## Update a vendor library or interpretation input

Importing changed libraries creates a new revision with its own physical
identities. Reviewed interpretation changes create a separate knowledge revision.
ABI/model selections belong to analysis or execution recipes; changing them does
not manufacture a new source revision when captured inputs are unchanged.
Correspondence and automatic transfer below remain target behavior.

Correspondence analysis proposes associations between old and new occurrences.
Lineage composes supported relationships across several revisions. The result
distinguishes confirmed evidence, ambiguous candidates, conflicts and unmatched
entities. Knowledge review decides which assertions can be transferred under
their applicability; behavioral evidence remains bound to its original context.

Planning invalidates only computations whose semantic dependencies changed. A
rename does not invalidate unrelated structural byte facts; an executable model
change invalidates consumers of that model even when its human label is unchanged.
An old result can still be opened explicitly without being shown as current proof.

Success means no manual repair of generated files is necessary to preserve the
old investigation or establish the new one. The user can explain each retained,
recomputed and unresolved result.

## Cancel, close and recover

Cancellation is a request to the application supervisor, independent of whether
it comes from CLI signals, TUI, or an embedded client. The supervisor stops new
work, cooperatively cancels workers, terminates owned child processes where
necessary, reaps them and releases staging within its shutdown contract.

An operation cancelled before commit does not advance publication. An operation
that already committed reports completion. Closing the application's frontend
does not detach a worker whose resource lifetime has no remaining owner.

After a crash, opening the repository reconciles abandoned operations before
admitting a new writer. Read-only inspection never performs that reconciliation
implicitly; it either reads a committed snapshot or returns an explicit recovery
requirement. A subsequent run can reuse verified completed computations.

Next query/Plan admission reconciles its private runtime root independently of
project recovery: dead owner, inactive lease and empty containment are all
required before deleting an identified workspace. Live or unverifiable entries
remain with bounded diagnostics. This path cannot delete saved Plans, exports or
project revisions. The [temporary storage contract](../../next/README.md#temporary-storage-and-crash-cleanup)
also governs aggregate admission and retained-result lifetime.

Disk exhaustion reports protected data and the failed operation. Corrupt retained
evidence reports an integrity failure and supports restoration from a known backup;
it is not silently regenerated under today's analyzer. Damaged disposable cache
data can be discarded and recomputed with a diagnostic.

Success means a previous coherent publication remains accessible, no interrupted
run appears complete, and recovery does not require deleting unknown database,
lock or pack files by hand.

## Export, back up and move an investigation

Research export and private backup have separate purposes. A research export
contains selected knowledge/evidence and provenance; it does not include private
binary bytes unless the caller explicitly selects a private bundle. A complete
backup includes retained revisions, their source/evidence closure and a manifest
of object identities. Neither operation uploads data implicitly.

Backup reads a fixed snapshot and retains its objects while streaming them. The
completed bundle is verified before it becomes the backup destination. Restore
imports into a separate destination, validates every referenced object and only
then exposes the restored project. A failed restore leaves the source and any
existing destination project intact.

Project and occurrence identities survive moving the repository. Origin paths
remain descriptive provenance and need not exist on the destination machine.
Reading results requires a supported record schema. Re-execution additionally
requires the recorded tool/model implementations and reports missing dependencies.

Downstream SVD/PAC and reference-code generators consume a selected validated
snapshot or versioned export. Their outputs retain links to the knowledge and
evidence used. Generator failure does not invalidate the research publication,
and generated code does not acquire a stronger proof class through publication.

## Preserve existing investigations

Compatibility preserves valuable data and its meaning; existing commands and
private Rust APIs need not retain their shape. A legacy import adapter runs
against a captured source project and creates a separate target repository.

The import inventory includes reviewed packs, provenance/schema inputs,
revision identities, saved correspondence/lineage, compiled-input bindings,
comparison evidence, publication manifests and referenced generated payloads.
Required tables and coefficients are included with their original provenance.
The adapter follows dependency references, including evidence retained by the
legacy cache, rather than assuming that everything called a cache is disposable.

Each source record receives one explicit outcome:

| Outcome | Preservation and use |
| --- | --- |
| Converted and validated | New typed record retains original identity, original bytes and an explicit mapping |
| Preserved but unresolved | Original bytes/provenance remain available; missing dependencies or ambiguous mapping block affected claims |
| Unsupported representation | Opaque original record is retained with its format identity and diagnostic; it does not become an accepted assertion |
| Missing source payload | Manifest retains the original reference and missing identity; affected proof remains unavailable |

No entry is silently omitted. The import report maps records and counts their
outcomes; it is an operation artifact in ignored output storage, not a tracked
migration diary. The old project remains usable with the old tool. A new current
publication is exposed only after validating the imported closure and its stated
limitations. Operator review is required to resolve uncertain semantic mappings.

## Acceptance scenarios

These are required behavioral checks for a future implementation, not a record
of tests already run. Synthetic ELF/AR fixtures exercise contracts without vendor
inputs. Real vendor reproductions remain private and use the resource-limited
host. Tests assert observable behavior and ownership rather than internal file
layout or generated register constants.

| ID | Scenario | Required observable result | Responsible boundary |
| --- | --- | --- | --- |
| A1 | Two archive members have equal names and bytes; symbols repeat across tables | Every occurrence remains selectable; unqualified selection reports ambiguity | artifacts, domain |
| A2 | Weak/common definitions, duplicate exports, library reordering and cyclic archive references | Inventory remains unchanged; selected image and recipe reflect the actual chosen linker semantics | image preparation |
| A3 | Thin archive with an external member; then delete its original directory | Imported member remains usable; an uncaptured member is an explicit gap | import, artifacts |
| A4 | Mixed or malformed archive members | Every payload occurrence has an outcome; supported subsets cannot claim complete archive coverage | artifacts, analysis |
| A5 | Companion supplies data relocation and callback definitions | Run, replay and comparison prepare equivalent images for the same recipe | application, backend |
| A6 | Source changes during import or after revision creation | Detectable capture change/expected-digest mismatch rejects capture; committed revision always uses its imported bytes | import, store |
| A7 | Required source mapping or relocation is unknown | Affected claim is incomplete, never inferred from an equal label or placeholder value | artifacts, verification |
| A8 | Import unchanged thin-container bytes with a changed external member | Old and new snapshots retain their own payload bindings; equal occurrence selectors do not reuse stale analysis or transfer evidence | domain, planning, store |
| K1 | Clear all computational cache after accepting reviewed knowledge | Assertions, cited evidence and captured inputs remain readable | store, knowledge |
| K2 | Change one assertion or selected model | Only dependent computations become stale, with an explanation; old evidence keeps its identity | planning, analysis |
| K3 | Rebase between renamed/changed/ambiguous functions | Supported associations are explicit; uncertainty blocks automatic proof transfer | analysis, knowledge |
| K4 | Two reviews use the same base revision | First commit succeeds; second reports conflict without lost decisions | application, store |
| K5 | An analysis proposes an assertion, then its candidate or evidence changes before review commit | Analysis cannot accept it; commit validates the exact reviewed candidate and retained supporting closure | knowledge, application, store |
| P1 | Inject failure before payload sync, before metadata commit and after commit | Readers see the previous or new complete publication; no mixed result bundle | store |
| P2 | Hold an old reader while publishing, compacting and pruning | Reader retains its selected content; active and persisted roots remain protected | store |
| P3 | Corrupt cache-only data versus referenced evidence | Cache miss/recompute and retained-evidence integrity failure remain distinct outcomes | store |
| P4 | Exhaust disk quota with protected content | Write fails without evicting accepted evidence or replacing current publication | store |
| P5 | Export files are deleted or changed | Snapshot queries still read retained content; export can be recreated | application, store |
| P6 | Exhaust temporary-output quota or disk while spooling a query, manifest or image | Typed failure, prior publication intact, owned residue identified, no eviction of retained evidence | application, store, host |
| J1 | Cancel CPU analysis, blocked external tool and TUI-owned comparison | Bounded cleanup, descendant reaping, no unmanaged worker or false publication | supervisor, host |
| J2 | Race cancellation or project revision change with commit | One explicit terminal outcome; stale run cannot replace a newer revision's current result | application, store |
| J3 | Kill a writer and restart | Interrupted attempt is abandoned; committed evidence survives; verified work can be reused | recovery |
| J4 | Drop all client handles or close the frontend during import, query or execution | Supervisor retains ownership through terminal cleanup; shutdown rejects new work and does not detach workers | application, host |
| J5 | Query succeeds but destination fails; cancellation races with commit admission | Delivery failure remains distinct from computation; accepted cancellation prevents commit and rejected cancellation reports actual commit outcome | application, store, host |
| R1 | Exhaust work in an ELF table, member iteration or long name | Typed resource failure retains exact last context; current is unchanged | application, artifacts |
| R2 | Cancel or expire deadline inside a heavy operation | Cooperative stop, with forced cleanup for uncooperative code; budget never restarts between phases | application, host |
| R3 | Worker aborts, receives a signal, hits cgroup OOM or floods stderr | Bounded diagnostics distinguish observations from inferred causes | host |
| R4 | Cleanup fails after an earlier failure or completed commit | Primary cause and committed success survive; cleanup is secondary | application, store |
| R5 | Reopen large inventory or run doctor under a small working-memory budget | Bounded read-only processing without hidden writes or partial success | store, application |
| R6 | Exhaust scoped scratch, unwind a phase, then start another | Typed capacity failure; no escaped reference or stale ID; temporary capacity is reusable | computing module |
| R7 | Process many thin members and a large result | Input leases and temporary memory end when no longer needed; staged output avoids aggregate RAM growth | application, store, artifacts |
| R8 | Repeat with the same work policy and budget; overflow a counter | Deterministic accounting and checked arithmetic; no budget reset or wraparound | application |
| R9 | Cyclic CFG/call graph or nonconvergent dataflow; a cycle in pass dependencies | Program traversal is iterative and bounded with declared completeness; scheduler dependency cycle is rejected before execution | analysis, planning |
| R10 | A sink retains records, a phase grows a buffer, or a child worker starts | Simultaneous allocations and result lifetimes remain charged; capacity/work cannot be duplicated or reset | consumer, application |
| R11 | Exhaust computation capacity while emitting diagnostics; request a claimed allocation-controlled path | Fixed-size failure remains available; only independently checked paths claim absence of hidden allocation | computing module, host |
| V1 | Missing behavior, known difference and fully discharged comparison | Typed INCOMPLETE, DIFF and MATCH with their coverage and claim scopes | verification |
| V2 | Generated reference or model stands in for production behavior | Evidence keeps its limited class; exact production equivalence is not asserted | verification |
| V3 | Stateful phase is incomplete, or independent cases run in a different order | Dependent phases cannot consume unknown state; independent cases have fresh session state and no cross-case leakage | executor, verification |
| I1 | Issue equivalent requests through API, CLI/JSON and TUI | Same selection, revision, results and diagnostics; only presentation differs | application, frontends |
| I2 | Create two applications with different provider sets | Independent behavior and identities without process-global interference | host composition |
| I3 | Open an absent cache or query an existing snapshot | No hidden analysis, repair, migration or writer acquisition | query interface |
| M1 | Import legacy data with unknown schema, missing dependency or ambiguous identity | Every record has a preservation outcome; original project remains unchanged | legacy adapter |
| M2 | Backup, move and restore; original input paths are unavailable | Retained closure verifies and remains readable; missing replay tools are reported | store, application |
| B1 | Build generic Blobray without open-radio-specific providers | No generic dependency on production code, chip hosts or qualification policy | crate dependency checks |
| B2 | Attempt a forbidden dependency or mutation through a read handle | Dependency checks or type boundary reject it | architecture and API checks |
| B3 | Pass a snapshot/analysis port to code attempting writer acquisition, tool discovery or provider installation | Public capabilities do not expose those operations; compile-fail API checks reject authority escalation | application, domain, store |
| B4 | Start a planned operation after inputs/providers/current selection change | It uses retained inputs and identified implementations or returns explicit unavailability; no implicit replanning or fallback | application, host |

## Current inspection reports

**Implemented:** `coverage --id PUBLICATION` joins the immutable membership stream
with captured inventory, retaining only one object's ranges at a time. Aliases and
overlapping extents count once. The publication's selected-function completeness
and executable intervals outside that selection are separate observations.
`storage-usage` reports accumulated logical storage without changing the project.

**Implemented within the current format:** backup/restore, project move, reopening
without source files, and recreating accepted-data exports. Other database or
journal versions are rejected unchanged. No old-reader packaging, upgrade or
compatibility reader is provided. Exports retain bytes and provenance; they do not
promise automatic import into another format.

**Target extensions:** cross-revision correspondence/rebase, cache invalidation,
pruning with transitive retention pins, general provider sets and TUI remain
unsupported. Acceptance rows referring to those capabilities are target
obligations, not prerequisites of the current PHY workflow. In particular K1–K3,
P2–P3 and I2 do not describe current callable maintenance/planning operations.

## Contract enforcement

Crate-graph checks enforce dependency direction and standalone composition. They
cannot establish module authority, lifetime safety or recovery behavior. Compile-
fail API tests cover forbidden capabilities and escaping scratch/image borrows;
behavioral tests cover publication, cancellation, retention and claim semantics.
Failure injection targets capture, output staging, payload durability, commit
admission, metadata commit and terminal cleanup separately.

Capability tests establish what the supplied API permits, not an OS security
sandbox: code linked with `std` can still attempt ambient filesystem or process
access. Source/dependency review must reject such hidden access in computation
modules; host containment covers resource failures, not arbitrary hostile plugins.

Tests use synthetic AR/ELF inputs for structural and lifecycle contracts. A large
unsupported payload tests streaming inspection, not an ELF allocation limit;
memory-limit checks need a supported object that exercises the actual workspace.
Nested/compressed formats not supported by a parser test explicit coverage gaps,
not nonexistent decompression behavior. Real vendor cases supplement these tests
without replacing them or entering tracked fixtures.

An operation is conforming only when its public API, every frontend adapter and
embedded entry point meet the same semantic contract. Lower-level streaming APIs
publish their caller obligations explicitly. A completed import/inventory path
does not establish readiness of linking, review, retention or comparison.

The design is complete only when each operation above maps to a component and a
resource owner in the linked contracts. Implementation acceptance additionally
requires these behavioral tests, published API/schema documentation, standalone
composition checks and relevant repository architecture checks. Documentation
validation alone establishes neither those runtime guarantees nor hardware
qualification.


## Captured interface observation and review (implemented profile)

1. Select a retained analysis or exact captured pointer-table span. Run the shared
   `interfaces` read query with explicit ABI where argument roots require it.
2. Inspect physical root, dereference/index path, slot and evidence record. Missing
   provenance or unsupported expressions remain issues; numeric targets do not
   establish table ownership. Null/external/unresolved pointer values stay distinct.
3. Propose a native interface contract with reviewed layout and preconditions.
   Leave signature/semantic binding unknown when evidence only establishes a slot.
   Validate and review through the ordinary knowledge lifecycle.
4. Query with the exact accepted knowledge revision. Inspect candidate states,
   ambiguity and unverified runtime conditions. Matching never executes a model.
5. Export observations and preserve the project; source removal and backup/restore
   retain identical query identities and content. The real PHY scenario checks a ROM
   global-pointer/slot path with independently established instruction operands.

This structural scenario does not claim callback execution or hardware behavior.
Runtime interface placement and callback execution use the separate explicit
scenarios below. Neither is a read query success condition.

## Reviewed runtime callback (implemented profile)

Select an accepted interface assertion and its exact knowledge revision. Supply
`Invocation.tables` with that selection, a fresh normal-memory range, exact slot
addresses and existing writable pointer cells. Select captured callback bytes or
an explicit call model with reviewed semantic/signature metadata. Run through the
shared `execute`/`compare` operation.

Inspect both code outcome and runtime-table lifecycle: initialization, conditions,
current-target association, writes and closure. Unknown/ambiguous targets or failed
guards are incomplete. Warm phases retain only session instances; cold phases
start fresh. Preserve the project and use retained reads/replay after source
removal. The [runtime interface contract](contracts.md#runtime-interface-instances)
defines exact ownership and the value-association claim.


## Function and argument-context review (implemented profile)

Select a captured function symbol or explicit executable range. Propose its native
function contract with retained evidence, known or unknown signature, argument
contexts, field roles and caller preconditions. Validate through the shared read
query, then apply/review against an explicit knowledge base. Conflicting accepted
interpretations require explicit supersession; invalid physical identities or
contradictory layout/predicates publish no knowledge revision.

Query/export the accepted revision after source removal. Review preserves the
interpretation and never rewrites the saved function analysis or supplies observed
runtime preconditions. Field navigation and route witnesses consume explicit
reviewed declarations through their own query contracts; declaration alone does
not assert a call path, executable route or behavioral equivalence.


### Selected research navigation

Implemented: select a revision, saved publications/analyses and optional knowledge;
list functions and declared contracts; inspect callers/callees or object readers/writers;
select an accepted context declaration and inspect observed field accesses; export
and reopen the same query after source removal. `navigate` shares the application
read lifecycle and resource budget. Missing/partial facts and ambiguous addresses
remain visible. No read performs hidden planning, linking or analysis, and no empty
selection proves that the entire firmware lacks an effect. Structural flow/effect inventory and ordered path review use `flow` and the native
path claim. `memory-slice` inspects local RAM definitions at publication.
`event-route` validates reviewed conditional asynchronous routes against saved facts.


### Saved structural path review

Implemented: select a root and target analysis; inspect the returned structural
predecessor hops and frontiers; propose an exact native path with root evidence;
validate/review every selected physical call; export the accepted knowledge and
flow query. Review rejects an ambiguous selected step instead of choosing an
interpretation. The same source-free project can produce a reachable memory-effect
inventory with original local/composed provenance and explicit unknown addresses.
These results do not establish event delivery, feasible execution or comparison.


### Saved RAM definitions

Implemented: select one retained analysis and an exact local call/store/transfer
anchor, then discover preceding writes or select a known address/access span.
Inspect incoming state, last definitions, source facts, CFG witnesses and barriers;
export the query and reopen it after removing source files or restoring a backup.
Joining paths, loops, unknown aliases/calls and partial coverage remain explicit.
The slice never launches analysis or turns composed may-effects into RAM state.


### Conditional event-route review

Implemented: choose exact dispatch, registration/delivery or domain/subscription
calls and the saved field/selector/callback evidence. Inspect `event-route`
checks, correct unresolved or mismatched bindings, then use ordinary knowledge
proposal and acceptance. Query/export the same declaration after source removal;
knowledge export preserves its participant identities. Selector delivery, static
callbacks and broker subscriptions each have their own finite declaration.

Acceptance authenticates the physical structure and the reviewed assignment of
service roles. Mechanism semantics, object lifetime, delivery/context/guards and
registration order remain explicit obligations for a runtime consumer. Static
review alone cannot complete an asynchronous execution scenario.
## Register discovery and source publication

Select saved analyses/publications and, optionally, a frozen knowledge revision.
`registers` reports the selected coverage, unknown/alternative addresses, local and
composed observations, expression masks and applicable declarations/conflicts. The
same API exports its complete typed result without reacquiring original binaries.
Use exact analysis records as proposal evidence and supply physical widths/fields
explicitly. Acceptance preserves occurrence applicability; it does not turn an
instruction access width into physical geometry or establish hardware behavior.

To publish source, the independent register tool initializes explicit empty
peripherals or imports retained CMSIS-SVD into an unreviewed native model. A source
review supplies canonical physical identities, evidence, applicability and hardware
semantics in reviewed packs. Validation composes those with the selected model,
memory/ownership and PAC policies, then generates all four outputs. There is no
automatic Next-to-hardware acceptance or binary-derived write-semantics fallback.
See the [source-authoring commands](../../../registers/README.md) and
[saved register reference](../../next/README.md#saved-register-research).

Regression owners are Next `functions/registers.rs` (review, conflicts, limits,
source-free export and restore), analysis `registers` (mask bounds), application
`registers::index` (applicability and retired claims) and register tool `drafts`
(initialization/import, explicit review and four-output publication).

## Saved semantic IR build (implemented)

Select frozen publications/analyses and optional knowledge, then configure named
all/prefix/exact-analysis roots and resolved call closure. One application build
publishes an immutable IR identity. Show/export streams the original facts with
profile membership, partial coverage, unresolved links and transitive provenance.
Deleting origins, moving the project and restoring a backup preserve that identity.
A successful build means the configured bundle was retained; static-trace exactness
and concrete execution are separate consumer claims. Details and commands:
[IR profiles](../../next/README.md#saved-semantic-ir-profiles).

## Static trace comparison (implemented, bounded profile)

Build an IR profile from original local publications, select exact function entries,
supply explicit register inputs and physical observation ranges, then query `trace`.
Read the path blockers and exactness independently from operation completion. Compare
ordered MMIO/fence events only; return/RAM/call relations belong to other profiles.
Export the result and retain its project backup to reproduce the same trace without
source binaries. [Static trace scope and assumptions](../../next/README.md#static-observable-traces)
define MATCH/DIFF/INCOMPLETE and the distinction from composed research and concrete
execution.


The implemented device workflow declares exact ports and applicability in the shared
execution request, executes cold/warm phases, reads code and model outcomes, then
replays the retained request after source removal or backup/restore. Sequence/FIFO
obligations close at the declared lifetime; code return cannot turn missing model
participation into MATCH. The [device regressions](../../next/tests/execution/devices.rs)
cover all eight mechanisms, ownership conflicts, closure, forged records and restored
replay. [Memory tests](../../crates/application/src/devices.rs) verify release of model
payload/state and cancellation before response consumption.


The implemented external-call scenario composes admitted allocation, a warm-phase
read, an explicit call response, a device sequence and modeled delays in one
execution. [Call regressions](../../next/tests/execution/calls.rs) check the actual
return and participation, all three verdicts, output ownership, stack words,
unknown values, response exhaustion, early goals, resource failure and identical
API/CLI replay after source removal and backup/restore. Model effects retain their
conditional scope; hardware qualification and reviewed service dispatch are separate.


## Reviewed FIFO service scenario (implemented profile)

Select accepted interface contracts and place their explicit `service` slots in
an execution request. Declare bounded FIFO owners with handle, item width,
initial items and phase/session lifetime; bind enqueue/dequeue/length to exact
reviewed table slots and call boundaries. Supply physical ABI arguments and
private-stack input/output pointers in the captured caller.

Run setup and action phases through the shared execution API/CLI. Inspect queue
transitions, known inputs/outputs, wake values and closure alongside code outcomes.
Use `observe-dequeue` for a selected successful service event; use `return` when
the caller must finish. Full/empty are declared responses, while invalid pointers,
handles and unknown required values are incomplete. Warm continuation cannot hide
a failed phase. Preserve the project and reopen/replay without source files.
These FIFO mechanisms do not execute an RTOS or qualify scheduler/hardware behavior.

## Selected final-state comparison (implemented profile)

Declare `observe_memory` on each invocation, including exact named address ranges.
Select per-case `relation` with return words, event channels and memory pair indices.
Use different physical addresses only through an explicit equal-length pair; this
scenario does not infer a layout mapping. Run the shared comparison operation and
inspect its typed difference or selected unknown/unavailable bytes.

All selected bytes, including unchanged data, survive retained reads and replay.
Excluded observations remain available. An incomplete code phase's RAM snapshot
is intermediate evidence; successful execution coverage alone does not prove a
comparison when required outputs are unknown. Preservation and reopening use the
same project backup/restore lifecycle as other execution evidence.

### Compare physical call boundaries (implemented)

Set each invocation's `observe_calls` to an explicit word/tail profile and select
`relation.calls: true`. For exact comparison use the identical profile on both
sides; target overrides select words only for that physical destination. Execute
or compare through the shared application scenario, then read the retained
`call-transfer`/`transfer-argument` groups and typed difference. A zero-word profile
compares target order without claiming argument equivalence. Source-free project
backup/restore and replay preserve the same evidence and producer identity.

Compare call groups together with the required MMIO/fence/delay channels to retain
their relative order. Unknown selected words yield INCOMPLETE. `observe-call`
goals stop after boundary capture and before dispatch, so that prefix never proves
callee-body behavior. Exact physical comparison does not map renamed or relocated
semantic operations; select reviewed correspondence explicitly for that scenario.

### Compare reviewed call operations (implemented)

Create a call-pair proposal through `knowledge propose-call-pair --request` or
`Application::start_propose_call_pair`; the generic `KnowledgeAction::Propose`
path accepts the same `call-pair` claim and enforces the same physical checks.
Each endpoint names its captured context and exact code symbol/address or explicit
model/service binding identity. Review the proposal with the ordinary knowledge
accept/reject action. A proposed or rejected assertion cannot authorize comparison.

Select accepted `knowledge`/`assertion` references in each case's `reviewed_calls`,
choose exact/selected/ignored physical words in the reviewed claim, and explicitly
choose exact or excluded unlisted calls. Execute/compare with `calls: false`.
Read `manifest.call_pairs` and raw calls beside the verdict. Different addresses
can correspond only through the selected pair. Changed source/definition or
ambiguous selected endpoints fail before publication. Reading and replaying the
retained result after source removal/restore uses the same frozen reviews.

### Compare the internal physical timeline (implemented)

Set invocation `observe_timeline` flags for required normal reads/writes, atomics
and conditional branches. Select the corresponding `events.timeline` flags in the
case relation; both sides must capture every selected channel. Use execute/compare
and the ordinary execution/query/replay lifecycle. All existing call/MMIO/fence/
delay selections preserve their order relative to selected internal observations.

A final-memory match can coexist with a timeline difference: an intermediate write,
load order, atomic outcome/order or branch choice may differ despite identical final
bytes and return. Excluded raw observations remain readable. Unknown/inaccessible
selected reads cannot MATCH. Model/service memory effects retain their explicit
assumption provenance; setup and inspection are excluded. Physical branch sites
compare exactly; reviewed cross-layout/ABI/control correspondence remains the next
profile, with no automatic normalization.

### Reviewed layout comparison — implemented finite profile

Capture both linked entries → propose explicit paired fields/branches (or call ABI
word positions) → review → select the exact accepted snapshot in the comparison.
Capture requested final ranges/timeline channels and execute through the shared
application workflow. Compare keeps raw physical observations and explicit unknowns;
unmapped selected effects cannot MATCH. Different addresses/word positions can match
only under the selected mapping. Query, move, backup/restore and replay preserve
that frozen scope independently of subsequent knowledge changes. Arbitrary type,
pointer-value and dynamic-path conversions are not part of this profile.

### Reviewed effect refinement — implemented finite profile

Capture both compiled inputs → identify exact root entries → propose explicit
required/omitted/replaced/added or forbidden MMIO/delay/fence rules → review →
select the accepted effect assertion with the desired call, layout, timeline,
return and final-memory relations → execute/compare → inspect claim ceiling and
remaining obligations → preserve/reopen/replay the same research.

All raw effects remain evidence. A policy may deliberately relax physical
observations only under the reviewed refinement ceiling; it cannot discharge
unknown classification, missing required exercise or unfinished execution.
[Effect scenarios](../../next/tests/execution/effects.rs) exercise generic and
specialized proposal, CLI review, conflicting/stale selection, combined relations,
failed-run atomicity and source-free restore/replay.
