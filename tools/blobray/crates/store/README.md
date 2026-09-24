# Blobray store

`blobray-store` owns private capture, verified payloads, metadata and publication.
Its only internal dependency is [domain](../domain/README.md). It does not parse
ELF/AR, select targets, resolve thin paths or install a process supervisor.

Projects require the [current native formats](../../next/README.md#current-formats);
earlier/future formats are rejected without conversion or mutation. Revision
manifests have an independent version.
See [storage diagnosis](../../next/README.md#diagnosis-and-recovery).

`Writer` owns the exclusive import/publication lock. `register` creates a run
identity and owned staging. `register_with_stage_owner` assigns the path to its
capacity owner before filesystem setup, so early cleanup failure stays charged.
`Staging` gives a worker payload access with no SQLite
connection or publication method. Captured sources are copied, hashed and checked
for detectable mutation. `FileLease` owns a verified file handle and exposes
bounded positional reads; it never follows an original input path. The older
materialized `ArtifactLease` helper has a default capacity cap.
The worker validates its revision and closure before issuing a `PreparedImport`.

After the trusted worker is stopped and reaped, `retain_candidate` streams hash
and length verification, promotes referenced payloads, and returns an opaque
`RetainedImport`. `publish_run` consumes that capability, checks the expected base
and commits revision/current/completed-run metadata in one transaction. Ordinary
run updates cannot fabricate completion or rewrite publication fields. Dropping
an uncommitted writer releases the lock without publishing.

The supervisor owns cancellation; storage invokes its checkpoints during streamed
validation. The final metadata transaction either commits fully or rolls back; investigation
child inserts still observe the remaining work/deadline budget.
Recovery requires the writer lock, host owner-identity checks and exclusive
staging leases. It cleans only inactive owned temporary work and marks interrupted
attempts abandoned. Referenced corruption is an integrity error, never permission
to overwrite retained bytes or reopen source paths. Unreferenced CAS objects and
all committed revisions remain retained; garbage collection is not implemented.

`RevisionStream`, `InputStream` and `ObjectStream` assemble schema-1 JSON from
owned temporary record files. No complete revision is required in RAM. Finishing
the stream validates the same ownership relationships as read-side validation
before returning a receipt. Dropping unfinished streams deletes their fragments;
publication authority remains with the coordinator.

`Project::read_inventory` verifies captures and navigates manifest file ranges,
then returns a retained `SnapshotView`. Record callbacks borrow one decoded value
at a time. `doctor_stream` enumerates SQLite rows and revisions without collecting
the project history. Run records use read-only incremental SQLite access: their
size is admitted before a complete TEXT value is loaded. Both take explicit
working memory and cooperative control;
resource exhaustion aborts rather than becoming an integrity finding. Their
no-write/no-repair authority also applies to old revisions. JSON field order is
not part of validity; the stored bytes remain the revision identity.

Materialized `snapshot`, `lease` and `doctor` conveniences have default capacity
admission and retain their synchronous APIs. CLI operations use the streaming
ports and application supervision. Controlled serde I/O restores the original
typed resource or consumer error. See the
[implemented memory contract](../../next/README.md#current-memory-boundary).

[Persisted records](src/records.rs) own `RunRecord`, `OwnerIdentity` and the
`PreparedImport` receipt. Owner fields preserve PID, start ticks and boot identity;
the Linux host interprets them and decides whether the process is alive. Store
does not inspect procfs or implement the worker transport. The application wraps
store readers in restricted capabilities instead of exposing `Project` to read
consumers.

Run schema 33 carries the admitted operation, optional concrete scenario
resolution and scoped `ResultAssessment`. Readers validate one published result,
its assessment identity and scenario shape. Earlier journal formats are rejected.
Recovery records the last valid stage checkpoint before cleanup, while
holding the lease that excludes a live worker. Atomic checkpoint files are
transient diagnostics, not an alternative publication authority.

`ManifestLease` verifies the digest of an explicit manifest file and uses the
same record/relationship reader as project inventory. Its `visit` validates
metadata and reports the revision header without reopening captured payloads;
it deliberately does not certify current payload availability or pin a transitive
closure. Application inspection plans use it for their private immutable manifest
copies. Project inventory and plan reopening still verify retained captures.

`TemporaryBudget` owns one worker's shared logical disk allowance. `TemporaryFile`
admits extension before writes, keeps staged payloads charged after persistence
and refunds fragments only after successful deletion and handle closure. All
capture/manifest fragments share the same authority; copying a fragment accounts
for source and destination coexisting. `TemporaryControl` publishes usage and
physical position through domain ports. Store restores structured capacity and
`disk-full` errors through serde I/O adapters. Application owns policy, aggregate
admission and workspace removal; store does not choose a runtime root.

Supervised workers receive `temporary.json` before writing. Low-level staging
callers without that metadata use an 8 GiB default. Callers sharing a writable
stage must share `TemporaryBudget`; reconstructing it does not scan or account
preexisting files. Coordinator reopening of completed import staging is read-only.
The [temporary storage contract](../../next/README.md#temporary-storage-and-crash-cleanup)
defines the control reserve, logical-byte boundary and retained data exclusions.

`images` owns `ImageLease`, image receipts and publication capabilities.
`retain_image` verifies/promotes the output closure after worker validation;
`publish_image` commits image metadata and completed run together without changing
current revision. `image` verifies the saved manifest, source revision identity
and output digests; `doctor_stream` checks images as well as source revisions.
Store does not repeat ELF interpretation or invoke a linker. `ImageLease` retains
open payload handles; all committed source revisions remain retained. See
[image ownership](../../next/README.md#ownership-limits-and-persistence).

`functions` owns verified `FunctionLease`, `PreparedFunctionReceipt` and opaque
`RetainedFunction`. Publication atomically records the result and completed run;
semantic partial coverage remains distinct from interrupted execution. Its CAS
closure and source revision identity are checked by read/doctor operations.
Store neither interprets instructions nor silently recomputes corrupt results.

Function manifests accept version 7 with typed source, address space, declared
ELF ABI and a semantic producer/value-effect summary. Earlier derived schemas
are unsupported; existing CAS bytes are never rewritten. Function assessment coverage is complete only
when structural coverage and semantic
coverage are both complete; unknown values alone do not imply missing semantics.
The analyses table and publication transaction share the current metadata format.


`investigations` owns `InvestigationLease`, `PreparedInvestigationReceipt` and the
opaque `RetainedInvestigation`. The manifest binds a plan, JSONL membership digest
and coverage counters. Retention verifies every child against the exact request,
producer and payload, without interpreting instructions. `publish_investigation`
streams child analysis rows and commits publication/current-pointer/completed-run
metadata in one transaction. It changes the pointer only for the current source
revision; imports never delete old publications. Failed transactions can leave
unreachable CAS bytes, never visible partial membership. `update_run` cannot
change publication identity. `doctor_stream` checks publication closures too.

`investigation_status` reads source revision and publication pointer in one SQLite
read transaction. Opening a publication verifies its membership and all result
closures; it does not open original inputs or recompute analysis. The JSONL reader
holds at most one 64 KiB record at a time, inside caller-admitted metadata capacity.
See the [library contract](../../next/README.md#library-investigations).

## Review and preservation storage

`knowledge_revisions` indexes immutable event manifests in publication order.
`KnowledgeRevisionId` is separate from source revisions. Expected-base checks,
the inserted decision and the completed durable run share an immediate SQLite
transaction. Retention verifies evidence roots before publication.
`legacy_imports` roots the immutable capture/conversion catalog and original
payloads. Doctor verifies both closures; no cache deletion or GC is implemented.

Backup pins a database read snapshot and streams CAS bytes. Restore builds a
private project and verifies its complete retained closure before application
exposes it. No parsing of ELF/AR or claim acceptance belongs to this store. See
the [wire formats and application ownership](../../next/README.md#knowledge-and-preservation).

Research recipes retain the selected source/companion publications, ABI assumption
and knowledge revision. Function reads validate their immediate CAS roots without
recursively traversing publication/analysis links; doctor separately verifies
registered publications and knowledge history. Knowledge event schema 2 uses the
same input/image source identity. Earlier derived schemas are not converted.


Concrete execution manifests and JSONL evidence use CAS payloads. Schema-10
execution manifests in the existing database carry their publication reference;
`retain_execution` validates the admitted producer/request and evidence structure,
then `publish_execution` commits the result and terminal run atomically. Reads
verify the reference, immutable manifest and payload digests; doctor also checks
stream ordering and summaries. Store does not execute or compute comparison
verdicts. See [concrete execution](../../next/README.md#concrete-execution-and-comparison).

`knowledge_snapshot` verifies a selected immutable event history and evidence
roots once. Its admitted owned entries preserve proposal, review and supersession
states for repeated lookups within one operation; it is not a persistent cache.

`storage_usage` walks logical CAS/metadata/staging sizes under read-only authority.
It includes unreachable objects, does not follow symlinks or run recovery, and
provides no reclaimability estimate or atomic filesystem snapshot guarantee.

Semantic IR builds publish an immutable manifest/index and a `semantic_ir` table
entry in the same transaction as the completed run. Reads use that index to obtain
an admitted journal cell and verify the admitted request, original function streams,
profile counts, frozen knowledge and transitive analysis dependencies. The index
contains no copied function facts; query/export expands original CAS streams.
Doctor checks indexes and completed-run references, and backup/restore retains
both. Retention roots for any future GC must include these transitive dependencies.


Execution validation checks model definition identity, cumulative participation, transcript totals and phase/session closure. Missing or inconsistent observations and MATCH with unmet obligations are rejected. This structural validation never supplies device execution semantics.


External-call trace validation checks binding/response identity, ordered ABI arguments, outputs/allocation/delay/return, consumption counts and lifetime closure. It does not execute code or grant hardware validity to modeled effects.

`execution_tables` groups admitted declarations by frozen knowledge snapshot with
O(n log n) charged sorting and one history load per snapshot per validation call.
It preserves declaration order within each group, validates every selected contract
and target independently, and releases each snapshot before loading the next.
Retention and reopening both use these checks without a persistent cache.
It also validates bounded table
lifecycle evidence: placement identity, initialization/pointer writes, current target
association, counters, conditions and phase/session closure. Reading never resolves
live paths or re-executes callbacks; captured reviews and objects remain project roots.

`execution_services` validates FIFO transitions using working-capacity-admitted
rings, including exact oldest values, full/empty/wake responses, private-stack
ranges, reviewed associations, closure and selected dequeue goals.
`validate_execution_records` requires the caller's `WorkingMemory`; reading never
allocates queue capacity outside the shared budget or executes a guest instruction.

`execution_observation` checks canonical final-memory chunks, complete selected
range coverage, phase ordering, blocked absence and whether selected bytes are
known. Missing chunks or MATCH with selected unknown data is rejected. Difference
descriptors must belong to the explicit per-case relation; comparison algorithms
remain in verification.

`execution_projections` admits immutable accepted layout contracts per operation and
validates selected entry/source applicability. The manifest carries exact resolved
policies; reopening rejects changed/missing review content. A bounded sorted final
field index checks selected-byte knownness without requiring padding to be known.
Unmapped selected timeline records cannot admit MATCH. No projection mutates raw
evidence or resolves current knowledge implicitly.
