# Resources, storage and recovery

Operate bounded jobs, inspect resource use and recover or preserve retained projects.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Coverage and storage observations

```console
cargo blobray coverage --project /path/to/research --id PUBLICATION --limit-mode watchdog --format json
cargo blobray storage-usage --project /path/to/research --limit-mode watchdog --format json
```

`coverage` preserves the publication's assessment and reports executable sections,
selected extent unions and outside intervals in section-relative coordinates.
Aliases count once; malformed/unavailable structure and invalid extents produce
explicit unknowns. These intervals do not establish which bytes are code or
whether an unselected region accesses a register. `storage-usage` returns CAS,
metadata and staging logical sizes and metadata row counts; it neither follows
symlinks nor estimates reclaimable space. Filesystem sizes are not an atomic
snapshot. Both operations use the common read-query lifecycle and budgets.

See [resource and ownership contracts](../../../docs/design/contracts.md#research-memory-and-read-only-observations)
for research phase lifetimes, admitted memory diagnostics and storage semantics.

## Preparation and measurements

Whole-library execution groups consecutive functions by exact captured occurrence.
Application owns a bounded ordinal-to-payload index for each archive and borrows
function views from one prepared object. Artifact preparation reads, hashes and
parses the object once, shares target names and prepares each selected section
once. Analysis owns sorted relocation/normalization indexes; physical relocation
identities and nonadjacent HI/LO pairing remain intact. Every object scope releases
its buffers before the next object. There is no process-wide cache.

Research reads each selected publication once into an admitted address index,
loads each image's companion bindings once, and verifies the frozen knowledge
history once. Exact registers and overlapping MMIO regions use local indexes;
blocked/ambiguous call targets remain unresolved. Temporary data cannot outlive
the operation's `WorkingMemory` authority.

`run.diagnostics.progress.measurements` contains fixed counters for archive
entries, prepared objects/sections, object bytes read/hashed, relocation lookups,
publication passes and knowledge-history passes. `phases` records cumulative
elapsed milliseconds and work units by phase, including coordinator retention.
Independent coordinator validation counts as work; counters are not result
identity. `working_memory` reports admitted requested capacity, not RSS.
Host RSS and cgroup observations remain separate. Default working capacity is
256 MiB; no automatic limit increase or reduced scope occurs on exhaustion.

## Cooperative control and failure diagnostics

`RunControl` is the domain port supplied to artifact inspection and controlled
storage operations. `RunContext` in application owns a checked work counter and
an absolute deadline using the injected host's monotonic clock. The worker and
coordinator share that deadline and transfer charged work through the bounded
worker report; retention never receives a fresh budget. Libraries install no
signal handlers. The private worker host converts SIGTERM/SIGINT into cooperative
cancellation, while the guard retains forced cleanup after the grace period.
Every checkpoint checks cancellation and the exact work limit. The clock, the
deadline and progress publication are sampled on a context's first checkpoint
and then at least every 32 checkpoints or 4096 work units, so hot loops do not
read the clock per step and zero-unit waiting loops still reach the deadline.
Temporary-storage usage is reported on the same stride; the budget itself
enforces capacity on every allocation.

Work policy 1 charges one unit per visited structural record and one per block of
up to 4096 bytes for each read, hash, copy, delimiter scan or serialization output
operation. Repeated processing is charged again. Constant-time borrowed-range
access performs a zero-cost checkpoint. Charges are admission costs before work,
not CPU instruction counts, and are not refunded after an I/O error. Accounting
also includes closure-record/framing visits. Repeated execution with the same
inputs, retained objects and policy has the same work cost; reusing an existing
object requires verification and can change the cost compared with an empty store.
JSON output, execution evidence and query record spools are buffered into
bounded writes. ELF string scans, explicit table
loops, capture, hashing and coordinator reads have cooperative checkpoints.
Opaque third-party calls, allocator operations, filesystem calls and final sync
are not promised to be interruptible inside the call; the owned worker remains
necessary for failures outside cooperative control.

The phases are starting, capture, read-captured, members, elf, validate-revision,
serialize, retain and publish. Progress records carry physical input/member/table/
entry ordinals and available artifact digests, charged work, elapsed time, a
sequence and an optional fixed-size stop reason/request. Context unavailable in a
phase is null. Progress is the last observation, not a claim about the exact crash
instruction. Checkpoints are atomically replaced in staging at most every 100 ms
per producer, with a final checkpoint at termination. Recovery reads the last
valid checkpoint under the staging lease before removing that directory. These
transient checkpoints support process-crash diagnosis, not power-loss durability;
terminal run records use the durable metadata transaction.

`RunRecord.error` is the primary failure. `diagnostics` independently retains
progress, exit code/signal and cgroup OOM evidence, optional memory observations,
the last 8192 stderr bytes and up to four secondary errors. Truncation is explicit;
error prose is limited to 1024 UTF-8 bytes plus a marker. Missing measurements are
null, not zero. Memory identifies `cgroup-peak` or `sampled-tree-rss`; sampled peaks
can miss between-sample spikes. An unexplained SIGKILL is `worker-exited`, never
invented OOM evidence. An absent/malformed worker report is `worker-protocol`;
checkpoint publication errors are `diagnostic-channel`. Cleanup or journal-write
failure does not overwrite an earlier error or revoke a completed publication.
If metadata writes themselves fail, the in-memory result can contain diagnostics
that could not be persisted; recovery treats a remaining nonterminal row as
abandoned rather than inventing its missing terminal outcome.

Both human output and JSON expose these observations. The guard drains stderr
without allowing unlimited output to consume unbounded memory or starve timeout
checks. A worker report still has a 64 KiB serialized limit. Normal capacity/work
failure does not become malformed-file coverage or a truncated successful import.

### Current memory boundary

`WorkingMemory` is an explicit capacity authority for one operation. Owned
`ScratchBytes` use fallible allocation and retain their reservation until drop;
Rust borrows prevent scratch references from outliving their owner. Error and
unwind paths release the same capacity. No resettable integer scratch handles or
custom global allocator are exposed. This implementation uses admitted owned
buffers rather than one mmap arena.

`memory_bytes` remains the process limit. `working_memory_bytes` is independent
algorithm capacity, exposed as `--working-memory-mib`; missing historical values
remain unknown. Admission counts requested buffer capacity and conservative
bounds for record decoding, temporary names and fixed control/I/O storage.
Import reserves 256 KiB for control and stream buffers. Query preparation and
delivery each reserve 64 KiB; manifest validation also reserves 32 KiB. Planning,
reopening and planned execution also reserve 256 KiB for retained recipe metadata. Typed
result decoding admits 64 times the encoded record length plus 4 KiB and the
encoded buffer. These reservations are included in the
limit. `RunProgress.working_memory` reports live and peak reserved capacity at
normal termination, including cooperative failure. Handoffs retain the maximum
observed peak across worker and delivery/retention phases. This is not RSS or a claim
that every allocation goes through an arena. Runtime, allocator bookkeeping,
stacks, SQLite internals and host diagnostics remain under the process limit.

`FileLease` keeps a verified captured file open. `MemberCursor` reads archive
headers and names through positional `ByteSource` reads; it does not retain the
archive or all member descriptors. GNU/BSD/COFF names and the finite AIX member
index are read on demand. Thin payload leases end after each occurrence. Unknown
payloads only require a bounded prefix and streaming hash. ELF inspection admits
one contiguous object buffer for borrowed object-crate views, plus a conservative
name workspace of twice that buffer's length and 16 KiB. An ELF that does not fit
fails explicitly; no algorithm silently switches to unbounded memory.

Sections, symbols, relocations and diagnostics are emitted to synchronous sinks.
Store assembles schema-1 manifests from temporary disk streams and validates
ownership before issuing a receipt. No complete inventory or symbol-ID set is
required in memory. A sink failure aborts the operation, including an integrity
error from the sink; it cannot become malformed-ELF coverage.

Snapshot validation navigates JSON file ranges with a fixed nesting stack and
decodes one typed record at a time. It admits a conservative bound of 64 times
the encoded leaf length plus 4 KiB, as well as the encoded input buffer. Field
order is independent of the original serializer. Ordinary symbol order validates
in one pass; unordered older records use budgeted lookback to detect duplicates
without a growing index. Repeated reads count as work. No read builds an index,
rewrites a manifest, repairs SQLite or reopens an origin path.

`inventory_stream` and `doctor_stream` require explicit memory/control ports;
embedded callers own their clock, cancellation and consumer capacity. The older
materializing `inventory`/`snapshot` and `doctor` convenience APIs have a default
256 MiB admission cap and no host deadline. They are not used by CLI queries.
Consumers retaining copies from streaming callbacks owe their own capacity budget.

The CLI uses `Application::start_query`, an internal guard and private output
outside the project. Successful worker cleanup precedes `take_output`;
`QueryOutput` Drop removes its files. Failed computation emits no partial stdout.
Delivery continues the original work budget/deadline and accepts cancellation.
Only one delivery attempt is allowed: output I/O failure can leave a prefix and
must not cause an implicit retry. Read-query diagnostics are returned to the
caller, not persisted in the project. Temporary results use the bounded storage
contract below. No separately invoked `blobray-run` is required.

Capacity exhaustion returns `resource-limited` with an optional structured
`error.memory`: requested reservation, available/limit bytes and phase/input/member.
The request is an admission amount, not an estimate of memory needed to finish.
Bounded host diagnostics are independent of the exhausted working capacity;
physical host allocation failure and opaque library calls retain process containment.

## Temporary storage and crash cleanup

`TemporaryStoragePolicy` belongs to the local `Application`, independently of
`ResourceBudget` and the serialized inspection recipe. The CLI accepts
`--temporary-root`, `--temporary-mib` (default 8192) and
`--temporary-total-mib` (default 32768) on supervised operations. The operation
limit must be at least 1 MiB and the aggregate at least the operation limit.
Changing these options does not change saved Plan bytes or `PlanId`. A saved
Plan can fail under a smaller local capacity without being rewritten.

Admission reserves the full operation allowance from the Application pool.
Completed query output shrinks that reservation to the logical length of the
files it retains. `QueryOutput`, a live `Plan` and its clones retain capacity
until the last owner releases the files. Each execution needs another full
reservation; a single-operation aggregate cannot admit execution while a Plan
still occupies part of that aggregate. Pools are per Application, not shared
between processes or independent Application instances.

Store-owned `TemporaryFile` admits growth before writing and offers no raw
writable file handle. Its shared `TemporaryBudget` counts simultaneously live
capture files, manifest fragments, closure data, query spools and Plan manifest
copies. Overwrites do not charge the same bytes again; sparse extension counts
the hole. Short writes refund unwritten growth; successful truncation and
fragment deletion refund capacity. Deduplicated fragments release their own
charge after deletion; staged retained payloads remain charged until operation
cleanup. A filesystem failure is independent of logical admission.

One MiB **inside** the operation limit is reserved for fixed control files.
Requests, owner/configuration records, progress/checkpoint replacement files and
worker/guard reports are each limited to 64 KiB; at most sixteen such files may
coexist. They can report a cooperative data-capacity failure even when the data
allowance is exhausted. This reserve cannot guarantee diagnostic writes when the
filesystem itself is full. Permanent CAS/SQLite data, saved Plan descriptions
and caller-selected export destinations are outside temporary accounting.
This is a logical-byte contract for trusted writers, not physical filesystem
reservation: block overhead, journals, external writers and open files held by
unrelated processes are not included.

Capacity failure returns `resource-limited` with `error.storage`: rejected
extension, available/limit bytes, run owner and last physical position. ENOSPC
and EDQUOT return `disk-full`, independently of remaining logical capacity.
`RunProgress.temporary_storage` records the worker's current/peak admitted bytes,
including the fixed control reserve; it is a sampled observation, not a live
counter after result deletion. Historical records without this field remain
unknown. `Application::temporary_storage_status` reports live aggregate
reservations, retained residue paths and bounded cleanup diagnostics. A failed
cleanup preserves its reservation; admission releases it only after the path is
confirmed absent. Residue admission is bounded by `max_operations`; already
admitted operations may contribute another `max_operations` residues. Cleanup
errors do not replace a primary failure or revoke committed publication.

Linux validates an owned private root (0700, no root symlink), by default
`$TMPDIR/blobray-next-<uid>`. Query/Plan workspaces live there, outside projects.
Each has versioned owner metadata (PID, start ticks, boot identity), a run ID
and a shared lease held through computation, delivery and retained Plan lifetime.
Opening that runtime for admission automatically reclaims only known-version
workspaces with a dead owner and an exclusively acquired lease. The host first
confirms/removes empty containment, then application removes owned files.
Runtime creation, reconciliation and owned cleanup share a root lock. Files from an active
owner or lease remain untouched. Unknown/corrupt metadata, symlinks, special
files and unexpected nesting remain in place with diagnostics; initialization
interrupted before a valid owner record also requires inspection. Cleanup never
follows a link to a saved revision or export.

Reconciliation examines at most 4096 root entries; exceeding this bound rejects
admission. A workspace inspection permits at most 4096 entries and one stage
directory level; unrecognized workspaces are retained. Diagnostics retain four
messages and expose a truncation flag. The CLI emits these warnings to stderr,
including failures during final result cleanup. Import staging remains inside
the project and requires explicit `recover`; runtime reconciliation does not
acquire project writer authority or delete retained evidence. These ownership rules also cover the supervised analysis and execution operations.

## Diagnosis and recovery

```console
cargo blobray doctor --project /path/to/investigation \
  --limit-mode watchdog --format json
cargo blobray recover --project /path/to/investigation
```

Projects require the [current native formats](../interfaces-formats/README.md#current-formats). Earlier and future
formats are rejected without conversion or mutation. There is no `upgrade`
command or compatibility reader. Keep older projects intact; new investigations
use a new project directory. Revision manifests have an independent version.

`doctor` verifies saved revisions, prepared-image and function-analysis closures and reports unfinished runs without acquiring a
writer, repairing SQLite or changing current. A hot rollback journal can prevent
a read-only open; `recover` explicitly acquires writable recovery authority.
Recover checks owner identity and staging leases, removes only owned inactive
temporary directories and empty run cgroups, and records abandoned attempts.
It never constructs success from loose files or reimports original source paths.
Repeated recovery is safe; active owners/leases prevent cleanup.

The private `.blobray-next` directory contains SQLite metadata, immutable
`objects/`, per-run `staging/` and a writer lock. Unix directory permissions and
Git ignores keep captured binaries outside version control. Files are copied
from inputs, never hard-linked to mutable originals. Promotion may hard-link an
already synced private staging payload into the content store. Existing content
is verified before reuse, and corruption is never overwritten automatically.
SQLite uses `synchronous=EXTRA`; payloads and Unix directory metadata are synced
before publication. Local filesystem durability does not imply equivalent
network-filesystem guarantees.

All committed revisions are retained. There is no CAS garbage collector, pruning,
compaction or legacy-data migration. Failed imports or image preparations may leave unreferenced CAS
objects. Recovery cleans owned temporary work, not retained objects. Source
metadata detects ordinary capture mutation; without expected digests or an
external filesystem snapshot, several mutable inputs are not an atomic source
snapshot. Instruction decoding and local function graphs are implemented;
bounded concrete machine-code execution and comparison use the profile below. TUI and general equivalence proofs remain outside the implemented scope.
