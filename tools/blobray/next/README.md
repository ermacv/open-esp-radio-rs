# Blobray Next: captured inputs and bounded operations

`blobray-next` imports immutable inputs, analyzes selected RV32 functions or
whole libraries, prepares synthetic images, executes explicit RV32 scenarios,
compares compiled observations, retains reviewed knowledge and
preserves projects through backup/restore and phased legacy capture. The API and
CLI share supervised work, cancellation, publication and recovery. It is
independent of the legacy engine; `cargo blobray` selects this host.
The [architecture](../docs/design/architecture.md) owns module authority;
[contracts](../docs/design/contracts.md) owns identities, assessment, lifetime and
resource rules; [workflows](../docs/design/workflows.md) distinguishes implemented
scenarios from target capabilities. This README is the command reference.

The current data model separates captured source revisions, selected analysis
recipes, reviewed knowledge revisions and immutable result publications. A run
records termination independently of scoped result coverage, policy checks and
comparison verdicts. `Completed` can legitimately describe partial research or a
`DIFF`/`INCOMPLETE` comparison. See [result assessment](../docs/design/contracts.md#result-assessment).

Implemented profiles include archive/thin-archive inventory, RV32 ELF inspection,
local and whole-library static analysis, linked-image PHY/ROM research, explicit
MMIO review, bounded integer execution/comparison/replay, target auditing and
project preservation. Unsupported ISA semantics remain explicit gaps. There is
no general equivalence proof, TUI, CAS pruning or allocation-free core.

## Use

Run from the repository root with the pinned Rust toolchain:

```console
cargo blobray init --project /path/to/investigation
cargo blobray import --project /path/to/investigation \
  --cgroup-root /sys/fs/cgroup/path/to/delegated-parent \
  --input vendor=/path/to/libvendor.a --input companion=/path/to/companion.o
cargo blobray inventory --project /path/to/investigation \
  --cgroup-root /sys/fs/cgroup/path/to/delegated-parent --format json
cargo blobray runs --project /path/to/investigation
cargo blobray revisions --project /path/to/investigation
```

`import`, `inventory`, `doctor`, `select`, `plan`, `run`, `link-plan`,
`prepare-image`, `images`, `image`, `export-image`, `analyze-function`,
`analyses`, `analysis` and `export-analysis` require Linux containment in the CLI. The default `--limit-mode kernel` requires a writable
cgroup v2 parent delegated to this user, with the memory controller available and
no processes in the parent when enabling that controller. `--cgroup-root` selects
that parent; without it the host tries its current cgroup. The adapter enables
`+memory` in the delegated parent, creates a per-run child and moves the worker
there before exec. It sets `memory.max`, disables swap for the worker group,
and enables group OOM handling. An unavailable backend fails before worker
execution; it never selects watchdog automatically.

Where kernel containment is unavailable, explicitly select
`--limit-mode watchdog`. This samples the worker tree's RSS, including descendants
that enter another session. A short-lived memory spike can exceed the budget
between samples; the run record identifies this mode. Both modes own descendant
cleanup and use the same application cancellation/publication contract.

Defaults are a 4096 MiB process limit, 256 MiB working capacity, 900 seconds,
1,000,000,000 work units, a 100 ms sampling interval and a 10 second termination
grace. Operation commands accept `--memory-mib`, `--working-memory-mib`,
`--timeout-secs` and `--max-work-units` (positive).
`ResourceBudget` also exposes sampling and grace intervals to API clients.
Ctrl+C or SIGTERM requests cancellation. SIGKILL cannot produce a normal terminal
response; the guard detects the closed coordinator pipe and stops its worker tree.

Repeat `--input ROLE=PATH` in the intended order. Roles are nonempty UTF-8 labels;
duplicate roles, paths and member names are allowed. Native source paths and
archive/symbol names retain their bytes (UTF-16 units for Windows origin records).
Human rendering can be lossy; JSON byte arrays preserve original names exactly.
`--expect INDEX=SHA256` attaches an expected digest to a zero-based occurrence.
Digests contain 64 lowercase hexadecimal digits. Captured mismatches fail import;
unavailable inputs retain their unfulfilled expectations and diagnostics.

`inventory --revision SHA256` selects a committed revision, otherwise current.
Old inventory remains identical after another import, source deletion or moving
the entire project. Reading never imports changed files or reruns the parser.

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

See [resource and ownership contracts](../docs/design/contracts.md#research-memory-and-read-only-observations)
for research phase lifetimes, admitted memory diagnostics and storage semantics.

## Owners and interfaces

| Crate | Responsibility | Internal dependencies |
| --- | --- | --- |
| [domain](../crates/domain/README.md) | Identities, revision records, budgets and portable control/stream ports | None |
| [artifacts](../crates/artifacts/README.md) | ELF/AR inventory over borrowed captured bytes | Domain |
| [store](../crates/store/README.md) | Capture, integrity, persisted run records, publication and owned staging | Domain |
| [knowledge](../crates/knowledge/README.md) | Claim validation and review transitions | Domain |
| [verification](../crates/verification/README.md) | Concrete observation comparison and verdicts | Domain |
| [analysis](../crates/analysis/README.md) | Local CFG, values and memory effects | Domain |
| [riscv](../crates/riscv/README.md) | RV32 decoding, lifting and relocation interpretation | Domain |
| [application](../crates/application/README.md) | Common supervisor, execution protocol, ordering, thin-member resolution and recovery | Domain, artifacts, store, analysis, knowledge, verification |
| [next](src/main.rs) | Rendering, signals, Linux process/cgroup adapter and host composition | Domain, application, backend-riscv |

`Application::new` receives an `OperationHost`; libraries do not discover executables,
install signal handlers or change process-global supervision state.
`Application::start_import` and `Application::start_query` return `RunHandle`
with `status`, `events`, `cancel` and repeatable `wait`. `Application::import`
and `Application::query` are blocking adapters over those same jobs. Import
returns a compact `RunRecord`; retrieve full inventory separately. Query output
transfers once through `take_output`, after worker cleanup. Dropping handles
does not detach operations.

`shutdown` closes admission, cancels active jobs and drains workers without
holding the registration mutex; concurrent shutdown callers wait for teardown.
Application Drop uses the same path. Each project admits one writer and concurrent
readers, with `busy` for a competing import. `ApplicationLimits` defaults to 16
operation/result slots and 64 events per job; `with_limits` accepts 1..1024 slots
and 1..4096 events. Full admission returns `busy`, without queuing. Finished
application-owned jobs are reaped on admission or shutdown. Retained client
handles, transferred output and live plans continue to occupy their slot until
dropped. Plan clones share a slot; each concurrent run additionally needs its own.
Progress cursors are monotonic; the terminal record remains available through
wait, and import records also through persistent `runs` queries.

Inventory query admission resolves current once to an explicit revision ID.
`ReadView` exposes selection, inventory and doctor without writer or recovery
methods. `InventoryView` owns a verified manifest lease, and its byte-source
borrow cannot outlive that lease. Neither exposes the underlying store project.

The worker receives a payload-only staging capability. It captures, parses,
validates the manifest/dependency closure and serializes large results inside
resource containment. It cannot publish through that capability. The coordinator
accepts a bounded receipt, verifies and promotes referenced files using streaming
reads, then publishes. Large inventory never becomes a supervisor message or an
import response. The host/worker protocol is trusted composition, not a sandbox
for arbitrary hostile plugins running under the same OS user.

### Relationship to target interfaces

The [target capability boundaries](../docs/design/contracts.md#handles-and-capability-boundaries)
are broader than these callable interfaces. Import, inventory, doctor, selection
and inspection planning/execution share
application admission, worker lifecycle, cancellation and teardown. Queries keep
their status and diagnostics in memory; they never acquire a project writer or
create a run journal. Private job messages and host ports belong to application;
store owns persisted run/owner/receipt schemas, and the Linux adapter interprets
process identities. Human/JSON formats belong to the frontend.

The worker writes typed length-delimited records and a bounded summary into a
private result directory; inventory also captures the validated manifest bytes.
This result protocol is internal to the matching application/host build. It does
not add another archive parser or retained research format. The frontend consumes
borrowed records or the captured manifest without loading the complete result.

`InventoryView` is not a transitive retention pin for future pruning/compaction,
which Next does not implement. Inspection `Plan`, synthetic `LinkPlan` and retained
prepared images, bounded review and concrete comparison are implemented. General
pass planning and extended model/review policies remain outside this profile. Metadata listing,
initialization and recovery do not use the supervised worker path.

## Synthetic prepared images

`link-plan` validates a selected revision and creates a schema-1 `LinkPlan`.
`prepare-image` materializes its captured inputs, invokes explicit LLVM LLD 22,
validates the output and atomically publishes an immutable prepared image and
completed run. It never changes the current import revision. A ready plan means
supported inputs and unambiguous roots; it does not promise that all references
will resolve or that the requested layout is large enough. Those checks also run
during preparation. Plans with blockers can be saved and inspected but not run.

The request is JSON with `revision` (ID or null to freeze current at admission),
ordered `inputs` (zero-based imported binding ordinals), `entry`, optional `roots`
and `layout`. An entry/root is `{ "input": 0, "symbol": <SymbolId> }`; obtain the
complete physical `SymbolId` from `select` or inventory. Names are not selectors.
The layout has `code` and `data`, each with numeric `start` and `length`, for example
`{"code":{"start":268435456,"length":65536},"data":{"start":536870912,"length":65536}}`.
Regions must be nonempty, start on 4096-byte boundaries, fit RV32 and not overlap.

```console
cargo blobray link-plan --project /path/to/investigation \
  --request /path/to/link-request.json --linker /usr/bin/ld.lld \
  --output /path/to/link-plan.json --limit-mode watchdog
cargo blobray prepare-image --project /path/to/investigation \
  --plan /path/to/link-plan.json --linker /usr/bin/ld.lld --limit-mode watchdog
cargo blobray images --project /path/to/investigation \
  --limit-mode watchdog --format json
cargo blobray image --project /path/to/investigation \
  --id <prepared-image-id> --limit-mode watchdog --format json
cargo blobray export-image --project /path/to/investigation \
  --id <prepared-image-id> --output /path/to/new-directory --limit-mode watchdog
```

The application equivalents are `start_link_plan`/`link_plan`,
`RunHandle::take_link_plan`, `LinkPlan::write`, `read_link_plan`,
`start_prepare_image`, and `ReadQuery::{Images,Image}`. Plan clones retain one
application slot, immutable captured manifest and temporary workspace. Preparing
uses another slot and revalidates the frozen revision and all selected captures
in the explicitly selected project. Dropping a plan does not cancel an admitted
image job. A saved description needs that project's retained source closure;
it cannot reopen original source paths. The store currently retains all revisions
and images without pruning. Reopening an image verifies retained digests and
needs neither the linker nor original input files. Project relocation preserves
identities. An exported bundle contains `image.elf`, `manifest.json`, `link.map`,
`extraction.tsv` and `provenance.jsonl`. Export requires a nonexistent destination,
writes the manifest last and never overwrites an existing destination. A failed
export can leave an incomplete directory; its presence alone does not mean success.

### Link policy and evidence

Link policy 2 accepts little-endian RV32 ILP32/ILP32F/ILP32D relocatable objects, ordinary
archives and captured thin archives. Companion code and data participate in the
same link. Every selected payload must be supported; malformed, missing, TLS,
RV32E and quad-float inputs block preparation. Linked firmware/ROM ELF bindings as linker inputs,
dynamic linking, arbitrary scripts and caller-supplied linker flags are not
supported. Root symbols must be defined global/weak executable symbols with
unambiguous static-table and section identity. Input names containing CR/LF are
rejected because LLD's unescaped map cannot safely represent them.

Each member is materialized once as `i<input>-m<ordinal>.o`; duplicate archive
names and byte-identical imported bindings remain distinct occurrences. Root
objects are forced first in entry/root order, deduplicated by occurrence and
removed from their original lazy groups. Remaining archive members use ordered
`--start-lib`/`--end-lib` groups; standalone objects are direct inputs. Archive
indices are not reused. This explicit synthetic selection policy can differ from
the producer's original link and never establishes original firmware selection.

The generated script defines CODE and DATA regions, RX/RW load segments,
text/rodata/eh-frame, data/sdata and bss/common sections and the RISC-V global
pointer. LLD uses `elf32lriscv`, one thread, section GC, emitted relocations,
no relaxation, no ICF, no build ID, no demangling and strict undefined-symbol
handling. The environment is cleared except `LC_ALL=C` and `TZ=UTC`; core dumps
are disabled. Other sections follow LLD 22 placement rules and must pass the same
post-link segment bounds. The tool path is explicit: no PATH/rustc search or
linker substitution. Its full version response and executable digest enter the
recipe and are checked again for preparation. This identifies the executable,
not a hermetic snapshot of its dynamically loaded system libraries.

`LinkPlanId` hashes the schema/policy, project, revision, input order, exact roots,
layout and linker identity. Policy 4 fixes accepted ABI families, explicit ROM definitions, executable-section placement, transformation, script generation,
flags and environment. Time, memory and disk budgets and local paths do not enter
this identity. Existing inspection Plan schemas and IDs keep their own contracts.
`PreparedImageId` hashes its manifest, which references the ELF, raw map, extraction
report and normalized provenance by content digest. The manifest labels the image
synthetic and retains verified roots/segments and bounded successful tool stderr.

Validation requires RV32 ET_EXEC, bounded nonoverlapping PT_LOAD segments inside
the declared regions, no writable executable segment, allocated sections covered
by load segments, file-backed executable roots and no unresolved symbol relocation
in allocated sections, including unresolved weak references. The selected entry
must equal `e_entry`. Root addresses are proved by input-section map rows plus
source symbol offsets, unchanged section sizes and matching output symbols.
Hidden symbols may be localized by LLD. Symbol rows cannot impersonate section
rows. No exact root mapping means no publication. Other map records retain their
reported source occurrence with `exact: false`; they are observations, not a
complete instruction-by-instruction provenance proof. Execution and comparison
are not implemented by image preparation.

### Ownership, limits and persistence

Domain owns recipes/IDs and result values; artifacts owns input and output ELF
validation; application owns selection, materialization, policy, map evidence
and orchestration. The Linux `Lld22` adapter implements the injected `LinkerHost`
port and only launches/drains the specified tool. Store owns payload leases,
`PreparedImageReceipt`, `RetainedImage` and transactional publication. No worker
has publication authority and no linker adapter selects project inputs.

The same operation guard contains worker and LLD. Output ELF, map and extraction
flow through concurrently drained pipes into quota-admitted `TemporaryFile`s;
LLD receives no writable file output path. Materialized inputs and script are
also charged. LLD buffers stdout ELF in its own memory, covered by the process
limit, not by Blobray `WorkingMemory`. Inspection and validation admit one full
object/image buffer at a time; this is not a streaming ELF linker or a no-heap
core. A bounded 8 MiB metadata reservation covers at most 512 selected bindings,
4096 members, 16 roots and 32 blockers. Root names/sections are at most 4096 bytes,
map lines at most 64 KiB, saved plan metadata at most 60 KiB, image manifest at
most 56 KiB and successful stderr tail at most 8192 bytes. Capacity exhaustion
fails explicitly, without partial publication.

Durable run records identify the admitted operation and its published result;
query status stays in memory. Payload promotion precedes one transaction for
image plus completed run. Failures may leave unreferenced CAS objects, never a
listed partial image. Cancellation is linearized before commit; a committed
result remains successful even if response delivery is lost. Recovery abandons
interrupted attempts and never converts staged ELF into success. Doctor checks
retained image closures. See [JSON and checks](#json-and-checks) for versions.

Real-link integration tests require LLD 22 at `/usr/bin/ld.lld`, or an explicit
`BLOBRAY_TEST_LLD` executable. This dependency is mandatory, including for
`cargo xtask check blobray-standalone`; absence is a test failure. These tests
exercise synthetic RV32 bytes, not private vendor binaries or execution readiness.

## Selection and inspection plans

`select` searches exact name bytes and streams all matching candidates. It never
chooses the first match. UTF-8 `--name` and byte-exact `--name-hex` are mutually
exclusive; `--input` restricts the zero-based input binding. A candidate includes
its revision, complete `scope` selector and captured payload digest when known.
Names, roles and source paths are not identities. Empty names are permitted.

```console
cargo blobray select --project /path/to/investigation \
  --kind symbol --name same --limit-mode watchdog --format json
cargo blobray select --project /path/to/investigation \
  --kind symbol --name-hex 6c6f63616cff --input 0 --limit-mode watchdog
cargo blobray plan --project /path/to/investigation \
  --request /path/to/request.json --output /path/to/plan.json --limit-mode watchdog
cargo blobray run --project /path/to/investigation \
  --plan /path/to/plan.json --limit-mode watchdog --format json
```

The request is the same `PlanRequest` accepted by the application API. Example
for inspecting one input binding (set `revision` to a selected digest to avoid
using current at admission):

```json
{
  "revision": null,
  "scope": {"kind": "input", "input": 0},
  "budget": {
    "mode": "watchdog",
    "memory_bytes": 4294967296,
    "working_memory_bytes": 268435456,
    "timeout_ms": 900000,
    "grace_ms": 10000,
    "poll_ms": 100,
    "max_work_units": 1000000000,
    "work_policy": 1
  }
}
```

Other scopes are `{"kind":"revision"}`, object (`input` plus `object: ObjectId`)
and symbol (`input` plus `symbol: SymbolId`). Copy the candidate's `scope` and
`revision` into the request rather than resolving its name again. This preserves
repeated input bindings and static/dynamic symbol tables. Identical thin archive
bytes can select different payloads in different inputs or revisions.

Object inspection returns its input/object headers, sections, symbols,
relocations, thin binding and diagnostics. Symbol inspection returns one exact
symbol with its input/object context, defining section when identified, referring
relocations and applicable diagnostics. Undefined/common/absolute symbols do not
invent a defining section. Input headers and object ELF headers in callbacks have
empty nested arrays; following records supply those values. No linker definition
selection, disassembly or behavioral analysis occurs. `revision_complete` always
describes the full source inventory, even when a selected subset is inspectable.

`Application::start_plan`/`plan` create an immutable `Plan` under a separately
supplied planning budget. `RunHandle::take_plan` transfers it once; wait remains
repeatable. The plan owns a private captured manifest with no writer access.
Clones share that ownership and admission slot. `start_run(&Plan)` acquires its
own ownership until worker teardown and uses the captured manifest, even after
the original project path disappears. It does not reparse binary inputs or
inspect current. Last release removes the private manifest. This metadata lease
is not a transitive payload pin for future garbage collection.

`PlanDescription` contains an ID and recipe: schema/operation/result versions,
project and revision, exact scope, target, inventory producer, selected capture
binding, revision coverage and execution budget. Whole-revision dependencies are
bound by the manifest digest rather than an in-memory list. `PlanId` is SHA-256 of
the compact typed JSON recipe in serializer field order; input key order and
whitespace do not affect it. IDs include execution budgets, but exclude project
paths, time and attempt IDs. They establish content identity, not authenticity.
This is an inspection recipe identity, not a future semantic computation/cache key.

`PlanDescription::read` bounds the complete file to 64 KiB and checks versions
and ID. `start_reopen_plan`/`reopen_plan` additionally verify project membership,
retained dependencies and exact recipe agreement before granting a live Plan.
They never substitute current or replan a changed description. A missing target
returns `not-found`; corruption is an integrity error. Missing/unsupported
inventory remains inspectable with coverage diagnostics, not a fabricated
analysis capability.

`Plan::write` delivers the portable description once under the remaining planning
budget; writing a destination does not modify the project. CLI `--output` stages
and publishes that file without overwriting an existing file. Without `--output`,
`plan --format json` writes the description to stdout; human mode displays a
bounded summary. Saving is explicit and no plan database is created.

Every `start_run` gets a new `RunId` and deadline using the saved execution budget.
CLI `run` limit flags control reopening only; execution cannot silently override
the recipe. Reopening defaults to kernel containment even when the saved execution
budget requests watchdog, so select watchdog explicitly where delegation is absent.
Within each operation, work/deadline accounting continues through output delivery.
Planning, reopening, selection and inspection do not create durable run journals.

`select` and `run` JSON output use `{schema:2,records:[...],summary:{...},assessment:{...}}`. Records
carry `kind` and `value`; candidates carry complete selectors. Both the records
array and human records are streamed. Zero matches is a successful search with
coverage reported; precise planning of an absent occurrence fails. Consumer
failure is terminal and cannot be converted to a coverage gap or retried implicitly.

## Identity and schema 1

[Domain records](../crates/domain/src/lib.rs) define revision serialization:

- `ProjectId` is a persistent random 256-bit identity independent of directory.
- `ArtifactId` hashes exact captured bytes with SHA-256.
- `ObjectId` combines the container artifact with standalone or zero-based archive
  payload ordinal. Archive index/name-table metadata does not consume ordinals.
- `SymbolId` includes object, table kind, table section and entry index.
- `RevisionId` hashes the exact stored JSON manifest bytes, not arbitrary JSON
  reserialization. `RunId` identifies an execution attempt, never its result bytes.

Revision schema 1 retains project/parent, target, producer, ordered roles,
lossless absolute origins, expected digests, captures, thin-member bindings and
inventory. Repeated content may share stored bytes while member occurrences
remain separate. Thin container identity alone does not identify external bytes;
use the selected revision's ordinal-to-capture bindings.

Inventory retains sections, static/dynamic symbols including null/local/weak/
common/undefined entries, raw metadata and REL/RELA references. Malformed names,
invalid references, unreadable members and unknown remaining membership are
explicit diagnostics. Packed RELR decoding and nested archive expansion are
unsupported. Inventory never chooses linker definitions or proves equivalence.
The initial interpretation target is `riscv32-ilp32`; structural ELF32/64 reading
is independent of target execution support.

## Job outcomes and publication

The normal path is registered → running → validating → completed. Failure,
cancellation, timeout and resource exhaustion have distinct terminal states;
recovery marks interrupted nonterminal attempts abandoned.

Cancellation admitted before the commit boundary prevents publication. Once the
coordinator enters the final metadata transaction, cancel returns false and the
transaction determines the outcome. A successful commit atomically records the
revision, advances current and marks the run completed. Lost response delivery
or late cancellation cannot relabel that stored success. Cleanup errors remain
diagnostics and do not revoke a committed revision.

The Linux guard is a dedicated child-subreaper process. It observes a coordinator
lifetime pipe, applies TERM then KILL after the grace period, and reaps descendants
before reporting completion. It retains a shared staging lease during cleanup;
recovery requires an exclusive lease. Owner records include PID, process start
time and boot identity. A PID by itself does not establish that an owner survives.

Worker memory is kernel-enforced or sampled as recorded; coordinator validation
uses fixed-size streaming buffers and bounded control messages. The deadline also
covers coordinator verification before commit. Final commit and durable sync are
not interruptible halfway. Kernel uninterruptible I/O can delay teardown; these
host deadlines are not hard real-time guarantees. Read-only inventory/doctor use owned query workers under the same host guard.
They create no project journal row and never acquire a project writer.

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

Work policy 1 charges one unit per visited structural record and one per block of
up to 4096 bytes for each read, hash, copy, delimiter scan or serialization output
operation. Repeated processing is charged again. Constant-time borrowed-range
access performs a zero-cost checkpoint. Charges are admission costs before work,
not CPU instruction counts, and are not refunded after an I/O error. Accounting
also includes closure-record/framing visits. Repeated execution with the same
inputs, retained objects and policy has the same work cost; reusing an existing
object requires verification and can change the cost compared with an empty store.
JSON output is buffered into bounded writes. ELF string scans, explicit table
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
Runtime creation and reconciliation share a root lock. Files from an active
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

Projects require metadata schema 13 and journal schema 14. Earlier and future
formats are rejected without conversion or mutation. There is no `upgrade`
command or compatibility reader. Keep older projects intact; new investigations
use a new project directory. Revision manifests keep their own schema 1.

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

## JSON and checks

`import` returns envelope schema 3 with `run`, including its state, budget/mode,
selected base, resulting revision/completeness and diagnostic. Completed imports
exit 0 even for incomplete inventory; all other run outcomes exit nonzero and
write the run envelope to stderr. Inventory coverage is not a verification verdict.

Run records use schema 14 for every durable and read operation. Storage metadata
uses schema 13; revision manifests use schema 1 and execution manifests use schema
1. These are independent formats. Earlier journals are rejected by single-run,
list, recovery and restore readers. `assessment` replaces generic run-level
`complete`/`verdict`; its scoped coverage, optional policy check and optional
comparison are independent of `state`. Empty assessment means the operation has
no research coverage or verdict to assert. A non-completed run has no assessment.

The private import/read-query requests use schema 2; image, function,
investigation, knowledge, execution and concrete scenario requests use schema 1.
Worker reports use schema 6 with tagged receipts and fixed progress counters.
These private protocols require the matching worker binary. Request/control
messages have a 64 KiB encoded limit before allocation can expand the message.
Admission requires working capacity and work policy; diagnostics cannot be
invented while reading unsupported old records.

`runs`/`recover` retain envelope schema 2; `init` uses schema 1. Successful
inventory and doctor output use schema 2 and include `assessment`; inventory
also retains `complete` within its inventory-specific contract and `snapshot`.
Record streams use schema 2 with `records`, `summary` and `assessment`.
Command run envelopes (import 3, function/research 4, investigation 5, knowledge 6,
execution 7) wrap the same schema-14 run; an envelope version is not a journal
version. Partial research and valid comparison verdicts exit 0. Failed or
inconclusive policy checks, including doctor/link-plan blockers, exit nonzero.
Request/admission errors use `{schema:1,error:{code,message}}` on stderr; worker
query failures use `{schema:1,query:WorkerReport}`. Codes are machine interfaces;
descriptive prose is not a parsing key.

```console
cargo test -p blobray-domain -p blobray-artifacts -p blobray-store \
  -p blobray-application -p blobray-next
cargo clippy -p blobray-domain -p blobray-artifacts -p blobray-store \
  -p blobray-application -p blobray-next --all-targets -- -D warnings
cargo xtask check blobray-standalone
```

Tests use synthetic binaries and isolated fixture processes. The cgroup allocation
test requires real memory delegation and is explicitly ignored in ordinary runs.
Build its executable with `cargo test -p blobray-next --lib --no-run`, then run
that printed test executable under a delegated systemd user service:

```console
systemd-run --user --pipe --wait --collect \
  --property=Delegate=memory --property=DelegateSubgroup=coordinator \
  /absolute/path/to/test-executable --ignored \
  --exact linux::tests::kernel_cgroup_enforces_real_memory_allocation
```

This needs a systemd version supporting `DelegateSubgroup`. The standalone check
extracts only the shipping core and runs its tests. It excludes legacy execution
and the independent register source-publication tool.

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

From the repository root, build `cargo build --profile blobray -p blobray-next`.
Use `target/blobray/blobray` as `blobray` below. Linking requires the
explicit LLD 22 executable; reading and analysis do not require a linker.

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
LLD checks ABI compatibility across selected inputs. Floating-point instruction
semantics remain unsupported in static research; concrete execution uses the separate
[execution contract](#concrete-execution-and-comparison). Unsupported instructions stay gaps.
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
[JSON and checks](#json-and-checks).

### Values and memory effects

Function recipe/manifest version 6 records the semantic producer, typed source,
address space, register values, memory accesses, transfers and semantic gaps.
Older function schemas are unsupported; reading never converts or recomputes
a result. Captured inputs are unchanged. Function manifests are independent
of storage metadata version. Function selection policy 7 validates physical static/dynamic tables.
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
blobray knowledge --project research --limit-mode watchdog propose-register --analysis ANALYSIS_ID --subject phy.force-dig-gain --name FORCE_DIG_GAIN --address 0x20100408 --field ENABLE:16:1 --field GAIN_0:0:8 --field GAIN_1:8:8 --actor researcher --reason "Reviewed register evidence"
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
[execution contract](#concrete-execution-and-comparison).

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

## Knowledge and preservation

Source `RevisionId`, `KnowledgeRevisionId` and analysis/publication IDs have
different meanings. A knowledge revision is an immutable review event with its
expected parent, project, assertion, actor, reason and retained evidence roots.
The head is the last committed event. Proposal, review and head advancement share
one transaction with the durable run outcome. A stale base returns `conflict`;
failure before commit leaves the previous head readable. Superseded and rejected
assertions, evidence and historical revisions remain retained; there is no GC.

The pure [knowledge crate](../crates/knowledge/README.md) owns claim and transition
rules. Application owns occurrence/evidence validation and review orchestration.
Store owns content identities, event history and transactional publication. CLI
only supplies the same typed requests used by API clients. Accepted hypotheses
remain hypotheses; review does not authenticate a legacy proof.

```console
blobray knowledge --project PROJECT --limit-mode watchdog validate --change change.json
blobray knowledge --project PROJECT --limit-mode watchdog apply --change change.json
blobray knowledge --project PROJECT --limit-mode watchdog show
blobray knowledge --project PROJECT --limit-mode watchdog history --revision KNOWLEDGE_SHA
blobray knowledge --project PROJECT --limit-mode watchdog export --output knowledge.json
blobray backup --project PROJECT --output project.blobray --limit-mode watchdog
blobray restore --backup project.blobray --project NEW_PROJECT --limit-mode watchdog
blobray import-legacy --request legacy.json --project NEW_PROJECT --limit-mode watchdog
blobray legacy --project NEW_PROJECT --format json --limit-mode watchdog
blobray export-payload --project NEW_PROJECT --id PAYLOAD_SHA --output retained.bin --limit-mode watchdog
```

`Application::start_knowledge` takes a `KnowledgeChange`. `expected_base` is
required, including an explicit `null` for the first proposal. `actor` and
`reason` must be nonempty. The tagged `action` is either
`{"kind":"propose","proposal":...}` or
`{"kind":"review","assertion":"ASSERTION_SHA","decision":"accept","supersedes":null}`.
`decision` also accepts `reject`. An explicit `supersedes` assertion replaces a
conflicting accepted assertion. `validate` runs the same checks in a read query;
it publishes nothing and does not reserve the base against a subsequent writer.
`show` streams assertion states; `history` streams original review events.
Both freeze the current knowledge head at admission unless a revision is given.
`export` writes the history and evidence references as JSON without binary bytes.
It is not a complete private backup. All export destinations must be new.

A proposal contains `subject`, `occurrence`, `claim`, `evidence` and optional
`note`. `occurrence` contains `revision`, `source` (`input` or `image`), `object` and optional `symbol`. Knowledge event manifests use version 2; earlier derived events are not converted.
The claim tags are `name` (a `name` string), `binding`, `function-extent` (an
`extent` range), `hypothesis` (a `text` string), `mmio-region` and `mmio-register`. Evidence tags are:

| Tag | Fields and validation |
| --- | --- |
| `source` | `payload`, `range`: retained object digest and nonempty object-file byte range; ordinary archive members use their exact ordinal |
| `analysis` | `analysis`, optional `record`: retained analysis of the same occurrence; record is a zero-based JSONL ordinal |
| `publication` | `publication`: retained publication containing the occurrence in the same source revision |
| `document` | `payload`: retained provenance/review document bytes; the document does not assert semantic correctness |

Function extents additionally pass the artifact parser's executable section and
range checks. A plan can select accepted boundaries explicitly using
`InvestigationRequest.reviewed_extents`, a list of `{revision, assertion}` pairs.
Each reference must denote an accepted extent at that exact knowledge revision
and the plan's source revision. Explicit and reviewed extents cannot repeat a
physical selector. Plans retain the review references; function computation still
uses the exact source and range. Name changes do not change a function analysis
identity or silently change a saved plan. `status` includes the knowledge head;
it reads publication metadata rather than rehashing every child analysis.
`doctor` performs closure validation.

Backup takes a SQLite read snapshot and copies immutable CAS payloads. Concurrent
publication cannot change that database snapshot; the bundle may include extra
unreferenced objects added during copying. The version-1 private bundle includes
an entry length and SHA-256 for the database and every payload. Restore verifies
entry identities, rejects duplicate entries, trailing bytes and path substitutions,
and runs doctor before delivery. Restored unfinished runs become `abandoned`;
the original database bytes remain in CAS. Imported revisions, completed results
and review events retain their identities. Restore rejects unsupported metadata and journal formats without converting them.

Backup and new-project preparation run in the common supervised query lifecycle.
The caller owns one result slot through delivery. Delivery copies to a private
sibling destination under the remaining time/work/disk budget, syncs it, and
exposes `.blobray-next` only after completion. The destination directory must not
exist. Cancellation or a copy failure cannot replace an existing project. A
crash at exposure can leave an empty destination or a complete state directory;
there is no partial project publication. A post-publication directory-sync error
is reported and may leave a complete destination requiring verification.

`LegacyRequest` has `manifest` (the original project TOML), optional `run_spec`,
`roots`, `inputs` and `target` (`riscv32-ilp32`). Paths use the lossless
`OriginPath` encoding. Additional `inputs` use `ImportBinding`: role, origin and
optional expected digest. The schema-1 run spec contributes its ordered exact
role/path bindings. No binary paths are inferred from symbol names. All inputs
must be stable while capturing; the importer never modifies its source project.

The adapter captures the entire manifest directory, including hidden caches,
revision snapshots, reviewed packs, comparison outputs and compiled binding
files present there. It follows the supported TOML file-reference fields and
captures additional caller-selected private roots. Symlink records preserve the
link target and separately capture the target; cycles are deduplicated. Every
file and top-level TOML table/value or array-table item has a catalog outcome:
`converted`, `preserved-unresolved`, `unsupported` or `missing-payload`. The
original file remains available by content digest, including all nested fields,
provenance, schema inputs and recovered hardware/calibration data.

The active conversion is schema-1 code boundaries with exact source digest,
source role, member, section and one matching function entry. It validates the
extent and records the old decision with actor `legacy-import`. Legacy names
remain in the proposal note; they are not automatically accepted as new name
claims. Ambiguous occurrences and conflicting decisions remain unresolved.
Unsupported ABI/interface/register representations and opaque files (including
compressed snapshots) remain verbatim and are not activated. References embedded
inside opaque formats are not interpreted: supply their external payload roots
explicitly. Catalog status reports this limitation; a completed capture does not
mean full semantic conversion or legacy feature parity.

Discovery is iterative and bounded: 16,384 paths, 4 MiB aggregate path bytes,
4,096 bytes per path, TOML nesting 64 and an 8 MiB per-document parser ceiling.
Private project builds admit at most 4 MiB of metadata before another bounded
event, with 16 MiB of reserved SQLite growth/journal headroom.
Exceeding a bound fails the operation without exposing the new project. Working
capacity also admits the path catalog and each parsed document. Temporary
capacity covers retained bytes and destination copying; insufficient capacity
fails rather than discarding evidence. Runtime result inspection permits six
directory levels and up to one million entries. Kernel/watchdog process limits
remain necessary for allocation in third-party libraries; these APIs do not
claim a single mmap arena or a process-wide no-allocation guarantee.

## Concrete execution and comparison

`execute`, `compare`, `replay` and `execution` use the existing supervised
operation/query paths. No legacy engine or external limiter participates.
Execution manifests use schema 1; journal/storage versions follow [JSON and checks](#json-and-checks). The completed journal record is the publication
reference. No second result index or current-source change is needed.

```console
blobray execute --project research --request execution.json --limit-mode watchdog
blobray compare --project research --request comparison.json --limit-mode watchdog
blobray execution --project research --id EXECUTION_SHA --limit-mode watchdog --format json
blobray replay --project research --id EXECUTION_SHA --limit-mode watchdog
```

`execute` requires one implementation; `compare` requires both and an explicit
binding class. A request has the following shape (replace the revision and entry
with an exact captured occurrence):

```json
{
  "schema": 1,
  "vendor": {
    "revision": "REVISION_SHA",
    "source": { "kind": "input", "input": 0 },
    "entry": 268435456,
    "companions": [],
    "abi": "riscv-integer",
    "stack": { "address": 805306368, "length": 65536, "fill": null, "bytes": [] }
  },
  "replacement": null,
  "binding": null,
  "cases": [{
    "name": "one-explicit-case",
    "vendor": { "arguments": [0, 0, 0, 0, 0, 0, 0, 0], "memory": [], "mmio": [] },
    "replacement": null
  }],
  "case_execution": "independent",
  "max_events": 4096,
  "compare_return": true
}
```

A target selects a captured standalone static RV32 ELF (`input`) or a retained
prepared image (`{"kind":"image","image":"IMAGE_SHA"}`). ET_REL/archive
entries must first use the shared image-preparation operation. `companions`
explicitly selects additional captured standalone ELF input ordinals in that
target's revision, including their code and data segments. Overlap is rejected;
linker absolute definitions alone do not supply executable bytes. Reading and
replay never rediscover origins or substitute another symbol implementation.

For comparison, supply a second target in `replacement`, set `binding` to
`production-entry` or `shared-core`, and supply each case's replacement invocation.
Binding is the caller's declared relationship to production; a label does not
authenticate that relationship or grant qualification. Captured bytes, entries,
scenarios and implementation identities remain in the evidence. Side-specific
arguments and layouts are explicit; the verifier does not infer their equivalence.

An invocation supplies all eight integer argument registers. x0 is zero, sp
starts at the aligned end of the declared stack, and ra is a reserved unmapped
return sentinel. Other integer registers begin unknown. Loading an unknown byte,
using an unknown register, an inaccessible/misaligned memory access or an
unsupported instruction ends that phase with a typed `incomplete` observation
and its PC. The current integer executor supports RV32IMC arithmetic, branches,
loads/stores, direct/indirect jumps and ordinary fence events. Atomics, FP,
CSR/privileged execution, syscalls, dynamic loading and TLS are unsupported.
Declared float ABI flags do not silently select floating-point execution.

The `static-elf/explicit-ram/register-bank-1` environment maps validated ELF
segments with their permissions and ELF-defined zero-fill. Scenario memory is
writable non-executable RAM: each seed has `address`, `length`, optional `fill`
and a byte prefix. Absent fill leaves new bytes unknown. Seeds cannot replace
ELF segments or overlap the stack, one another or MMIO. Stack fill is an explicit
condition; zero is never inferred from absent initialization.

MMIO is an explicitly selected register-bank model. Each cell has `address`,
`width` (1, 2 or 4 bytes) and `value`; a matching read returns the latest written
value and both operations emit events. This model does not imply real hardware
read side effects, FIFO behavior or timing. Cells must be aligned, disjoint and
outside mapped RAM/code. Missing cells remain inaccessible; there are no implicit
responses. This release has no external-call substitutions or pluggable device
model registry.

Independent cases recreate both sessions. Stateful cases retain writable ELF
and declared RAM bytes separately for each implementation. Later explicit seed
bytes/fill replace only that declared RAM; omitted bytes retain their prior state.
Registers, stack, MMIO cells and event buffers reset each phase. An incomplete
phase blocks subsequent dependent phases on both sides. A completed difference
does not erase concrete state or prevent later phases from running.

The [verifier](../crates/verification/README.md) compares ordered MMIO and fence
events plus the low 32-bit return when `compare_return` is true. It does not
compare RAM contents, calls or the high return register. `MATCH` applies only to
the listed concrete scenarios, not all possible arguments or paths. `DIFF`
retains the first differing event/return; `INCOMPLETE` retains missing execution
obligations. Across cases, a known difference survives other incomplete coverage.
`manifest.complete` means all phases returned, independently of the verdict.
A completed operation, including `DIFF` or `INCOMPLETE`, exits 0; admission,
resource, integrity and execution infrastructure failures exit nonzero.

All instruction loops share the admitted work/deadline control. Execute progress
uses `table` for the case ordinal and `entry` for the last PC. Each session
reserves metadata plus event capacity before allocation, and owns admitted byte
and initialization buffers for every region. Input ELF buffers are admitted
separately and released after loading. Event vectors remain charged through
comparison and serialization, then are reused or dropped with their session.
No host call-stack recursion follows the analyzed program.

Requests remain capped at 64 KiB, with 1–128 cases, at most 64 companions per
target, 128 RAM seeds and 1024 MMIO cells per invocation, 2048 regions per session,
and 1–65536 events per implementation per case. `max_events` exhaustion is a
resource failure with no publication; events are never silently truncated.
Traces stream as bounded JSONL events/outcomes/comparisons into quota-owned
staging. The coordinator checks the admitted recipe and stream structure before
atomically committing the result reference and completed run. Cancellation,
limits or corruption cannot publish partial evidence. Process-level OOM and
opaque dependency containment retain the existing host guarantees.

`execution` only reads retained evidence. `replay` checks the executor,
environment and verifier identities, reuses the exact request, and charges
selection and execution against one application run, original deadline and work budget. Missing implementations
are reported, never replaced. Doctor validates execution references, record order
and CAS integrity. Backup/restore includes the journal, evidence and captured
inputs; reopening evidence does not require replay tools. There is no converter
for an incompatible execution schema.

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


## Captured data, tables and coefficients

`data` reads exact ranges and selected saved analyses from one captured object.
It does not run an analyzer or infer a table boundary. `export-data` selects a
reviewed integer table or constant at an explicit knowledge revision. Both use
the same application-owned query, supervision and delivery budget as other reads.

```console
blobray data --project research --request data-request.json --limit-mode watchdog
blobray data --project research --request data-request.json --output observations --limit-mode watchdog
blobray knowledge --project research --limit-mode watchdog propose-data --request table-proposal.json
blobray knowledge --project research --limit-mode watchdog propose-constant --request constant-proposal.json
blobray knowledge --project research --limit-mode watchdog show
blobray knowledge --project research --limit-mode watchdog accept --base PROPOSAL_REVISION --assertion ASSERTION_ID --actor researcher --reason "Checked exact source evidence"
blobray export-data --project research --revision ACCEPTED_REVISION --assertion ASSERTION_ID --output accepted-data --limit-mode watchdog
```

The `DataRequest` JSON has `occurrence`, `ranges`, `analyses` and optional
`pointer_table` (null/absent for ordinary byte observations). Copy the exact
revision, source and object identity from inventory or an analysis recipe;
`occurrence.symbol` is optional. Object identity retains the archive member
ordinal even when names and bytes repeat. `analyses` is an array of analysis IDs
from that same revision/source/object. All their records are retained, including
call inputs, unknown targets, MMIO, expression definitions and semantic gaps.
Record ordinals address that retained stream, not instruction offsets.

Each range uses one of these selectors:

| Selector | JSON fields in addition to `kind` | Meaning |
| --- | --- | --- |
| `section` | `section`, `offset`, `length` | Explicit section-relative bytes |
| `symbol` | `symbol`, `length` (integer or null) | Exact static or dynamic SymbolId; null uses its declared size |
| `image` | `address`, `length` | Virtual address in a file-backed load mapping of an executable ELF |

The prepared-object profile requires little-endian RV32 ET_REL/ET_EXEC with at most one each
of SHT_SYMTAB and SHT_DYNSYM; table-free section/range selection is supported; names
such as `.symtab` are not table identity. Dynamic symbol selection does not enable
dynamic loading, TLS or relocation application. Section relocations must reference
the static table through `sh_link`; a different table is an explicit unsupported
profile, never an index interpreted in the static table. There are at most 32 ranges and 32 analysis IDs per request; at
least one is required. A zero-sized symbol needs an explicit length. A sized symbol cannot
be expanded past its declared size. Overflow, out-of-range, ambiguous addresses,
compressed sections and NOBITS are explicit errors. Requests never guess length
from the next symbol. Several ranges share one prepared ELF owner and section
metadata; borrowed views cannot escape its callback.

A `DataProposalRequest` contains `occurrence`, `analyses`, `subject`, `selector`,
`layout`, `purpose`, `applicability`, `expected_base`, `actor` and `reason`.
For example, a signed little-endian array of 100 contiguous 16-bit elements has:

```json
{"kind":"integer","encoding":{"width":2,"signed":true,"byte_order":"little"},"count":100,"stride":2}
```

Widths are 1, 2, 4 or 8 bytes; byte order is `little` or `big`. Stride is in bytes
and cannot be less than element width. The layout must cover exactly the selected
range, including internal padding. The application canonicalizes symbol/image selectors to exact section ranges,
adds matching payload/range evidence and validates it again during review.
Generic knowledge changes, specialized proposals, review and export share physical
occurrence validation. An optional image data symbol must exist in its declared
object/table even when the selector is a canonical section range. Source evidence
and the selected range are validated against that same prepared object.
This also detects conflicts between physical aliases of the same table. `purpose` and `applicability`
record the proposed meaning; their presence is not automatic semantic acceptance.

A `ConstantProposalRequest` contains `analysis`, `record`, `operand`, `value`,
`subject`, `purpose`, `applicability`, `expected_base`, `actor` and `reason`.
`value` is an RV32 unsigned bit pattern. The operand is a tagged object such as
`{"kind":"value"}`, `{"kind":"write-value"}`, `{"kind":"address"}`,
`{"kind":"call-argument","index":0}`, `{"kind":"return-low"}` or
`{"kind":"return-high"}`. It must match a known constant in that exact saved
record. Unknown values and expressions are not accepted as numeric constants.
Instruction-derived coefficients retain their analysis evidence; no fictitious
contiguous data table is created for them.

Export creates a new directory containing `manifest.json`, `object.elf`,
`data.bin` and `records.jsonl`. The object is the exact captured ELF member/image,
not its enclosing archive. The manifest records its digest, occurrence, source
file and section ranges, optional image addresses, per-range digests and offsets
into concatenated `data.bin`. The records preserve analysis IDs and ordinals;
`ranges` links known relocation references into the selected ranges. Empty links
do not assert absence of other uses. The source object's byte order is distinct
from a reviewed table's explicit interpretation.

For tables, integer records decode captured bytes. Writable sections are marked
as initialization data, never current runtime state. All section relocations are
retained. Data manifest schema 3 reports `overlapping_relocations` for the selected
byte range and `unknown_relocation_extents` for the section. Integer decoding is
withheld with an explicit `unresolved` record if either count is nonzero, even
when the layout is accepted. Known fixed-width writes ending at the range start
or starting at its end do not overlap. Unknown transformations cannot establish
nonoverlap from their offset alone. The pinned structural parser currently
supplies RV32 NONE/32/64 classifications; other types retain unknown extents.
Known writes beyond the section are rejected. ET_EXEC relocation sites are
normalized from virtual to section-relative coordinates. No relocation is
applied by data export. For constants, `data.bin` is empty; the object and
analysis instruction/value records are the evidence. Analysis coverage remains
in each retained function manifest and is not promoted by successful export.

Pointer observations use `pointer_table: {"count":11,"stride":4}` in a `DataRequest`
with exactly one selected range. The captured RV32 little-endian profile reads
four-byte slots; count/stride must cover that range exactly. For proposal through
the same `knowledge propose-data` command, use
`layout: {"kind":"pointers","count":11,"stride":4}`. Acceptance records this
layout and exact source evidence; it does not accept inferred callback signatures
or manufacture resolved external definitions.

The injected RISC-V profile `rv32-absolute-rela/1` interprets `R_RISCV_32` RELA
as a physical symbol plus addend. Defined and external symbols remain distinct.
Absolute/null symbol arithmetic is modulo 2³². A retained ET_EXEC relocation must
agree with the captured linked word; an ET_REL observation leaves the original
bytes unchanged. NONE relocations do not write. Other transformations, unknown
write extents, partial/multiple writes, unsupported symbols or implicit addends
produce a typed unresolved pointer. Invalid structural bounds fail the operation.

Each `pointer` record contains index, byte offset, captured bits and a value:
`null`, `address`, `defined-symbol`, `external-symbol` or `unresolved`. A numeric
address's `image_address` flag identifies an executable ELF address domain; it
proves neither a load mapping nor an executable target. A defined symbol may be
an internal label, not a function boundary. `pointers` summary counts each class;
there is no blanket resolved/complete verdict. `pointer_producer` identifies the
interpretation. Slot lookup uses the sorted section relocation index and bounded
write widths; it does not restart a whole-section scan for every slot.

Unreviewed observation exports have no accepted assertion. Reviewed exports
include the assertion and selected knowledge revision; pending, rejected and
superseded assertions are refused at that revision. Historical acceptance can
still be read at its original revision. The output remains available after
source removal, project move and backup/restore. Review/export does not generate
Rust, publish register definitions or claim qualification. Export never overwrites
an existing directory; a failed delivery may leave a prefix and cannot be retried
implicitly.

A data export contains the selected object and explicitly selected analyses.
Interprocedural provenance may reference analyses outside that bundle; IDs do not
include their payloads implicitly. Use project backup/restore to preserve the full
research closure in the supported format.

### Reviewed interface declarations

`knowledge validate/apply/accept/show/export` also accepts the native `interface`
claim. It declares a conditional table contract used by `interfaces` queries.
It supplies no runtime model. Existing evidence, expected-base review and physical
occurrence checks apply. Example `claim` within a `KnowledgeChange` proposal:

```json
{
  "kind": "interface",
  "contract": {
    "root": {"kind": "address", "address": 4096},
    "path": [{"kind": "load-pointer", "offset": 0}],
    "layout_version": "reviewed-layout/1",
    "layout_bytes": 16,
    "pointer_bytes": 4,
    "abi": "riscv-integer",
    "index_domains": [],
    "guards": [{"kind": "runtime-value", "offset": 0, "width": 1,
                "mask": 255, "value": 1, "purpose": "required runtime layout tag"}],
    "slots": [{
      "offset": 4,
      "name": "callback",
      "semantic": "project.callback",
      "signature": {
        "arguments": [{"role": "project.argument", "value_type": {"kind": "integer", "bits": 32, "signed": false}}],
        "result": {"kind": "integer", "bits": 32, "signed": false},
        "variadic": false
      }
    }],
    "purpose": "Explicit conditional callback interface",
    "applicability": "Selected captured occurrence with the declared runtime tag"
  }
}
```

Roots can instead be `symbol` with a physical `SymbolId` and signed `addend`, or
`function-argument` with an exact `FunctionSelector` and zero-based `argument`,
or `section` with a physical section index and byte offset.
Symbol-less function ranges are valid argument contexts. An address root is a
literal RV32 address scoped by the occurrence, not an offset into the ELF file.
Paths contain explicit `offset`, `load-pointer` and `index` steps. Each index
names an argument and byte stride and requires a unique inclusive `min`/`max`
domain with a reason. Domains are declared caller preconditions, not inferred
values. Argument positions 0..31 can be declared; this does not assert execution
support for all stack arguments or signatures.

The declaration profile has four-byte pointers, aligned nonoverlapping slots,
RV32 integer ABI, nonvariadic signatures, 8/16/32/64-bit integers, pointers and a
void result. A slot may have `signature: null` when its call signature is unknown;
this preserves a structural observation without inventing argument or return types.
At most 64 slots, 32 arguments per signature, 16 path steps, 16 guards
and eight index domains fit within the existing 64 KiB knowledge-event limit.
A `captured-payload` guard must match the exact object digest (including detached
thin-member identity). Runtime byte/halfword/word guards are bounds/mask checked;
contradictory overlapping bits fail validation. Their acceptance never means the
runtime condition was observed or satisfied. Semantic keys are reviewed labels;
they select neither machine-code callees nor external execution models.

Changed declarations for the same subject/path or provably overlapping static
root ranges conflict during acceptance. Different dynamic dereference paths are
distinct declared identities; static validation does not prove runtime non-aliasing.
Export retains review events and evidence references, not a transitive binary
backup. Use project backup/restore to preserve the captured research.


### Interface discovery and selected bindings

`interfaces --request query.json [--output observations.json]` is one supervised
read query. The optional output is an atomic JSON export to a new file. Without
it, normal human/JSON output selection applies. Example request over saved facts:

```json
{
  "input": {"kind": "analysis", "analysis": "<analysis-id>", "abi": "riscv-integer"},
  "knowledge": null
}
```

Alternatively `input` is `{"kind":"data","occurrence":<KnowledgeOccurrence>,
"selector":<DataSelector>,"layout":{"count":4,"stride":4}}`. This reads exact
captured pointer bytes through the same occurrence/relocation owner as `data`.
`captured_span` preserves section, digest and writable-initialization classification;
pointer values distinguish null, physical symbols, numeric addresses, external
symbols and unresolved transformations. Slot offsets are relative to the selected
root. Captured pointer contents do not assert function boundaries or live values.

Analysis discovery reads retained instructions, call inputs and expression facts,
including calls in ET_REL objects. It never schedules analysis or linking. Paths
retain literal/section/symbol/function-argument roots, dereferences, byte offsets,
bounded scaled argument indices and the final pointer slot. The current argument
profile requires an explicit integer ABI and supports entry registers a0..a7.
Missing facts, unsupported expressions, call results, non-pointer loads and path
bounds produce explicit issues. A known or finite call destination can lack a
retained pointer-load expression: `no-pointer-path` retains that target and does
not guess its originating table. A path is a structural may-observation, not proof
of an executable route or of runtime index/guard satisfaction.

`knowledge: null` means no declarations. A revision ID selects exactly that saved
snapshot, never the current head implicitly. The query indexes physical occurrence,
root/path and slot once. Bindings retain assertion IDs and proposed/rejected/accepted
states; only accepted matches contribute to `matched_accepted`. Multiple accepted
candidates remain visible and increment `ambiguous_bindings`. Runtime guards/index
domains yield `runtime-conditions-unverified`; neither acceptance nor matching runs
a model, resolves a callee or satisfies those conditions. Unknown signatures and
semantic keys remain null. Summary counts have no general `complete` or PASS claim.

To review a discovered path, propose an `interface` claim using the summary's exact
occurrence and payload, the observation's path/slot and its source analysis record
as evidence. Use the existing validate/apply/accept lifecycle, then repeat the query
with that explicit knowledge revision. Export preserves these identities and review
states; retain the project for transitive evidence and source-free reopening.
